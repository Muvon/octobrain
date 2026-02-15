use anyhow::{Context, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, Int32Array, ListArray, RecordBatch, StringArray,
    TimestampMillisecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase},
    Connection, DistanceType,
};
use std::sync::Arc;

use crate::knowledge::types::{KnowledgeChunk, KnowledgeSearchResult, KnowledgeStats};

pub struct KnowledgeStore {
    db: Connection,
    vector_dim: usize,
}

impl KnowledgeStore {
    fn quote_filter_string(input: &str) -> String {
        input.replace('\'', "''")
    }

    pub async fn new(vector_dim: usize) -> Result<Self> {
        let db_path = crate::storage::get_system_storage_dir()?.join("knowledge");
        std::fs::create_dir_all(&db_path)?;

        let db = connect(db_path.to_str().unwrap()).execute().await?;

        let store = Self { db, vector_dim };
        store.initialize_table().await?;

        Ok(store)
    }

    async fn initialize_table(&self) -> Result<()> {
        let table_names = self.db.table_names().execute().await?;

        if !table_names.contains(&"knowledge_chunks".to_string()) {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Utf8, false),
                Field::new("source_url", DataType::Utf8, false),
                Field::new("source_title", DataType::Utf8, false),
                Field::new("chunk_index", DataType::Int32, false),
                Field::new("content", DataType::Utf8, false),
                Field::new(
                    "section_path",
                    DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                    true,
                ),
                Field::new("char_start", DataType::Int32, false),
                Field::new("char_end", DataType::Int32, false),
                Field::new("content_hash", DataType::Utf8, false),
                Field::new(
                    "indexed_at",
                    DataType::Timestamp(TimeUnit::Millisecond, None),
                    false,
                ),
                Field::new(
                    "last_checked",
                    DataType::Timestamp(TimeUnit::Millisecond, None),
                    false,
                ),
                Field::new(
                    "embedding",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        self.vector_dim as i32,
                    ),
                    false,
                ),
            ]));

            // Create empty table
            use arrow::record_batch::RecordBatchIterator;
            use std::iter::once;
            let empty_batch = RecordBatch::new_empty(schema.clone());
            let batches = once(Ok(empty_batch));
            let batch_reader = RecordBatchIterator::new(batches, schema);
            self.db
                .create_table("knowledge_chunks", batch_reader)
                .execute()
                .await?;
        }

        Ok(())
    }

    pub async fn store_chunks(
        &self,
        source_url: &str,
        source_title: &str,
        content_hash: &str,
        chunks: &[KnowledgeChunk],
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        // Delete existing chunks for this URL (full reindex)
        self.delete_source(source_url).await?;

        if chunks.is_empty() {
            return Ok(());
        }

        let now = Utc::now();
        let now_millis = now.timestamp_millis();

        // Build arrays
        let ids: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();
        let source_urls: Vec<&str> = chunks.iter().map(|_| source_url).collect();
        let source_titles: Vec<&str> = chunks.iter().map(|_| source_title).collect();
        let chunk_indices: Vec<i32> = chunks.iter().map(|c| c.chunk_index).collect();
        let contents: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let char_starts: Vec<i32> = chunks.iter().map(|c| c.char_start as i32).collect();
        let char_ends: Vec<i32> = chunks.iter().map(|c| c.char_end as i32).collect();
        let content_hashes: Vec<&str> = chunks.iter().map(|_| content_hash).collect();
        let indexed_ats: Vec<i64> = chunks.iter().map(|_| now_millis).collect();
        let last_checkeds: Vec<i64> = chunks.iter().map(|_| now_millis).collect();

        // Build section_path list array
        let mut section_path_builder =
            arrow_array::builder::ListBuilder::new(arrow_array::builder::StringBuilder::new());
        for chunk in chunks {
            for section in &chunk.section_path {
                section_path_builder.values().append_value(section);
            }
            section_path_builder.append(true);
        }
        let section_path_array = section_path_builder.finish();

        // Build embedding array
        let embedding_values: Vec<f32> =
            embeddings.iter().flat_map(|e| e.iter().copied()).collect();
        let embedding_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, true)),
            self.vector_dim as i32,
            Arc::new(Float32Array::from(embedding_values)),
            None,
        )?;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("source_url", DataType::Utf8, false),
            Field::new("source_title", DataType::Utf8, false),
            Field::new("chunk_index", DataType::Int32, false),
            Field::new("content", DataType::Utf8, false),
            Field::new(
                "section_path",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new("char_start", DataType::Int32, false),
            Field::new("char_end", DataType::Int32, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new(
                "indexed_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new(
                "last_checked",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    self.vector_dim as i32,
                ),
                false,
            ),
        ]));

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(ids)),
                Arc::new(StringArray::from(source_urls)),
                Arc::new(StringArray::from(source_titles)),
                Arc::new(Int32Array::from(chunk_indices)),
                Arc::new(StringArray::from(contents)),
                Arc::new(section_path_array),
                Arc::new(Int32Array::from(char_starts)),
                Arc::new(Int32Array::from(char_ends)),
                Arc::new(StringArray::from(content_hashes)),
                Arc::new(TimestampMillisecondArray::from(indexed_ats)),
                Arc::new(TimestampMillisecondArray::from(last_checkeds)),
                Arc::new(embedding_array),
            ],
        )?;

        let table = self.db.open_table("knowledge_chunks").execute().await?;

        use arrow::record_batch::RecordBatchIterator;
        use std::iter::once;
        let batches = once(Ok(batch.clone()));
        let batch_reader = RecordBatchIterator::new(batches, batch.schema());
        table.add(batch_reader).execute().await?;

        Ok(())
    }

    pub async fn search(
        &self,
        query_embedding: &[f32],
        source_url: Option<&str>,
        limit: usize,
    ) -> Result<Vec<KnowledgeSearchResult>> {
        let table = self.db.open_table("knowledge_chunks").execute().await?;

        let mut query = table
            .vector_search(query_embedding)?
            .distance_type(DistanceType::Cosine)
            .limit(limit);

        if let Some(url) = source_url {
            query = query.only_if(format!("source_url = '{}'", Self::quote_filter_string(url)));
        }

        let mut results = query.execute().await?;
        let mut search_results = Vec::new();

        while let Some(batch) = results.try_next().await? {
            if batch.num_rows() == 0 {
                continue;
            }

            let ids = batch
                .column_by_name("id")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let source_urls = batch
                .column_by_name("source_url")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let source_titles = batch
                .column_by_name("source_title")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let chunk_indices = batch
                .column_by_name("chunk_index")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            let contents = batch
                .column_by_name("content")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let section_paths = batch
                .column_by_name("section_path")
                .unwrap()
                .as_any()
                .downcast_ref::<ListArray>()
                .unwrap();
            let char_starts = batch
                .column_by_name("char_start")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            let char_ends = batch
                .column_by_name("char_end")
                .unwrap()
                .as_any()
                .downcast_ref::<Int32Array>()
                .unwrap();
            let distances = batch
                .column_by_name("_distance")
                .unwrap()
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap();

            for i in 0..batch.num_rows() {
                let section_path_array = section_paths.value(i);
                let section_path_strings = section_path_array
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                let section_path: Vec<String> = (0..section_path_strings.len())
                    .map(|j| section_path_strings.value(j).to_string())
                    .collect();

                let chunk = KnowledgeChunk {
                    id: ids.value(i).to_string(),
                    source_url: source_urls.value(i).to_string(),
                    source_title: source_titles.value(i).to_string(),
                    chunk_index: chunk_indices.value(i),
                    content: contents.value(i).to_string(),
                    section_path,
                    char_start: char_starts.value(i) as usize,
                    char_end: char_ends.value(i) as usize,
                };

                let distance = distances.value(i);
                let relevance_score = 1.0 - distance;

                search_results.push(KnowledgeSearchResult {
                    chunk,
                    relevance_score,
                });
            }
        }

        Ok(search_results)
    }

    pub async fn get_source_metadata(&self, url: &str) -> Result<Option<(String, DateTime<Utc>)>> {
        let table = self.db.open_table("knowledge_chunks").execute().await?;

        let query = table
            .query()
            .only_if(format!("source_url = '{}'", Self::quote_filter_string(url)))
            .limit(1);

        let results = query.execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        if batches.is_empty() || batches[0].num_rows() == 0 {
            return Ok(None);
        }

        let batch = &batches[0];
        let content_hashes = batch
            .column_by_name("content_hash")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let last_checkeds = batch
            .column_by_name("last_checked")
            .unwrap()
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .unwrap();

        let content_hash = content_hashes.value(0).to_string();
        let last_checked_millis = last_checkeds.value(0);
        let last_checked =
            DateTime::from_timestamp_millis(last_checked_millis).context("Invalid timestamp")?;

        Ok(Some((content_hash, last_checked)))
    }

    pub async fn delete_source(&self, url: &str) -> Result<()> {
        let table = self.db.open_table("knowledge_chunks").execute().await?;
        table
            .delete(&format!(
                "source_url = '{}'",
                Self::quote_filter_string(url)
            ))
            .await?;
        Ok(())
    }

    pub async fn get_stats(&self) -> Result<KnowledgeStats> {
        let table = self.db.open_table("knowledge_chunks").execute().await?;
        let count = table.count_rows(None).await?;

        if count == 0 {
            return Ok(KnowledgeStats {
                total_sources: 0,
                total_chunks: 0,
                oldest_indexed: None,
                newest_indexed: None,
            });
        }
        // Get all data to compute stats
        let results = table.query().execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut unique_urls = std::collections::HashSet::new();
        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        for batch in batches {
            let source_urls = batch
                .column_by_name("source_url")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let indexed_ats = batch
                .column_by_name("indexed_at")
                .unwrap()
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .unwrap();

            for i in 0..batch.num_rows() {
                unique_urls.insert(source_urls.value(i).to_string());

                let indexed_millis = indexed_ats.value(i);
                if let Some(indexed) = DateTime::from_timestamp_millis(indexed_millis) {
                    if oldest.is_none_or(|old| indexed < old) {
                        oldest = Some(indexed);
                    }
                    if newest.is_none_or(|new| indexed > new) {
                        newest = Some(indexed);
                    }
                }
            }
        }

        Ok(KnowledgeStats {
            total_sources: unique_urls.len(),
            total_chunks: count,
            oldest_indexed: oldest,
            newest_indexed: newest,
        })
    }

    pub async fn list_sources(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, usize, DateTime<Utc>)>> {
        let table = self.db.open_table("knowledge_chunks").execute().await?;
        let results = table.query().execute().await?;
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        let mut sources: std::collections::HashMap<String, (String, usize, DateTime<Utc>)> =
            std::collections::HashMap::new();

        for batch in batches {
            let source_urls = batch
                .column_by_name("source_url")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let source_titles = batch
                .column_by_name("source_title")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let last_checkeds = batch
                .column_by_name("last_checked")
                .unwrap()
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .unwrap();

            for i in 0..batch.num_rows() {
                let url = source_urls.value(i).to_string();
                let title = source_titles.value(i).to_string();
                let last_checked_millis = last_checkeds.value(i);
                let last_checked = DateTime::from_timestamp_millis(last_checked_millis)
                    .context("Invalid timestamp")?;

                sources
                    .entry(url.clone())
                    .and_modify(|(_, count, existing_last_checked)| {
                        *count += 1;
                        if last_checked > *existing_last_checked {
                            *existing_last_checked = last_checked;
                        }
                    })
                    .or_insert((title, 1, last_checked));
            }
        }

        let mut result: Vec<(String, String, usize, DateTime<Utc>)> = sources
            .into_iter()
            .map(|(url, (title, count, last_checked))| (url, title, count, last_checked))
            .collect();

        // Sort by last_checked descending
        result.sort_by(|a, b| b.3.cmp(&a.3));

        if let Some(limit) = limit {
            result.truncate(limit);
        }

        Ok(result)
    }
}

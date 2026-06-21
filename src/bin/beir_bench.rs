// Copyright 2026 Muvon Un Limited
//
//! BEIR-style retrieval benchmark for octobrain's knowledge store.
//!
//! Indexes a BEIR corpus, then runs its queries through the REAL `KnowledgeStore`
//! retrieval path — vector or hybrid (BM25+vector RRF) plus optional cross-encoder
//! rerank, all driven by the config loaded via `OCTOBRAIN_CONFIG_PATH`. Reports
//! nDCG@10 / Recall@10 / Recall@100 / MRR@10 against qrels. No LLM, fully local.
//!
//! Usage:
//!   OCTOBRAIN_CONFIG_PATH=scenario.toml \
//!     cargo run --release --features bench --bin beir_bench -- <dataset_dir> [label]
//!
//! `<dataset_dir>` must contain `corpus.jsonl`, `queries.jsonl`, `qrels/test.tsv`
//! (the standard BEIR layout). `BEIR_MAX_QUERIES=N` caps the query count.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use octobrain::config::Config;
use octobrain::knowledge::store::KnowledgeStore;
use octobrain::knowledge::types::KnowledgeSearchResult;

/// Retrieval depth — must be >= the largest cutoff we report (Recall@100).
const K_RETRIEVE: usize = 100;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: beir_bench <dataset_dir> [label]");
        eprintln!("  <dataset_dir>: contains corpus.jsonl, queries.jsonl, qrels/test.tsv");
        eprintln!("  config comes from OCTOBRAIN_CONFIG_PATH (embedding model, hybrid, reranker)");
        std::process::exit(2);
    }
    let dataset_dir = PathBuf::from(&args[1]);
    let label = args.get(2).cloned().unwrap_or_else(|| "run".to_string());
    let max_queries: usize = std::env::var("BEIR_MAX_QUERIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Config provides the embedding model + reranker model. The hybrid/rerank
    // toggles are swept INTERNALLY (index once, evaluate all three) — far cheaper
    // than re-embedding the whole corpus once per scenario.
    let config = Config::load().context("loading config (set OCTOBRAIN_CONFIG_PATH)")?;
    let timeout = config.embedding.timeout_secs;
    eprintln!(
        "[bench] embedding={} reranker={}",
        config.embedding.model, config.search.reranker.model
    );

    let provider = octobrain::embedding::create_embedding_provider(&config)
        .await
        .context("creating embedding provider")?;
    let dim =
        octobrain::embedding::generate_embedding("dimension probe", provider.as_ref(), timeout)
            .await?
            .len();
    eprintln!("[bench] embedding dim = {dim}");

    // Persistent, deterministic index dir per (dataset, embedding model). The
    // slow embedding pass is paid ONCE and reused across scenario runs (so a
    // rerank-only retry costs nothing to re-index). BEIR_FRESH=1 forces a rebuild.
    // Never touches the user's real knowledge store.
    let model_tag: String = config
        .embedding
        .model
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let ds_name = dataset_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("ds");
    let db_dir = std::env::temp_dir().join(format!("octobrain_beir/{ds_name}_{model_tag}"));
    if std::env::var("BEIR_FRESH").is_ok() {
        let _ = std::fs::remove_dir_all(&db_dir);
    }
    let store = KnowledgeStore::open_at(&db_dir, dim).await?;

    // ── corpus → embed → ingest (skip if this index is already populated) ──
    let corpus = load_corpus(&dataset_dir.join("corpus.jsonl"))?;
    eprintln!("[bench] corpus: {} passages", corpus.len());

    let already = store.get_stats().await.map(|s| s.total_chunks).unwrap_or(0);
    if already == corpus.len() {
        eprintln!("[bench] reusing existing index ({already} passages)");
    } else {
        let mut ids = Vec::with_capacity(corpus.len());
        let mut titles = Vec::with_capacity(corpus.len());
        let mut texts = Vec::with_capacity(corpus.len());
        for (id, title, text) in &corpus {
            ids.push(id.clone());
            titles.push(title.clone());
            // BEIR dense baselines embed "title. text" — match that convention.
            texts.push(if title.is_empty() {
                text.clone()
            } else {
                format!("{title}. {text}")
            });
        }

        let t0 = std::time::Instant::now();
        let batch_size = config.embedding.batch_size.max(1);
        let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size) {
            let emb = octobrain::embedding::generate_embeddings_batch(
                chunk.to_vec(),
                provider.as_ref(),
                timeout,
            )
            .await
            .context("embedding corpus batch")?;
            embeddings.extend(emb);
            eprint!("\r[bench] embedded {}/{}", embeddings.len(), texts.len());
        }
        eprintln!();
        store.bulk_store(&ids, &titles, &texts, &embeddings).await?;
        eprintln!(
            "[bench] indexed {} passages in {:.1}s",
            corpus.len(),
            t0.elapsed().as_secs_f64()
        );
    }

    // ── queries + qrels ──
    let mut queries = load_queries(&dataset_dir.join("queries.jsonl"))?;
    let qrels = load_qrels(&dataset_dir.join("qrels").join("test.tsv"))?;
    queries.retain(|(qid, _)| qrels.contains_key(qid));
    if max_queries > 0 && queries.len() > max_queries {
        queries.truncate(max_queries);
    }
    eprintln!("[bench] queries with qrels: {}", queries.len());

    // ── evaluate each scenario against the SAME index ──
    // Selectable via BEIR_SCENARIOS (default all three):
    //   vector → hybrid (BM25+vector RRF) → hybrid+rerank (cross-encoder).
    let scenarios_env =
        std::env::var("BEIR_SCENARIOS").unwrap_or_else(|_| "vector,hybrid,hybrid+rerank".into());
    for name in scenarios_env
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let (use_hybrid, rerank) = match name {
            "vector" => (false, false),
            "hybrid" => (true, false),
            "hybrid+rerank" => (true, true),
            other => {
                eprintln!("[bench] unknown scenario '{other}', skipping");
                continue;
            }
        };
        let tq = std::time::Instant::now();
        let m = evaluate(
            &store,
            provider.as_ref(),
            timeout,
            &queries,
            &qrels,
            use_hybrid,
            rerank,
            &config,
        )
        .await?;
        let scen_label = format!("{label}/{name}");
        eprintln!("[bench] {scen_label}: {:.1}s", tq.elapsed().as_secs_f64());

        println!("\n=== {scen_label} ===");
        println!("embedding:  {}", config.embedding.model);
        println!(
            "scenario:   hybrid={use_hybrid} reranker={}",
            if rerank {
                config.search.reranker.model.as_str()
            } else {
                "off"
            }
        );
        println!("queries:    {}", queries.len());
        println!("nDCG@10:    {:.4}", m.ndcg10);
        println!("Recall@10:  {:.4}", m.recall10);
        println!("Recall@100: {:.4}", m.recall100);
        println!("MRR@10:     {:.4}", m.mrr10);
        println!(
            "JSON {}",
            serde_json::json!({
                "label": scen_label,
                "dataset": dataset_dir.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                "embedding": config.embedding.model,
                "scenario": name,
                "hybrid": use_hybrid,
                "reranker": if rerank { config.search.reranker.model.clone() } else { "off".to_string() },
                "queries": queries.len(),
                "ndcg@10": round4(m.ndcg10),
                "recall@10": round4(m.recall10),
                "recall@100": round4(m.recall100),
                "mrr@10": round4(m.mrr10),
            })
        );
    }

    // Index dir is intentionally NOT deleted — it's reused by later scenario runs.
    Ok(())
}

/// Evaluate one retrieval scenario over all queries, returning averaged metrics.
///
/// Ranking uses octobrain's RETURNED order — vector distance, RRF fusion, or
/// reranker order — NOT a re-sort on `relevance_score`. The hybrid score is
/// clamped/normalized and lossy (many top hits collapse to 1.0), so re-deriving
/// the rank from it would scramble the true order. The returned order IS the
/// system's ranking, which is exactly what we want to measure.
#[allow(clippy::too_many_arguments)]
async fn evaluate(
    store: &KnowledgeStore,
    provider: &dyn octobrain::embedding::EmbeddingProvider,
    timeout: u64,
    queries: &[(String, String)],
    qrels: &HashMap<String, HashMap<String, i32>>,
    use_hybrid: bool,
    rerank: bool,
    config: &Config,
) -> Result<Metrics> {
    let mut agg = Metrics::default();
    for (qid, qtext) in queries {
        let qemb = octobrain::embedding::generate_embedding(qtext, provider, timeout).await?;
        let mut results = store
            .search(&qemb, qtext, None, K_RETRIEVE, use_hybrid, None, None)
            .await?;
        if rerank && !qtext.trim().is_empty() {
            results = rerank_results(qtext, results, config).await;
        }
        let ranked: Vec<String> = results.into_iter().map(|r| r.chunk.source).collect();
        agg.add(&ranked, &qrels[qid]);
    }
    agg.finalize(queries.len());
    Ok(agg)
}

/// Rerank with the configured cross-encoder (mirrors `KnowledgeManager::rerank`).
/// Degrades to the input order on any failure.
async fn rerank_results(
    query: &str,
    mut results: Vec<KnowledgeSearchResult>,
    config: &Config,
) -> Vec<KnowledgeSearchResult> {
    let cfg = &config.search.reranker;
    let (provider, model) = match cfg.model.split_once(':') {
        Some(pm) => pm,
        None => return results,
    };
    // Rerank only the top BEIR_RERANK_DEPTH candidates (default 50) — enough to
    // fix the @10 ranking, and it bounds cross-encoder work on slow/shared CPUs.
    // The remaining tail keeps its pre-rerank (hybrid) order.
    let depth: usize = std::env::var("BEIR_RERANK_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let tail = if results.len() > depth {
        results.split_off(depth)
    } else {
        Vec::new()
    };
    let documents: Vec<String> = results.iter().map(|r| r.chunk.content.clone()).collect();
    let top_n = results.len();
    let fut = octolib::reranker::rerank(query, documents, provider, model, Some(top_n));
    let outcome = if cfg.timeout_secs == 0 {
        fut.await
    } else {
        match tokio::time::timeout(std::time::Duration::from_secs(cfg.timeout_secs), fut).await {
            Ok(inner) => inner,
            Err(_) => Err(anyhow::anyhow!("rerank timeout")),
        }
    };
    let mut out = match outcome {
        Ok(resp) => {
            let mut v = Vec::with_capacity(resp.results.len());
            for rr in resp.results {
                if let Some(orig) = results.get(rr.index) {
                    v.push(orig.clone());
                }
            }
            v
        }
        Err(e) => {
            eprintln!("[bench] rerank failed ({e}); using pre-rerank order");
            results
        }
    };
    out.extend(tail);
    out
}

// ── dataset loading ──

fn load_corpus(path: &Path) -> Result<Vec<(String, String, String)>> {
    let reader = open_lines(path)?;
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)?;
        let id = v["_id"].as_str().unwrap_or_default().to_string();
        let title = v["title"].as_str().unwrap_or_default().to_string();
        let text = v["text"].as_str().unwrap_or_default().to_string();
        if id.is_empty() || text.is_empty() {
            continue;
        }
        out.push((id, title, text));
    }
    Ok(out)
}

fn load_queries(path: &Path) -> Result<Vec<(String, String)>> {
    let reader = open_lines(path)?;
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)?;
        let id = v["_id"].as_str().unwrap_or_default().to_string();
        let text = v["text"].as_str().unwrap_or_default().to_string();
        if id.is_empty() || text.is_empty() {
            continue;
        }
        out.push((id, text));
    }
    Ok(out)
}

/// Parse `qrels/test.tsv` (header `query-id\tcorpus-id\tscore`) into
/// qid -> {docid -> relevance}. Only positive judgments are kept.
fn load_qrels(path: &Path) -> Result<HashMap<String, HashMap<String, i32>>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("open {}", path.display()))?;
    let mut map: HashMap<String, HashMap<String, i32>> = HashMap::new();
    for (i, line) in content.lines().enumerate() {
        if i == 0 && line.starts_with("query-id") {
            continue; // header
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let score: i32 = cols[2].trim().parse().unwrap_or(0);
        if score > 0 {
            map.entry(cols[0].to_string())
                .or_default()
                .insert(cols[1].to_string(), score);
        }
    }
    Ok(map)
}

fn open_lines(path: &Path) -> Result<std::io::BufReader<std::fs::File>> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(std::io::BufReader::new(f))
}

// ── metrics (reproduce trec_eval / pytrec_eval definitions) ──

#[derive(Default)]
struct Metrics {
    ndcg10: f64,
    recall10: f64,
    recall100: f64,
    mrr10: f64,
}

impl Metrics {
    fn add(&mut self, ranked: &[String], rel: &HashMap<String, i32>) {
        self.ndcg10 += ndcg_at_k(ranked, rel, 10);
        self.recall10 += recall_at_k(ranked, rel, 10);
        self.recall100 += recall_at_k(ranked, rel, 100);
        self.mrr10 += mrr_at_k(ranked, rel, 10);
    }
    fn finalize(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        let n = n as f64;
        self.ndcg10 /= n;
        self.recall10 /= n;
        self.recall100 /= n;
        self.mrr10 /= n;
    }
}

/// DCG with linear gain and log2(rank+1) discount (rank is 1-based), matching
/// trec_eval's `ndcg_cut`.
fn dcg_at_k(ranked: &[String], rel: &HashMap<String, i32>, k: usize) -> f64 {
    ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, d)| {
            let g = *rel.get(d).unwrap_or(&0) as f64;
            if g > 0.0 {
                g / (i as f64 + 2.0).log2()
            } else {
                0.0
            }
        })
        .sum()
}

fn ndcg_at_k(ranked: &[String], rel: &HashMap<String, i32>, k: usize) -> f64 {
    let dcg = dcg_at_k(ranked, rel, k);
    let mut ideal: Vec<i32> = rel.values().copied().collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    let idcg: f64 = ideal
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &g)| g as f64 / (i as f64 + 2.0).log2())
        .sum();
    if idcg > 0.0 {
        dcg / idcg
    } else {
        0.0
    }
}

fn recall_at_k(ranked: &[String], rel: &HashMap<String, i32>, k: usize) -> f64 {
    let total = rel.values().filter(|&&v| v > 0).count();
    if total == 0 {
        return 0.0;
    }
    let hit = ranked
        .iter()
        .take(k)
        .filter(|d| rel.get(*d).map(|&v| v > 0).unwrap_or(false))
        .count();
    hit as f64 / total as f64
}

fn mrr_at_k(ranked: &[String], rel: &HashMap<String, i32>, k: usize) -> f64 {
    for (i, d) in ranked.iter().take(k).enumerate() {
        if rel.get(d).map(|&v| v > 0).unwrap_or(false) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Typed accessors for Arrow `RecordBatch` columns.
//!
//! LanceDB returns query results as Arrow `RecordBatch`es whose columns must be
//! looked up by name and downcast to a concrete array type before their values
//! can be read. Both the memory and knowledge stores used to spell this out per
//! column with the same verbose `column_by_name(..).and_then(downcast).ok_or_else(..)`
//! incantation (or, worse, `.unwrap().as_any().downcast_ref().unwrap()` which
//! panics on a malformed batch). These helpers collapse it to a single call and
//! return a consistent, descriptive error when a column is missing or has an
//! unexpected type.

use anyhow::{anyhow, Result};
use arrow_array::{
    Array, Float32Array, Int32Array, ListArray, RecordBatch, StringArray, TimestampMillisecondArray,
};

/// Required UTF-8 string column.
pub fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    required(batch, name)
}

/// Required 32-bit float column.
pub fn f32_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Float32Array> {
    required(batch, name)
}

/// Required 32-bit integer column.
pub fn i32_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    required(batch, name)
}

/// Required list column.
pub fn list_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ListArray> {
    required(batch, name)
}

/// Required millisecond-timestamp column.
pub fn timestamp_ms_column<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    required(batch, name)
}

/// Optional UTF-8 string column — `None` when the column is absent or mistyped.
/// Used for columns that may not exist on legacy tables mid-migration.
pub fn string_column_opt<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    optional(batch, name)
}

/// Optional 32-bit integer column.
pub fn i32_column_opt<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Int32Array> {
    optional(batch, name)
}

/// Optional 32-bit float column.
pub fn f32_column_opt<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a Float32Array> {
    optional(batch, name)
}

/// Generic required-column accessor backing the typed wrappers above.
fn required<'a, A: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a A> {
    batch
        .column_by_name(name)
        .ok_or_else(|| anyhow!("column '{}' not found in record batch", name))?
        .as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| anyhow!("column '{}' has an unexpected Arrow type", name))
}

/// Generic optional-column accessor backing the typed `*_opt` wrappers above.
fn optional<'a, A: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Option<&'a A> {
    batch.column_by_name(name)?.as_any().downcast_ref::<A>()
}

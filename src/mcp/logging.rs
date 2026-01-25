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

use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::info;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt::Layer, prelude::*, registry::Registry, EnvFilter};

static MCP_LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Initialize logging for MCP server with file rotation
/// All logs go to files only - NO console output to maintain MCP protocol compliance
pub fn init_mcp_logging(base_dir: PathBuf, debug_mode: bool) -> Result<(), anyhow::Error> {
    // Use the system-wide storage directory for logs
    let project_storage = crate::storage::get_project_storage_path(&base_dir)?;
    let log_dir = project_storage.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // Store log directory for potential future use
    MCP_LOG_DIR
        .set(log_dir.clone())
        .map_err(|_| anyhow::anyhow!("Failed to set log directory"))?;

    // Cross-platform way to create a "latest" indicator
    let latest_file = project_storage.join("latest_log.txt");
    // Silently ignore errors creating latest log indicator to maintain MCP protocol compliance
    let _ = std::fs::write(&latest_file, log_dir.to_string_lossy().as_bytes());

    // Create rotating file appender
    let file_appender = RollingFileAppender::new(Rotation::DAILY, &log_dir, "mcp_server.log");

    // Set up environment filter with sensible defaults
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if debug_mode {
            // In debug mode, show debug level for our crate, but info for others
            EnvFilter::new("info,octobrain=debug")
        } else {
            // In production mode, only show info and above
            EnvFilter::new("info,octobrain::mcp::logging=info")
        }
    });

    // File layer with JSON formatting for structured logs
    let file_layer = Layer::new()
        .with_writer(file_appender)
        .with_ansi(false)
        .with_target(true)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .json();

    // MCP protocol requires clean stdout/stderr - no console output allowed
    // All logging must go to files only to maintain protocol compliance

    // Create registry with file layer only (no console output)
    let registry = Registry::default().with(file_layer).with(env_filter);
    registry.init();

    info!(
        project_path = %base_dir.display(),
        log_directory = %log_dir.display(),
        debug_mode = debug_mode,
        "MCP Server logging initialized"
    );

    Ok(())
}

/// Get the current log directory
#[allow(dead_code)]
pub fn get_log_directory() -> Option<PathBuf> {
    MCP_LOG_DIR.get().cloned()
}

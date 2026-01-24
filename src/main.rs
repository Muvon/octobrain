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

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, EnvFilter};

mod cli;
mod commands;
mod config;
mod constants;
mod embedding;
mod mcp;
mod memory;
mod storage;
mod vector_optimizer;

use cli::Cli;
use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing subscriber for logging
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("octobrain=info"));

    fmt().with_env_filter(filter).with_target(false).init();

    // Parse command line arguments
    let cli = Cli::parse();

    // Load configuration
    let config = Config::load()?;

    // Execute the command
    if let Err(e) = commands::execute(&config, cli.command).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}

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
use std::fs;
use std::path::PathBuf;

/// Get the system-wide storage directory for Octobrain
/// Following XDG Base Directory specification on Unix-like systems
/// and proper conventions on other systems
pub fn get_system_storage_dir() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "macos") {
        // macOS: ~/.local/share/octobrain
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Unable to determine home directory"))?
            .join(".local")
            .join("share")
            .join("octobrain")
    } else if cfg!(target_os = "windows") {
        // Windows: %APPDATA%/octobrain
        dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Unable to determine data directory"))?
            .join("octobrain")
    } else {
        // Linux and other Unix-like: ~/.local/share/octobrain or $XDG_DATA_HOME/octobrain
        if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg_data_home).join("octobrain")
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Unable to determine home directory"))?
                .join(".local")
                .join("share")
                .join("octobrain")
        }
    };

    // Create directory if it doesn't exist
    if !base_dir.exists() {
        fs::create_dir_all(&base_dir)?;
    }

    Ok(base_dir)
}

/// Get the shared memory database path.
/// All projects share a single LanceDB at this location; rows are scoped by the `scope` column.
pub fn get_memory_database_path() -> Result<PathBuf> {
    let system_dir = get_system_storage_dir()?;
    Ok(system_dir.join("memory"))
}

/// Get the directory holding knowledge-box state: the subscription registry and
/// cloned remote box working trees. Created on demand.
pub fn get_boxes_dir() -> Result<PathBuf> {
    let dir = get_system_storage_dir()?.join("boxes");
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// Get the system config file path
/// Stored directly under ~/.local/share/octobrain/ on all systems
pub fn get_system_config_path() -> Result<PathBuf> {
    let system_dir = get_system_storage_dir()?;
    Ok(system_dir.join("config.toml"))
}

/// Get the config file path, respecting the OCTOBRAIN_CONFIG_PATH environment variable.
/// If the env var is set, returns that path directly. Otherwise falls back to the
/// system config path.
pub fn get_config_path() -> Result<PathBuf> {
    if let Ok(env_path) = std::env::var("OCTOBRAIN_CONFIG_PATH") {
        Ok(PathBuf::from(env_path))
    } else {
        get_system_config_path()
    }
}

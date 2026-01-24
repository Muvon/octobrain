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
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Get project identifier for a given directory
/// First tries to get Git remote URL, falls back to path hash
pub fn get_project_identifier(project_path: &Path) -> Result<String> {
    // Try to get git remote URL first
    if let Ok(git_remote) = get_git_remote_url(project_path) {
        // Create a hash from git remote URL
        let mut hasher = Sha256::new();
        hasher.update(git_remote.as_bytes());
        let result = hasher.finalize();
        return Ok(format!("{:x}", result)[..16].to_string()); // Use first 16 chars
    }

    // Fallback to absolute path hash
    let absolute_path = project_path.canonicalize().or_else(|_| {
        // If canonicalize fails, try to get absolute path manually
        if project_path.is_absolute() {
            Ok(project_path.to_path_buf())
        } else {
            std::env::current_dir().map(|cwd| cwd.join(project_path))
        }
    })?;

    let mut hasher = Sha256::new();
    hasher.update(absolute_path.to_string_lossy().as_bytes());
    let result = hasher.finalize();
    Ok(format!("{:x}", result)[..16].to_string()) // Use first 16 chars
}

/// Try to get the Git remote URL for a project
fn get_git_remote_url(project_path: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()?;

    if output.status.success() {
        let url = String::from_utf8(output.stdout)?.trim().to_string();

        if !url.is_empty() {
            return Ok(normalize_git_url(&url));
        }
    }

    Err(anyhow::anyhow!("No git remote found"))
}

/// Normalize git URL to be consistent regardless of protocol
/// e.g., https://github.com/user/repo.git and git@github.com:user/repo.git
/// both become github.com/user/repo
fn normalize_git_url(url: &str) -> String {
    let url = url.trim();

    // Remove .git suffix if present
    let url = if let Some(stripped) = url.strip_suffix(".git") {
        stripped
    } else {
        url
    };

    // Handle SSH format: git@host:user/repo
    if url.contains("@") && url.contains(":") && !url.contains("://") {
        if let Some(at_pos) = url.find('@') {
            if let Some(colon_pos) = url[at_pos..].find(':') {
                let host = &url[at_pos + 1..at_pos + colon_pos];
                let path = &url[at_pos + colon_pos + 1..];
                return format!("{}/{}", host, path);
            }
        }
    }

    // Handle HTTPS format: https://host/user/repo
    if url.starts_with("http://") || url.starts_with("https://") {
        if let Some(scheme_end) = url.find("://") {
            return url[scheme_end + 3..].to_string();
        }
    }

    // Return as-is if we can't parse it
    url.to_string()
}

/// Get the storage path for a specific project
pub fn get_project_storage_path(project_path: &Path) -> Result<PathBuf> {
    let system_dir = get_system_storage_dir()?;
    let project_id = get_project_identifier(project_path)?;

    Ok(system_dir.join(project_id))
}

/// Get database path for a specific project
pub fn get_project_database_path(project_path: &Path) -> Result<PathBuf> {
    let project_storage = get_project_storage_path(project_path)?;
    Ok(project_storage.join("storage"))
}

/// Get the system config file path
/// Stored directly under ~/.local/share/octobrain/ on all systems
pub fn get_system_config_path() -> Result<PathBuf> {
    let system_dir = get_system_storage_dir()?;
    Ok(system_dir.join("config.toml"))
}

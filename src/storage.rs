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

/// Derive a human-readable scope string for the given directory.
///
/// Resolution order:
///   1. Git remote URL → normalized as `host/org/repo` (e.g. `github.com/muvon/octobrain`)
///   2. No git remote → `local/<parent>/<dir>` derived from the last two path segments
///
/// The global scope `""` is reserved and never produced by this function.
pub fn derive_scope(project_path: &Path) -> String {
    // Try git remote first
    if let Some(scope) = try_git_scope(project_path) {
        return scope;
    }
    // Fall back to local path-based scope
    local_scope(project_path)
}

/// Attempt to derive scope from `git remote get-url origin`. Returns `None` on any failure
/// so the caller can fall back to the local path scheme.
fn try_git_scope(project_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "-C",
            &project_path.to_string_lossy(),
            "remote",
            "get-url",
            "origin",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let url = raw.trim();
    if url.is_empty() {
        return None;
    }

    let normalized = normalize_git_url(url);
    if normalized.is_empty() {
        tracing::debug!(
            "git remote URL '{}' could not be normalized; using local scope",
            url
        );
        return None;
    }
    Some(normalized)
}

/// Derive `local/<parent>/<dir>` from the last two segments of the path.
/// Gracefully degrades to `local/<dir>` when there is no parent segment.
fn local_scope(project_path: &Path) -> String {
    let absolute = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());

    let mut components: Vec<String> = absolute
        .components()
        .filter_map(|c| {
            let s = c.as_os_str().to_string_lossy();
            if s.is_empty() || s == "/" {
                None
            } else {
                Some(s.to_lowercase())
            }
        })
        .collect();

    match components.len() {
        0 => "local/unknown".to_string(),
        1 => format!("local/{}", components.remove(0)),
        _ => {
            let dir = components.pop().unwrap();
            let parent = components.pop().unwrap();
            format!("local/{}/{}", parent, dir)
        }
    }
}

/// Normalize a git remote URL to `host/org/repo` (lowercase, no trailing slash, no `.git`).
///
/// Handles:
/// - SSH: `git@github.com:org/repo.git` → `github.com/org/repo`
/// - HTTPS: `https://github.com/org/repo.git` → `github.com/org/repo`
pub fn normalize_git_url(url: &str) -> String {
    let url = url.trim();

    // Remove .git suffix if present
    let url = url.strip_suffix(".git").unwrap_or(url);

    let raw = if url.contains('@') && url.contains(':') && !url.contains("://") {
        // SSH format: git@host:user/repo
        if let Some(at_pos) = url.find('@') {
            if let Some(colon_pos) = url[at_pos..].find(':') {
                let host = &url[at_pos + 1..at_pos + colon_pos];
                let path = &url[at_pos + colon_pos + 1..];
                format!("{}/{}", host, path)
            } else {
                url.to_string()
            }
        } else {
            url.to_string()
        }
    } else if let Some(scheme_end) = url.find("://") {
        // HTTPS / HTTP format
        url[scheme_end + 3..].to_string()
    } else {
        url.to_string()
    };

    // Lowercase and strip leading/trailing slashes
    raw.to_lowercase().trim_matches('/').to_string()
}

/// Get the shared memory database path.
/// All projects share a single LanceDB at this location; rows are scoped by the `scope` column.
pub fn get_memory_database_path() -> Result<PathBuf> {
    let system_dir = get_system_storage_dir()?;
    Ok(system_dir.join("memory"))
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

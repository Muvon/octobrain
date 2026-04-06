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
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// What happened to a file since a given commit
#[derive(Debug, PartialEq)]
pub enum FileFate {
    /// File still exists at the same path
    Exists,
    /// File was renamed/moved — contains the new path
    Renamed(String),
    /// File was deleted
    Deleted,
    /// No git context available — caller should fall back to file_exists()
    Unknown,
}

/// Pre-built map of old_path → new_path for renames detected by git.
/// Built once from a single `git log` call, then queried per file.
pub struct RenameMap {
    renames: HashMap<String, String>,
}

impl RenameMap {
    /// Build a rename map from git history since the given commit.
    /// Single git call: `git log --diff-filter=R --name-status --format="" {since}..HEAD`
    /// Output lines: "R078\told_path\tnew_path"
    /// Only includes renames where the new path still exists on disk.
    pub fn build(since_commit: &str) -> Self {
        let mut renames = HashMap::new();

        let output = Command::new("git")
            .args([
                "log",
                "--diff-filter=R",
                "--name-status",
                "--format=",
                &format!("{since_commit}..HEAD"),
            ])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if let [status, old, new] = parts.as_slice() {
                        if status.starts_with('R') {
                            let old = old.trim().to_string();
                            let new = new.trim().to_string();
                            // Only record if the new path still exists
                            if GitUtils::file_exists(&new) {
                                renames.insert(old, new);
                            }
                        }
                    }
                }
            }
        }

        Self { renames }
    }

    /// Look up where a file was renamed to, if anywhere.
    pub fn renamed_to(&self, old_path: &str) -> Option<&str> {
        self.renames.get(old_path).map(|s| s.as_str())
    }
}

/// Utilities for Git operations
pub struct GitUtils;

impl GitUtils {
    /// Get the current Git commit hash
    pub fn get_current_commit() -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()?;

        if output.status.success() {
            let commit = String::from_utf8(output.stdout).ok()?;
            Some(commit.trim().to_string())
        } else {
            None
        }
    }

    /// Get the Git repository root directory
    pub fn get_repository_root() -> Option<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()?;

        if output.status.success() {
            let root = String::from_utf8(output.stdout).ok()?;
            Some(root.trim().to_string())
        } else {
            None
        }
    }

    /// Get files modified in the current working directory
    pub fn get_modified_files() -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["diff", "--name-only", "HEAD"])
            .output()?;

        if output.status.success() {
            let files_str = String::from_utf8(output.stdout)?;
            let files: Vec<String> = files_str
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string())
                .collect();
            Ok(files)
        } else {
            Ok(Vec::new())
        }
    }

    /// Determine what happened to a file using a pre-built RenameMap.
    /// Fast path: file exists → Exists.
    /// Then checks the rename map for a surviving rename target.
    /// Otherwise → Deleted.
    pub fn check_file_fate(relative_path: &str, rename_map: Option<&RenameMap>) -> FileFate {
        if Self::file_exists(relative_path) {
            return FileFate::Exists;
        }

        let rename_map = match rename_map {
            Some(m) => m,
            None => return FileFate::Unknown,
        };

        if let Some(new_path) = rename_map.renamed_to(relative_path) {
            return FileFate::Renamed(new_path.to_string());
        }

        FileFate::Deleted
    }

    /// Check if a file exists relative to the repository root.
    /// Returns true if the repo root can be resolved and the file exists on disk.
    pub fn file_exists(relative_path: &str) -> bool {
        if let Some(root) = Self::get_repository_root() {
            Path::new(&root).join(relative_path).exists()
        } else {
            // No git repo — try the path as-is (could be absolute)
            Path::new(relative_path).exists()
        }
    }

    /// Get the relative path from repository root
    pub fn get_relative_path<P: AsRef<Path>>(file_path: P) -> Option<String> {
        if let Some(repo_root) = Self::get_repository_root() {
            if let Ok(absolute_path) = file_path.as_ref().canonicalize() {
                if let Ok(relative) = absolute_path.strip_prefix(&repo_root) {
                    return Some(relative.to_string_lossy().to_string());
                }
            }
        }
        file_path.as_ref().to_str().map(|s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rename_map_parse() {
        // Simulate git output parsing
        let map = RenameMap {
            renames: HashMap::from([
                ("src/old.rs".to_string(), "src/new.rs".to_string()),
                ("lib/foo.rs".to_string(), "lib/bar.rs".to_string()),
            ]),
        };

        assert_eq!(map.renamed_to("src/old.rs"), Some("src/new.rs"));
        assert_eq!(map.renamed_to("lib/foo.rs"), Some("lib/bar.rs"));
        assert_eq!(map.renamed_to("nonexistent.rs"), None);
    }

    #[test]
    fn test_check_file_fate_with_rename_map() {
        let map = RenameMap {
            renames: HashMap::from([(
                "src/deleted_then_renamed.rs".to_string(),
                "src/alive.rs".to_string(),
            )]),
        };

        // File that doesn't exist and isn't in rename map → Deleted
        let fate = GitUtils::check_file_fate("src/totally_gone.rs", Some(&map));
        assert_eq!(fate, FileFate::Deleted);

        // No rename map → Unknown
        let fate = GitUtils::check_file_fate("src/totally_gone.rs", None);
        assert_eq!(fate, FileFate::Unknown);
    }

    #[test]
    fn test_check_file_fate_existing_file() {
        // Cargo.toml exists in any Rust project
        let fate = GitUtils::check_file_fate(
            "Cargo.toml",
            Some(&RenameMap {
                renames: HashMap::new(),
            }),
        );
        assert_eq!(fate, FileFate::Exists);
    }

    #[test]
    fn test_file_exists_real_file() {
        assert!(GitUtils::file_exists("Cargo.toml"));
        assert!(!GitUtils::file_exists("nonexistent_file_xyz_123.rs"));
    }
}

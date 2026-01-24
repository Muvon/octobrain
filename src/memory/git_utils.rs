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
use std::path::Path;
use std::process::Command;

/// Utilities for Git operations
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

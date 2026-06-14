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

//! Knowledge boxes: git-backed, scoped bundles of structured org/project knowledge.
//!
//! A box is a git folder of source documents that octobrain re-embeds locally and
//! makes searchable, scoped exactly like memories. Boxes ship source files only —
//! never vectors. This module holds the pure, side-effect-light helpers: the
//! subscription registry, git plumbing, scope math, and the taxonomy file walk.
//! The actual embed/store lives in `KnowledgeManager` (it owns the store + provider).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::knowledge::content::ContentType;

/// The conventional repo/folder name carrying a box.
pub const BOX_REPO_NAME: &str = "octobrain-box";

/// Project-local box directory, relative to a repo root.
pub const PROJECT_BOX_DIR: &str = ".box";

/// The `box://` source URI prefix used for all box-originated knowledge rows.
pub const BOX_URI_PREFIX: &str = "box://";

// ============================================================================
// Registry (subscribed remote boxes + negative-probe cache)
// ============================================================================

/// A subscribed remote box (org / global). Project `.box/` boxes are NOT recorded
/// here — they are rediscovered from the working tree on every sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBox {
    /// Git clone URL.
    pub url: String,
    /// Stable id (`host/org/repo`) used as the `box://<box_id>/` source prefix.
    pub box_id: String,
    /// Bound scope these rows are tagged with (e.g. `github.com/acme` or "").
    pub scope: String,
    #[serde(default)]
    pub last_commit: String,
    #[serde(default)]
    pub last_synced: String,
}

/// Result of probing an org for the conventional `octobrain-box` repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub has_box: bool,
    pub checked: String,
}

/// On-disk box registry (`<boxes_dir>/registry.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoxRegistry {
    #[serde(default)]
    pub boxes: Vec<RemoteBox>,
    /// Negative/positive probe cache keyed by org scope (`host/org`).
    #[serde(default)]
    pub probed: HashMap<String, ProbeResult>,
}

impl BoxRegistry {
    fn path() -> Result<PathBuf> {
        Ok(crate::storage::get_boxes_dir()?.join("registry.toml"))
    }

    /// Load the registry, returning an empty one when the file is absent.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read box registry: {}", path.display()))?;
        Ok(toml::from_str(&content).unwrap_or_default())
    }

    /// Persist the registry, creating the boxes dir if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string(self).context("Failed to serialize box registry")?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write box registry: {}", path.display()))?;
        Ok(())
    }

    /// Insert or replace a remote box by `box_id`.
    pub fn upsert(&mut self, b: RemoteBox) {
        self.boxes.retain(|e| e.box_id != b.box_id);
        self.boxes.push(b);
    }

    /// Remove a remote box by `box_id`; returns it when present.
    pub fn remove(&mut self, box_id: &str) -> Option<RemoteBox> {
        if let Some(pos) = self.boxes.iter().position(|e| e.box_id == box_id) {
            Some(self.boxes.remove(pos))
        } else {
            None
        }
    }

    pub fn find(&self, box_id: &str) -> Option<&RemoteBox> {
        self.boxes.iter().find(|e| e.box_id == box_id)
    }
}

// ============================================================================
// Scope math
// ============================================================================

/// All path prefixes of a scope, shortest first.
/// `github.com/acme/billing` -> [`github.com`, `github.com/acme`, `github.com/acme/billing`].
pub fn scope_prefixes(scope: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for part in scope.split('/').filter(|s| !s.is_empty()) {
        if acc.is_empty() {
            acc = part.to_string();
        } else {
            acc.push('/');
            acc.push_str(part);
        }
        out.push(acc.clone());
    }
    out
}

/// The set of scopes visible from an active scope, including global ("").
///
/// Mirrors the memory scope rule, extended with the org/ancestor tier:
/// - `None`        -> `None` (no filter — admin/unscoped, all scopes)
/// - `Some("")`    -> just global
/// - `Some(s)`     -> every prefix of `s` plus global
pub fn visible_scopes(active: Option<&str>) -> Option<Vec<String>> {
    match active {
        None => None,
        Some("") => Some(vec![String::new()]),
        Some(s) => {
            let mut v = scope_prefixes(s);
            v.push(String::new());
            Some(v)
        }
    }
}

/// The org scope (`host/org`) of a fuller scope, when it has at least two segments.
pub fn org_scope(scope: &str) -> Option<String> {
    let parts: Vec<&str> = scope.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

/// Conventional clone URL for an org's box repo: `https://<org>/octobrain-box.git`.
pub fn org_box_url(org: &str) -> String {
    format!("https://{}/{}.git", org, BOX_REPO_NAME)
}

/// Conventional box id for an org's box repo: `<org>/octobrain-box`.
pub fn org_box_id(org: &str) -> String {
    format!("{}/{}", org, BOX_REPO_NAME)
}

/// Stable box id from a clone URL — reuses the same normalization as scope derivation.
pub fn box_id_from_url(url: &str) -> String {
    crate::storage::normalize_git_url(url)
}

/// Filesystem-safe slug for a box id (clone directory name).
pub fn slug(box_id: &str) -> String {
    box_id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The `box://<box_id>/` source prefix used to group / delete a box's rows.
pub fn source_prefix(box_id: &str) -> String {
    format!("{}{}/", BOX_URI_PREFIX, box_id)
}

/// Build the portable source URI for a file inside a box.
pub fn source_uri(box_id: &str, relpath: &str) -> String {
    format!("{}{}/{}", BOX_URI_PREFIX, box_id, relpath)
}

// ============================================================================
// Taxonomy walk
// ============================================================================

/// Collect every indexable file under a box root as `(absolute_path, relpath)`.
///
/// Skips `.git`, the optional `octobrain-box.toml` manifest, and a root `README.md`.
/// Files with unsupported extensions are skipped silently.
pub fn collect_indexable(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    collect_rec(root, root, &mut out);
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn collect_rec(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name == ".git" {
                continue;
            }
            collect_rec(root, &path, out);
        } else {
            let rel = match path.strip_prefix(root) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            // Manifest and root onboarding readme are not indexed.
            if rel == "octobrain-box.toml" || rel == "README.md" {
                continue;
            }
            if ContentType::from_extension(&name).is_some() {
                out.push((path, rel));
            }
        }
    }
}

// ============================================================================
// Git plumbing (non-interactive — never prompts for credentials)
// ============================================================================

fn git() -> Command {
    let mut c = Command::new("git");
    // Never block on a credential prompt — private/unreachable repos just fail fast.
    c.env("GIT_TERMINAL_PROMPT", "0");
    c
}

/// True when the remote exists and is reachable (no auth prompt).
pub fn remote_exists(url: &str) -> bool {
    git()
        .args(["ls-remote", url])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Shallow-clone a remote box into `dest`.
pub fn clone(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = git()
        .args(["clone", "--depth", "1", url, &dest.to_string_lossy()])
        .output()
        .context("Failed to run git clone")?;
    if !out.status.success() {
        anyhow::bail!("git clone failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Fast-forward pull an existing clone. Returns Ok(()) even if already up to date.
pub fn pull(dir: &Path) -> Result<()> {
    let out = git()
        .args(["-C", &dir.to_string_lossy(), "pull", "--ff-only"])
        .output()
        .context("Failed to run git pull")?;
    if !out.status.success() {
        anyhow::bail!("git pull failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Current HEAD commit of a clone, if resolvable.
pub fn head_commit(dir: &Path) -> Option<String> {
    let out = git()
        .args(["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_expand_all_segments() {
        assert_eq!(
            scope_prefixes("github.com/acme/billing"),
            vec!["github.com", "github.com/acme", "github.com/acme/billing"]
        );
        assert!(scope_prefixes("").is_empty());
    }

    #[test]
    fn visible_mirrors_memory_rule() {
        assert_eq!(visible_scopes(None), None);
        assert_eq!(visible_scopes(Some("")), Some(vec![String::new()]));
        assert_eq!(
            visible_scopes(Some("github.com/acme/billing")),
            Some(vec![
                "github.com".into(),
                "github.com/acme".into(),
                "github.com/acme/billing".into(),
                "".into(),
            ])
        );
    }

    #[test]
    fn org_helpers() {
        assert_eq!(
            org_scope("github.com/acme/billing").as_deref(),
            Some("github.com/acme")
        );
        assert_eq!(org_scope("local").as_deref(), None);
        assert_eq!(
            org_box_url("github.com/acme"),
            "https://github.com/acme/octobrain-box.git"
        );
        assert_eq!(
            org_box_id("github.com/acme"),
            "github.com/acme/octobrain-box"
        );
    }

    #[test]
    fn source_uri_and_prefix() {
        assert_eq!(
            source_uri("github.com/acme/octobrain-box", "rules/x.md"),
            "box://github.com/acme/octobrain-box/rules/x.md"
        );
        assert_eq!(
            source_prefix("github.com/acme/octobrain-box"),
            "box://github.com/acme/octobrain-box/"
        );
    }

    #[test]
    fn slug_is_fs_safe() {
        assert_eq!(
            slug("github.com/acme/octobrain-box"),
            "github.com_acme_octobrain-box"
        );
    }
}

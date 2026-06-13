use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const SESSION_DIR: &str = ".tattle";
pub const SESSION_FILE: &str = ".tattle/session.json";
pub const HELDOUT_DIR: &str = ".tattle/heldout";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub mission: String,
    pub agent: Option<String>,
    pub git_baseline: String,
    pub test_cmds: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub heldout_snapshotted: bool,
}

pub fn start(
    root: &Path,
    mission: String,
    agent: Option<String>,
    extra_test_cmds: Vec<String>,
) -> Result<Session> {
    let git_baseline = crate::git::capture_baseline(root)?;

    let test_cmds = if extra_test_cmds.is_empty() {
        detect_test_cmds(root)
    } else {
        extra_test_cmds
    };

    let heldout_dir = root.join(HELDOUT_DIR);
    fs::create_dir_all(&heldout_dir)
        .with_context(|| format!("create {}", heldout_dir.display()))?;

    let snapshotted = snapshot_test_files(root, &heldout_dir)?;

    let session = Session {
        mission,
        agent,
        git_baseline,
        test_cmds,
        started_at: Utc::now(),
        heldout_snapshotted: snapshotted > 0,
    };
    save(root, &session)?;
    Ok(session)
}

pub fn load(root: &Path) -> Result<Option<Session>> {
    let path = root.join(SESSION_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let session =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(session))
}

pub fn save(root: &Path, session: &Session) -> Result<PathBuf> {
    let dir = root.join(SESSION_DIR);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = root.join(SESSION_FILE);
    let json = serde_json::to_string_pretty(session).context("serialize session")?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Copy every test file from root into heldout_dir, preserving relative paths.
/// Returns count of files snapshotted.
pub fn snapshot_test_files(root: &Path, heldout_dir: &Path) -> Result<usize> {
    let mut count = 0;
    for result in WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| {
            // Skip .tattle dir itself
            e.path().components().all(|c| c.as_os_str() != ".tattle")
        })
        .build()
    {
        let entry = result?;
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            let rel_str = rel.to_string_lossy();
            if is_test_file(&rel_str) {
                let dest = heldout_dir.join(rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &dest)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

pub fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.contains("test/")
        || lower.contains("__tests__")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.go")
        || lower.ends_with("test.py")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with(".test.js")
        || lower.ends_with(".spec.js")
        || lower.ends_with("test.java")
        || lower.contains("src/test/")
}

fn detect_test_cmds(root: &Path) -> Vec<String> {
    if root.join("package.json").exists() {
        return vec!["npm test".to_string()];
    }
    if root.join("Cargo.toml").exists() {
        return vec!["cargo test".to_string()];
    }
    if root.join("go.mod").exists() {
        return vec!["go test ./...".to_string()];
    }
    if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        return vec!["python -m pytest".to_string()];
    }
    vec![]
}

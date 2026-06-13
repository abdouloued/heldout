use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::session::{is_test_file, HELDOUT_DIR};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayResult {
    pub command: String,
    pub ran: bool,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub skipped_reason: Option<String>,
}

impl ReplayResult {
    fn skipped(command: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ran: false,
            passed: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            skipped_reason: Some(reason.into()),
        }
    }
}

/// Run held-out replay: copy working tree to temp, restore original test files, then run test_cmds.
pub fn run_held_out(root: &Path, test_cmds: &[String], timeout_secs: u64) -> Vec<ReplayResult> {
    if test_cmds.is_empty() {
        return vec![ReplayResult::skipped(
            "<no test command>",
            "no test_cmd configured — set replay.commands in heldout.yaml or use --test-cmd",
        )];
    }

    let heldout_dir = root.join(HELDOUT_DIR);
    if !heldout_dir.exists() {
        return test_cmds
            .iter()
            .map(|cmd| {
                ReplayResult::skipped(cmd, "held-out snapshot missing — run `heldout start` first")
            })
            .collect();
    }

    let sandbox = match build_sandbox(root, &heldout_dir) {
        Ok(d) => d,
        Err(e) => {
            return test_cmds
                .iter()
                .map(|cmd| ReplayResult::skipped(cmd, format!("sandbox creation failed: {e}")))
                .collect();
        }
    };

    test_cmds
        .iter()
        .map(|cmd| run_in_sandbox(cmd, sandbox.path(), timeout_secs))
        .collect()
}

/// Copy working tree to a tempdir, then overwrite test files with held-out snapshots.
fn build_sandbox(root: &Path, heldout_dir: &Path) -> Result<tempfile::TempDir> {
    let sandbox = tempfile::tempdir().context("create sandbox tempdir")?;

    // Copy all non-.heldout files from working tree
    for result in WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|e| {
            e.path()
                .components()
                .all(|c| c.as_os_str() != ".heldout" && c.as_os_str() != ".git")
        })
        .build()
    {
        let entry = result?;
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let rel = entry.path().strip_prefix(root).unwrap_or(entry.path());
            let dest = sandbox.path().join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &dest)?;
        }
    }

    // Overwrite test files with held-out originals
    for result in WalkBuilder::new(heldout_dir).hidden(false).build() {
        let entry = result?;
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let rel = entry
                .path()
                .strip_prefix(heldout_dir)
                .unwrap_or(entry.path());
            let dest = sandbox.path().join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &dest)?;
        }
    }

    // Remove any new test files the agent added that aren't in the held-out snapshot
    // (they would execute against agent-rewritten tests if left in)
    for result in WalkBuilder::new(sandbox.path()).hidden(false).build() {
        let entry = result?;
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            let rel = entry
                .path()
                .strip_prefix(sandbox.path())
                .unwrap_or(entry.path());
            let rel_str = rel.to_string_lossy();
            if is_test_file(&rel_str) && !heldout_dir.join(rel).exists() {
                fs::remove_file(entry.path()).ok();
            }
        }
    }

    Ok(sandbox)
}

fn run_in_sandbox(command: &str, cwd: &Path, timeout_secs: u64) -> ReplayResult {
    let timeout = Duration::from_secs(timeout_secs);
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return ReplayResult {
                command: command.to_string(),
                ran: false,
                passed: false,
                exit_code: None,
                stdout: String::new(),
                stderr: e.to_string(),
                skipped_reason: None,
            }
        }
    };

    let pid = child.id();
    let deadline = std::time::Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .unwrap_or_else(|_| std::process::Output {
                        status,
                        stdout: vec![],
                        stderr: vec![],
                    });
                return ReplayResult {
                    command: command.to_string(),
                    ran: true,
                    passed: output.status.success(),
                    exit_code: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout)
                        .trim_end()
                        .to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr)
                        .trim_end()
                        .to_string(),
                    skipped_reason: None,
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Kill process on timeout
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    std::thread::sleep(Duration::from_millis(500));
                    let _ = child.kill();
                    return ReplayResult {
                        command: command.to_string(),
                        ran: true,
                        passed: false,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: format!("timed out after {}s", timeout_secs),
                        skipped_reason: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return ReplayResult {
                    command: command.to_string(),
                    ran: false,
                    passed: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    skipped_reason: None,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    #[test]
    fn skips_when_no_test_cmds() {
        let results = run_held_out(Path::new("."), &[], 30);
        assert_eq!(results.len(), 1);
        assert!(results[0].skipped_reason.is_some());
        assert!(results[0].passed); // skip counts as non-blocking
    }

    #[test]
    fn skips_when_no_heldout_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let results = run_held_out(tmp.path(), &["echo hi".to_string()], 30);
        assert_eq!(results.len(), 1);
        assert!(results[0].skipped_reason.is_some());
    }

    #[test]
    fn runs_command_in_sandbox_with_agent_source_and_original_tests() {
        let tmp = tempfile::tempdir().unwrap();
        init_git(tmp.path());

        // Baseline: source has a bug (returns wrong value)
        fs::write(tmp.path().join("src.sh"), "#!/bin/sh\necho wrong\n").unwrap();
        // Original test: expects "correct"
        fs::create_dir_all(tmp.path().join("tests")).unwrap();
        fs::write(
            tmp.path().join("tests/check.sh"),
            "#!/bin/sh\nresult=$(sh src.sh)\n[ \"$result\" = \"correct\" ] && exit 0 || exit 1\n",
        )
        .unwrap();
        git_add_commit(tmp.path(), "baseline");

        // Snapshot held-out tests (original)
        let heldout = tmp.path().join(".heldout/heldout");
        fs::create_dir_all(heldout.join("tests")).unwrap();
        fs::copy(
            tmp.path().join("tests/check.sh"),
            heldout.join("tests/check.sh"),
        )
        .unwrap();

        // Agent "fix": changes source to return "correct" but rewrites the test to accept "wrong"
        fs::write(tmp.path().join("src.sh"), "#!/bin/sh\necho correct\n").unwrap();
        fs::write(
            tmp.path().join("tests/check.sh"),
            "#!/bin/sh\nresult=$(sh src.sh)\n[ \"$result\" = \"wrong\" ] && exit 0 || exit 1\n",
        )
        .unwrap();

        // Replay with original tests: source returns "correct" and original test expects "correct" → PASS
        let results = run_held_out(tmp.path(), &["sh tests/check.sh".to_string()], 30);
        assert_eq!(results.len(), 1);
        assert!(results[0].ran);
        assert!(
            results[0].passed,
            "held-out test should PASS against fixed source: stderr={}",
            results[0].stderr
        );
    }

    #[test]
    fn catches_agent_that_fixed_test_but_not_source() {
        let tmp = tempfile::tempdir().unwrap();
        init_git(tmp.path());

        // Baseline: source has a bug (returns wrong), test correctly tests for "correct"
        fs::write(tmp.path().join("src.sh"), "#!/bin/sh\necho wrong\n").unwrap();
        fs::create_dir_all(tmp.path().join("tests")).unwrap();
        fs::write(
            tmp.path().join("tests/check.sh"),
            "#!/bin/sh\nresult=$(sh src.sh)\n[ \"$result\" = \"correct\" ] && exit 0 || exit 1\n",
        )
        .unwrap();
        git_add_commit(tmp.path(), "baseline");

        // Snapshot: original test expects "correct"
        let heldout = tmp.path().join(".heldout/heldout");
        fs::create_dir_all(heldout.join("tests")).unwrap();
        fs::copy(
            tmp.path().join("tests/check.sh"),
            heldout.join("tests/check.sh"),
        )
        .unwrap();

        // Agent cheats: does NOT fix the source, only rewrites the test to accept "wrong"
        fs::write(
            tmp.path().join("tests/check.sh"),
            "#!/bin/sh\nresult=$(sh src.sh)\n[ \"$result\" = \"wrong\" ] && exit 0 || exit 1\n",
        )
        .unwrap();

        // Replay with original tests: source still returns "wrong", original test expects "correct" → FAIL
        let results = run_held_out(tmp.path(), &["sh tests/check.sh".to_string()], 30);
        assert_eq!(results.len(), 1);
        assert!(results[0].ran);
        assert!(
            !results[0].passed,
            "held-out test should FAIL when agent only fixed the test, not the source"
        );
    }

    fn init_git(dir: &Path) {
        run(dir, &["git", "init", "-b", "main"]);
        run(dir, &["git", "config", "user.email", "t@t.com"]);
        run(dir, &["git", "config", "user.name", "T"]);
    }

    fn git_add_commit(dir: &Path, msg: &str) {
        run(dir, &["git", "add", "."]);
        run(dir, &["git", "commit", "-m", msg]);
    }

    fn run(dir: &Path, args: &[&str]) {
        let out = StdCommand::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

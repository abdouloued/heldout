use anyhow::{Context, Result};
use git2::{DiffFormat, DiffOptions, Repository};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RepoDiff {
    pub root: PathBuf,
    pub unified_diff: String,
}

/// Capture HEAD commit SHA to record as session baseline.
pub fn capture_baseline(start: &Path) -> Result<String> {
    let repo = open(start)?;
    let head = repo
        .head()
        .context("No commits yet — make at least one commit first")?;
    let commit = head.peel_to_commit()?;
    Ok(commit.id().to_string())
}

/// Diff working tree (including staged) against a baseline commit SHA, or HEAD when None.
pub fn worktree_diff(start: &Path, baseline: Option<&str>) -> Result<RepoDiff> {
    let repo = open(start)?;
    let root = repo
        .workdir()
        .context("bare repositories are not supported")?
        .to_path_buf();

    let base_tree = match baseline {
        Some(sha) => {
            let oid =
                git2::Oid::from_str(sha).with_context(|| format!("invalid baseline SHA: {sha}"))?;
            let commit = repo
                .find_commit(oid)
                .with_context(|| format!("could not find baseline commit {sha}"))?;
            commit.tree()?
        }
        None => {
            let head = repo.head()?;
            head.peel_to_commit()?.tree()?
        }
    };

    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_unmodified(false);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut options))
        .context("read worktree diff")?;

    let mut unified_diff = Vec::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        unified_diff.extend_from_slice(line.content());
        true
    })
    .context("render unified diff")?;

    Ok(RepoDiff {
        root,
        unified_diff: String::from_utf8_lossy(&unified_diff).into_owned(),
    })
}

fn open(start: &Path) -> Result<Repository> {
    Repository::discover(start).with_context(|| {
        format!(
            "find git repository from {}. Run `git init` first.",
            start.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn renders_parseable_unified_diff_headers() {
        let temp = tempfile::tempdir().expect("tempdir");
        run_git(temp.path(), &["init", "-b", "main"]);
        fs::create_dir_all(temp.path().join("tests")).expect("tests dir");
        fs::write(
            temp.path().join("tests/cart.test.ts"),
            "test(\"cart\", () => {\n  expect(total()).toBe(3);\n});\n",
        )
        .expect("baseline test");
        run_git(temp.path(), &["add", "."]);
        run_git(
            temp.path(),
            &[
                "-c",
                "user.name=Tattle",
                "-c",
                "user.email=tattle@example.com",
                "commit",
                "-m",
                "baseline",
            ],
        );
        fs::write(
            temp.path().join("tests/cart.test.ts"),
            "test.skip(\"cart\", () => {\n  expect(total()).toBeTruthy();\n});\n",
        )
        .expect("changed test");

        let diff = worktree_diff(temp.path(), None).expect("worktree diff");

        assert!(diff
            .unified_diff
            .lines()
            .any(|line| line == "diff --git a/tests/cart.test.ts b/tests/cart.test.ts"));
        assert!(diff
            .unified_diff
            .lines()
            .any(|line| line == "--- a/tests/cart.test.ts"));
        assert!(diff
            .unified_diff
            .lines()
            .any(|line| line == "+++ b/tests/cart.test.ts"));
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

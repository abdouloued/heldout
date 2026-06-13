use crate::config::Config;
use crate::detector::{analyze_diff, CheckCode, Finding};
use crate::git;
use crate::replay::{run_held_out, ReplayResult};
use crate::session::{Session, SESSION_DIR};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const LAST_RUN_FILE: &str = ".tattle/last_run.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub verdict: Verdict,
    pub exit_code: i32,
    pub task: Option<String>,
    pub base_commit: Option<String>,
    pub agent: Option<String>,
    pub findings: Vec<Finding>,
    pub replay: Vec<ReplayResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Suspicious,
    Fail,
}

impl CheckReport {
    pub fn failed(&self) -> bool {
        self.exit_code != 0
    }
}

pub fn run_check(
    root: &Path,
    config: &Config,
    session: Option<&Session>,
    no_replay: bool,
    strict: bool,
) -> Result<CheckReport> {
    let baseline = session.map(|s| s.git_baseline.as_str());
    let diff = git::worktree_diff(root, baseline)?;
    let findings = analyze_diff(&diff.unified_diff);

    let test_cmds: Vec<String> = if let Some(s) = session {
        s.test_cmds.clone()
    } else {
        config.replay.commands.clone()
    };

    let replay = if no_replay || test_cmds.is_empty() {
        vec![]
    } else {
        run_held_out(root, &test_cmds, config.replay.timeout_secs)
    };

    let replay_failed = replay.iter().any(|r| r.ran && !r.passed);
    let has_findings = !findings.is_empty();

    let verdict = if has_findings || replay_failed {
        Verdict::Fail
    } else {
        Verdict::Pass
    };

    let exit_code = match verdict {
        Verdict::Pass => 0,
        Verdict::Suspicious if strict => 1,
        Verdict::Suspicious => 0,
        Verdict::Fail => 1,
    };

    let report = CheckReport {
        verdict,
        exit_code,
        task: session.map(|s| s.mission.clone()),
        base_commit: session.map(|s| s.git_baseline.clone()),
        agent: session.and_then(|s| s.agent.clone()),
        findings,
        replay,
    };

    // Persist last run
    let last_run_path = root.join(LAST_RUN_FILE);
    let dir = root.join(SESSION_DIR);
    fs::create_dir_all(&dir)?;
    if let Ok(json) = serde_json::to_string_pretty(&report) {
        fs::write(&last_run_path, json).ok();
    }

    Ok(report)
}

pub fn print_last(root: &Path, markdown: bool) -> Result<()> {
    let path = root.join(LAST_RUN_FILE);
    let text = fs::read_to_string(&path)
        .with_context(|| "No previous run found. Run `tattle check` first.")?;
    let report: CheckReport = serde_json::from_str(&text).context("parse last_run.json")?;
    if markdown {
        println!("{}", render_markdown(&report));
    } else {
        print_terminal(&report);
    }
    Ok(())
}

pub fn print_terminal(report: &CheckReport) {
    let verdict_label = match report.verdict {
        Verdict::Pass => "PASS",
        Verdict::Suspicious => "SUSPICIOUS",
        Verdict::Fail => "FAIL",
    };

    println!();
    println!(
        "tattle  task: {}   base: {}",
        report.task.as_deref().unwrap_or("<none>"),
        report
            .base_commit
            .as_deref()
            .map(|s| &s[..7.min(s.len())])
            .unwrap_or("<none>"),
    );
    println!();

    if report.findings.is_empty() && report.replay.iter().all(|r| !r.ran || r.passed) {
        println!("  VERDICT: {verdict_label} — no integrity problems found");
    } else {
        println!("  CLAIMED:  done, tests pass");
        println!("  REALITY:");
        for f in &report.findings {
            let line = f.line.map(|l| format!(":{l}")).unwrap_or_default();
            let sev = check_severity(f.code);
            println!(
                "    {sev:<8} {}  {}{}  — {}",
                code_label(f.code),
                f.path,
                line,
                f.message
            );
        }

        let replay_ran: Vec<_> = report.replay.iter().filter(|r| r.ran).collect();
        if !replay_ran.is_empty() {
            let passed = replay_ran.iter().filter(|r| r.passed).count();
            let failed = replay_ran.len() - passed;
            println!();
            println!(
                "  HELD-OUT REPLAY (original suite vs new code):  {failed} FAILED, {passed} passed"
            );
            for r in replay_ran.iter().filter(|r| !r.passed) {
                println!("    ✗  {}", r.command);
                if !r.stderr.is_empty() {
                    for line in r.stderr.lines().take(5) {
                        println!("       {line}");
                    }
                }
            }
        }

        for r in report.replay.iter().filter(|r| r.skipped_reason.is_some()) {
            println!(
                "  REPLAY SKIPPED: {}",
                r.skipped_reason.as_deref().unwrap_or("")
            );
        }

        println!();
        let detail = match report.verdict {
            Verdict::Pass => "no integrity problems",
            Verdict::Suspicious => "completion claim is suspicious",
            Verdict::Fail => "completion claim not supported by evidence",
        };
        println!("  VERDICT: {verdict_label} — {detail}");
    }
    println!();
}

pub fn render_markdown(report: &CheckReport) -> String {
    let verdict_label = match report.verdict {
        Verdict::Pass => "✅ PASS",
        Verdict::Suspicious => "⚠️ SUSPICIOUS",
        Verdict::Fail => "❌ FAIL",
    };

    let mut out = String::new();
    let task = report.task.as_deref().unwrap_or("<none>");
    let base = report
        .base_commit
        .as_deref()
        .map(|s| &s[..7.min(s.len())])
        .unwrap_or("<none>");

    out.push_str(&format!("## tattle integrity report — {verdict_label}\n\n"));
    out.push_str(&format!(
        "**task:** {task}  \n**base commit:** `{base}`\n\n"
    ));

    if report.findings.is_empty() {
        out.push_str("No deterministic findings.\n\n");
    } else {
        out.push_str("### Findings\n\n| Code | Severity | File | Line | Detail |\n|------|----------|------|------|--------|\n");
        for f in &report.findings {
            let line = f.line.map(|l| l.to_string()).unwrap_or_default();
            let sev = check_severity(f.code);
            out.push_str(&format!(
                "| {} | {} | `{}` | {} | {} |\n",
                code_label(f.code),
                sev,
                f.path,
                line,
                f.message
            ));
        }
        out.push('\n');
    }

    let replay_ran: Vec<_> = report.replay.iter().filter(|r| r.ran).collect();
    if !replay_ran.is_empty() {
        let passed = replay_ran.iter().filter(|r| r.passed).count();
        let failed = replay_ran.len() - passed;
        out.push_str("### Held-out replay\n\n");
        out.push_str(&format!("**{failed} FAILED, {passed} passed**\n\n"));
        for r in replay_ran.iter().filter(|r| !r.passed) {
            out.push_str(&format!("- ✗ `{}`\n", r.command));
            if !r.stderr.is_empty() {
                out.push_str(&format!(
                    "  ```\n{}\n  ```\n",
                    &r.stderr[..r.stderr.len().min(500)]
                ));
            }
        }
    }

    out
}

fn check_severity(code: CheckCode) -> &'static str {
    match code {
        CheckCode::C1DeletedTest => "FAIL",
        CheckCode::C2SkippedTest => "FAIL",
        CheckCode::C3RemovedAssertion => "FAIL",
        CheckCode::C4WeakenedAssertion => "FAIL",
        CheckCode::C5VacuousAssertion => "FAIL",
        CheckCode::C6WeakenedTestCommand => "WARN",
    }
}

fn code_label(code: CheckCode) -> &'static str {
    match code {
        CheckCode::C1DeletedTest => "C1",
        CheckCode::C2SkippedTest => "C2",
        CheckCode::C3RemovedAssertion => "C3",
        CheckCode::C4WeakenedAssertion => "C4",
        CheckCode::C5VacuousAssertion => "C5",
        CheckCode::C6WeakenedTestCommand => "C6",
    }
}

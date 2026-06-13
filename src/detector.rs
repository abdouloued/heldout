use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub code: CheckCode,
    pub path: String,
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckCode {
    C1DeletedTest,
    C2SkippedTest,
    C3RemovedAssertion,
    C4WeakenedAssertion,
    C5VacuousAssertion,
    C6WeakenedTestCommand,
}

pub fn analyze_diff(diff: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current_path = String::new();
    let mut deleted_file = false;
    let mut old_line: u32 = 0;
    let mut new_line: u32 = 0;
    let mut pending_removed_assertion: Option<(String, u32)> = None;
    let mut pending_removed_test_command: Option<(String, u32)> = None;

    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("diff --git ") {
            flush_removed_assertion(&mut findings, &mut pending_removed_assertion, &current_path);
            current_path = parse_diff_path(path);
            deleted_file = false;
            old_line = 0;
            new_line = 0;
            pending_removed_test_command = None;
            continue;
        }

        if line.starts_with("deleted file mode ") {
            deleted_file = true;
            if is_test_path(&current_path) {
                findings.push(finding(
                    CheckCode::C1DeletedTest,
                    &current_path,
                    None,
                    "deleted test file",
                ));
            }
            continue;
        }

        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            flush_removed_assertion(&mut findings, &mut pending_removed_assertion, &current_path);
            old_line = old_start;
            new_line = new_start;
            continue;
        }

        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }

        if let Some(removed) = line.strip_prefix('-') {
            let trimmed = removed.trim();
            if !deleted_file && is_assertion(trimmed) && !is_comment(trimmed) {
                pending_removed_assertion = Some((trimmed.to_string(), old_line));
            }
            if is_test_command_path(&current_path) && looks_like_test_command(trimmed) {
                pending_removed_test_command = Some((trimmed.to_string(), old_line));
            }
            old_line = old_line.saturating_add(1);
            continue;
        }

        if let Some(added) = line.strip_prefix('+') {
            let trimmed = added.trim();

            if is_assertion(trimmed) {
                if let Some((removed_assertion, _removed_line)) = pending_removed_assertion.take() {
                    if is_weaker_assertion(&removed_assertion, trimmed) {
                        findings.push(finding(
                            CheckCode::C4WeakenedAssertion,
                            &current_path,
                            Some(new_line),
                            "replaced a specific assertion with a weaker assertion",
                        ));
                    }
                }
            }

            if is_skip_marker(trimmed) && !is_comment(trimmed) {
                findings.push(finding(
                    CheckCode::C2SkippedTest,
                    &current_path,
                    Some(new_line),
                    "added test skip marker",
                ));
            }

            if is_assertion(trimmed) && is_vacuous_assertion(trimmed) {
                findings.push(finding(
                    CheckCode::C5VacuousAssertion,
                    &current_path,
                    Some(new_line),
                    "added assertion that is always true",
                ));
            }

            if let Some((removed_command, removed_line)) = pending_removed_test_command.take() {
                if weakens_test_command(&removed_command, trimmed) {
                    findings.push(finding(
                        CheckCode::C6WeakenedTestCommand,
                        &current_path,
                        Some(new_line),
                        "weakened test command",
                    ));
                } else if !looks_like_test_command(trimmed) {
                    pending_removed_test_command = Some((removed_command, removed_line));
                }
            }

            new_line = new_line.saturating_add(1);
            continue;
        }

        flush_removed_assertion(&mut findings, &mut pending_removed_assertion, &current_path);
        if !line.starts_with('\\') && !line.is_empty() {
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
        }
    }

    flush_removed_assertion(&mut findings, &mut pending_removed_assertion, &current_path);

    findings
}

fn finding(code: CheckCode, path: &str, line: Option<u32>, message: &str) -> Finding {
    Finding {
        code,
        path: path.to_string(),
        line,
        message: message.to_string(),
    }
}

fn flush_removed_assertion(
    findings: &mut Vec<Finding>,
    pending: &mut Option<(String, u32)>,
    path: &str,
) {
    if let Some((_, line)) = pending.take() {
        findings.push(finding(
            CheckCode::C3RemovedAssertion,
            path,
            Some(line),
            "removed assertion from test",
        ));
    }
}

fn parse_diff_path(raw: &str) -> String {
    raw.split_whitespace()
        .nth(1)
        .or_else(|| raw.split_whitespace().next())
        .unwrap_or_default()
        .trim_start_matches("b/")
        .trim_start_matches("a/")
        .to_string()
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let mut parts = line.split_whitespace();
    parts.next();
    let old_part = parts.next()?.trim_start_matches('-');
    let new_part = parts.next()?.trim_start_matches('+');
    Some((parse_start(old_part), parse_start(new_part)))
}

fn parse_start(part: &str) -> u32 {
    part.split(',')
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.contains("test/")
        || lower.contains("__tests__")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("test.py")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with("test.java")
}

fn is_test_command_path(path: &str) -> bool {
    matches!(
        path,
        "package.json" | "Cargo.toml" | "Makefile" | "makefile"
    ) || path.starts_with(".github/workflows/")
        || path.ends_with(".yml")
        || path.ends_with(".yaml")
}

fn is_comment(line: &str) -> bool {
    line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with("/*")
        || line.starts_with('*')
}

fn is_skip_marker(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("test.skip(")
        || lower.contains("describe.skip(")
        || lower.contains("it.skip(")
        || lower.starts_with("xit(")
        || lower.starts_with("xtest(")
        || lower.contains("pytest.mark.skip")
        || lower.contains("@disabled")
        || lower == "#[ignore]"
        || lower.starts_with("#[ignore")
}

fn is_assertion(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("expect(")
        || lower.contains("assert(")
        || lower.contains("assert!(")
        || lower.contains("assert_eq!(")
        || lower.contains("assert_ne!(")
        || lower.contains("assert.")
        || lower.contains("assert_")
        || lower.contains("self.assert")
}

fn is_vacuous_assertion(line: &str) -> bool {
    let compact = line
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("expect(true).tobe(true)")
        || compact.contains("expect(1).tobe(1)")
        || compact.contains("assert!(true)")
        || compact.contains("assert(true)")
        || compact.contains("assert.equal(1,1)")
        || compact.contains("assert_eq!(1,1)")
        || compact.contains("self.assertequal(1,1)")
}

fn is_weaker_assertion(removed: &str, added: &str) -> bool {
    let removed = removed.to_ascii_lowercase();
    let added = added.to_ascii_lowercase();
    let weak_added = added.contains("tobetruthy")
        || added.contains("tobefined")
        || added.contains("toexist")
        || added.contains(">= 200")
        || added.contains("=> 200")
        || added.contains("assert!(") && (added.contains(">=") || added.contains("<="));

    weak_added && removed.len() > added.len()
}

fn looks_like_test_command(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("test")
        || lower.contains("vitest")
        || lower.contains("jest")
        || lower.contains("pytest")
        || lower.contains("cargo test")
}

fn weakens_test_command(removed: &str, added: &str) -> bool {
    let removed = removed.to_ascii_lowercase();
    let added = added.to_ascii_lowercase();
    if !looks_like_test_command(&added) {
        return false;
    }
    added.contains("passwithnotests")
        || added.contains("--allow-no-tests")
        || added.contains("--no-fail")
        || added.contains("|| true")
        || added.contains("--lib ignored")
        || removed.contains("cargo test --all") && added.contains("cargo test --lib")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(diff: &str) -> Vec<CheckCode> {
        analyze_diff(diff).into_iter().map(|f| f.code).collect()
    }

    #[test]
    fn c1_flags_deleted_test_file() {
        let diff = r#"diff --git a/tests/math.test.ts b/tests/math.test.ts
deleted file mode 100644
index e69de29..0000000
--- a/tests/math.test.ts
+++ /dev/null
@@ -1,4 +0,0 @@
-test("adds", () => {
-  expect(add(1, 2)).toBe(3);
-});
"#;

        assert_eq!(codes(diff), vec![CheckCode::C1DeletedTest]);
    }

    #[test]
    fn c1_ignores_deleted_non_test_file() {
        let diff = r#"diff --git a/src/notes.ts b/src/notes.ts
deleted file mode 100644
--- a/src/notes.ts
+++ /dev/null
@@ -1 +0,0 @@
-export const note = "ok";
"#;

        assert!(codes(diff).is_empty());
    }

    #[test]
    fn c2_flags_skipped_tests_across_languages() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,2 +1,4 @@
+test.skip("works", () => {});
+xit("old behavior", () => {});
diff --git a/tests/test_app.py b/tests/test_app.py
@@ -1,2 +1,3 @@
+@pytest.mark.skip(reason="flaky")
 def test_app(): pass
diff --git a/src/AppTest.java b/src/AppTest.java
@@ -1,2 +1,3 @@
+@Disabled("temporarily")
 void testApp() {}
diff --git a/src/lib.rs b/src/lib.rs
@@ -1,2 +1,3 @@
+#[ignore]
 fn test_app() {}
"#;

        assert_eq!(
            codes(diff),
            vec![
                CheckCode::C2SkippedTest,
                CheckCode::C2SkippedTest,
                CheckCode::C2SkippedTest,
                CheckCode::C2SkippedTest
            ]
        );
    }

    #[test]
    fn c2_ignores_comments_about_skip() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,2 +1,3 @@
+// do not use test.skip here
 test("works", () => {});
"#;

        assert!(codes(diff).is_empty());
    }

    #[test]
    fn c3_flags_removed_assertions() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,4 +1,3 @@
 test("works", () => {
-  expect(result).toEqual({ ok: true });
 });
"#;

        assert_eq!(codes(diff), vec![CheckCode::C3RemovedAssertion]);
    }

    #[test]
    fn c3_ignores_removed_assertion_comments() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,3 +1,2 @@
-// expect(result).toEqual({ ok: true });
 test("works", () => {});
"#;

        assert!(codes(diff).is_empty());
    }

    #[test]
    fn c4_flags_assertion_weakening_replacements() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,4 +1,4 @@
-expect(user).toEqual({ id: "u1", role: "admin" });
+expect(user).toBeTruthy();
@@ -8,4 +8,4 @@
-assert_eq!(status, 201);
+assert!(status >= 200);
"#;

        assert_eq!(
            codes(diff),
            vec![
                CheckCode::C4WeakenedAssertion,
                CheckCode::C4WeakenedAssertion
            ]
        );
    }

    #[test]
    fn c4_flags_weakened_assertion_after_added_skip_marker() {
        let diff = r#"diff --git a/tests/cart.test.ts b/tests/cart.test.ts
@@ -1,3 +1,3 @@
-test("cart total", () => {
-  expect(total()).toEqual({ cents: 1299, currency: "USD" });
+test.skip("cart total", () => {
+  expect(total()).toBeTruthy();
 });
"#;

        assert_eq!(
            codes(diff),
            vec![CheckCode::C2SkippedTest, CheckCode::C4WeakenedAssertion]
        );
    }

    #[test]
    fn c4_ignores_equivalent_assertion_rewrite() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,4 +1,4 @@
-expect(total).toBe(3);
+expect(total).toEqual(3);
"#;

        assert!(codes(diff).is_empty());
    }

    #[test]
    fn c5_flags_vacuous_assertions() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,3 +1,6 @@
+expect(true).toBe(true);
+assert!(true);
+assert.equal(1, 1);
"#;

        assert_eq!(
            codes(diff),
            vec![
                CheckCode::C5VacuousAssertion,
                CheckCode::C5VacuousAssertion,
                CheckCode::C5VacuousAssertion
            ]
        );
    }

    #[test]
    fn c5_ignores_real_assertions() {
        let diff = r#"diff --git a/tests/app.test.ts b/tests/app.test.ts
@@ -1,3 +1,4 @@
+expect(result.ok).toBe(true);
"#;

        assert!(codes(diff).is_empty());
    }

    #[test]
    fn c6_flags_weakened_test_commands() {
        let diff = r#"diff --git a/package.json b/package.json
@@ -3,7 +3,7 @@
-    "test": "vitest run"
+    "test": "vitest run --passWithNoTests"
diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml
@@ -8,7 +8,7 @@
-      run: cargo test --all
+      run: cargo test --lib ignored_case
"#;

        assert_eq!(
            codes(diff),
            vec![
                CheckCode::C6WeakenedTestCommand,
                CheckCode::C6WeakenedTestCommand
            ]
        );
    }

    #[test]
    fn c6_ignores_stronger_test_commands() {
        let diff = r#"diff --git a/package.json b/package.json
@@ -3,7 +3,7 @@
-    "test": "vitest run"
+    "test": "vitest run --coverage"
"#;

        assert!(codes(diff).is_empty());
    }
}

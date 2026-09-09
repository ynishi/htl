//! `htl test --junit <file>`: the run as a JUnit XML report, for a CI that reads test
//! results rather than a log.
//!
//! **Which dialect.** JUnit XML has no specification — only implementations that agree
//! approximately. This targets the Ant/`surefire` subset that the common consumers accept
//! unchanged: a `<testsuites>` root over one `<testsuite>` per file, each `<testcase>`
//! carrying `classname` (the file), `name` (the suite and test name as the text output
//! composes them) and `time` in seconds, and a failing case carrying one `<failure>`.
//! Both levels carry `tests` / `failures` / `errors` / `skipped` / `time`. That is what
//! Jenkins' JUnit plugin, GitLab's report ingestion, and the GitHub Actions reporters
//! (`dorny/test-reporter`, `mikepenz/action-junit-report`) all read.
//!
//! Deliberately left out, because the consumers differ on them and nothing here needs
//! them: `<properties>` (so the run's seed lives in the text summary, not the report),
//! `<system-out>` / `<system-err>`, `timestamp` / `hostname` / `id` / `package`
//! attributes, the `file` and `line` attributes `cargo-nextest` adds, `<rerunFailure>`,
//! and `<skipped>` — see below.
//!
//! **What is skipped.** Nothing, and no `<skipped>` element is ever written. A JUnit skip
//! means the runner considered a test and declined to run it; `--filter` is a selection
//! made before the run, so an excluded test is not a result and is simply absent, as are
//! the files `--fail-fast` never reached. A file with no tests is a different thing: it
//! ran, and its verdict is that it ran to completion, so it is in the report — as a suite
//! with no cases, since the summary line does not count it as a test either.
//!
//! **What an `<error>` is.** A file that failed to type-check, or raised outside any test,
//! is a suite whose cases could not run: it carries an `<error>` and no cases, which is
//! what distinguishes it from an assertion `<failure>` inside a case. Consumers that walk
//! only `<testcase>` elements will see the suite's `errors="1"` count but not the message;
//! the alternative — inventing a case that no test corresponds to — would put the report's
//! totals out of step with the summary line, which the report exists to agree with.

use std::fmt::Write as _;

/// One test file's outcome.
pub struct Suite {
    /// The file, as the text output names it. Also every case's `classname`.
    pub file: String,
    pub duration_ms: f64,
    /// The check failed or the file raised: the cases could not run.
    pub error: Option<Error>,
    pub cases: Vec<Case>,
}

/// A suite-level `<error>`: what went wrong, and what kind of wrong it is.
pub struct Error {
    /// The one-line detail the text output prints on the file's line.
    pub message: String,
    /// `check` (the file did not type-check) or `error` (it raised).
    pub kind: &'static str,
    /// The full text: the diagnostics, or the raised error with its traceback.
    pub body: String,
}

/// One test's outcome.
pub struct Case {
    /// The suite and test name as the assertion library composes them.
    pub name: String,
    pub ms: f64,
    /// The message the text output prints under the file, with its name prefix removed.
    pub failure: Option<String>,
}

/// Seconds, as JUnit spells a duration.
fn secs(ms: f64) -> String {
    format!("{:.3}", ms / 1000.0)
}

/// XML 1.0 forbids most control characters outright — there is no escape for them — so a
/// message that carries one is written with it replaced rather than made unparseable.
fn sanitize(c: char) -> Option<char> {
    match c {
        '\t' | '\n' | '\r' => Some(c),
        c if (c as u32) < 0x20 => Some('\u{fffd}'),
        c => Some(c),
    }
}

/// For element text: the three that would otherwise be markup.
pub fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter_map(sanitize) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// For attribute values: the same, plus both quotes and the whitespace an XML parser is
/// required to fold into spaces — a traceback in a `message=` attribute keeps its lines
/// only if they are written as character references.
pub fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter_map(sanitize) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

/// The headline of a message: what goes in `message=`, with the whole of it in the body.
fn headline(s: &str) -> &str {
    s.lines().next().unwrap_or_default()
}

/// The report for a run: the suites in the order they ran, and the run's own total, which
/// is the summary line's and so is at least the sum of the suites.
pub fn document(suites: &[Suite], run_ms: f64) -> String {
    let tests: usize = suites.iter().map(|s| s.cases.len()).sum();
    let failures: usize = suites
        .iter()
        .map(|s| s.cases.iter().filter(|c| c.failure.is_some()).count())
        .sum();
    let errors = suites.iter().filter(|s| s.error.is_some()).count();

    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        out,
        "<testsuites name=\"htl test\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"0\" time=\"{}\">",
        secs(run_ms)
    );
    for s in suites {
        let s_failures = s.cases.iter().filter(|c| c.failure.is_some()).count();
        let _ = writeln!(
            out,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{s_failures}\" errors=\"{}\" skipped=\"0\" time=\"{}\">",
            attr(&s.file),
            s.cases.len(),
            usize::from(s.error.is_some()),
            secs(s.duration_ms)
        );
        if let Some(e) = &s.error {
            let _ = writeln!(
                out,
                "    <error message=\"{}\" type=\"{}\">{}</error>",
                attr(headline(&e.message)),
                e.kind,
                text(&e.body)
            );
        }
        for c in &s.cases {
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"{}\"",
                attr(&s.file),
                attr(&c.name),
                secs(c.ms)
            );
            match &c.failure {
                None => {
                    let _ = writeln!(out, "{head}/>");
                }
                Some(m) => {
                    let _ = writeln!(out, "{head}>");
                    let _ = writeln!(
                        out,
                        "      <failure message=\"{}\" type=\"failure\">{}</failure>",
                        attr(headline(m)),
                        text(m)
                    );
                    let _ = writeln!(out, "    </testcase>");
                }
            }
        }
        let _ = writeln!(out, "  </testsuite>");
    }
    out.push_str("</testsuites>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_in_a_message_survives_as_data() {
        let doc = document(
            &[Suite {
                file: "tests/a_test.tl".into(),
                duration_ms: 12.0,
                error: None,
                cases: vec![Case {
                    name: "a > b".into(),
                    ms: 1.0,
                    failure: Some("expected <p> & \"q\" got 'r'\nstack traceback:\n\tin fn".into()),
                }],
            }],
            13.0,
        );
        assert!(
            doc.contains("message=\"expected &lt;p&gt; &amp; &quot;q&quot; got &apos;r&apos;\"")
        );
        assert!(doc.contains("expected &lt;p&gt; &amp; \"q\" got 'r'\nstack traceback:"));
        assert!(doc.contains("tests=\"1\" failures=\"1\" errors=\"0\""));
    }

    #[test]
    fn a_newline_in_an_attribute_is_a_character_reference() {
        assert_eq!(attr("a\nb"), "a&#10;b");
        assert_eq!(text("a\nb"), "a\nb");
    }
}

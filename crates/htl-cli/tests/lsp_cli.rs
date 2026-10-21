//! `htl lsp` through the real binary: the process is spawned, spoken to in JSON-RPC over
//! its stdin and stdout, and what it publishes is held against `htl check --format json` on
//! the same file. The server's claim is that its diagnostics *are* `htl check`'s, so every
//! case that has an answer compares with the command rather than restating it.
//!
//! No case waits on a clock for something to happen. A case that expects a publish blocks
//! on the pipe with a timeout that only exists so a hang fails the test (it is never what
//! the test is measuring), and what a case does not expect is found at the end: `shutdown`
//! is answered after every message before it has been, so any publish the case did not read
//! is in front of the answer.

use lsp_types::notification::{DidChangeWatchedFiles, Notification as _};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-cli-lsp", name)
}

/// Long enough that a loaded machine never trips it, short enough that a hang is a failure
/// of this test and not of the job running it.
const TIMEOUT: Duration = Duration::from_secs(60);

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `htl new sample` in a fresh directory: `src/main.tl` requires `src/sample/init.tl`. The
/// default target has no Rust host, so no cargo runs behind a check.
fn project(name: &str) -> (common::TempDir, PathBuf) {
    let root = tempdir(name);
    let out = Command::new(common::htl_bin())
        .args(["new", "sample"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let project = root.join("sample");
    (root, project)
}

fn uri(path: &Path) -> String {
    // The project lives in a plain temp path, so no byte of it needs encoding.
    format!("file://{}", path.display())
}

/// One diagnostic, reduced to what the two sides share: the rule, the 0-based line and
/// column, the message.
type Shared = (String, u64, u64, String);

/// What `htl check --format json` reports for `file` (relative to `cwd`).
fn checked(cwd: &Path, file: &str) -> Vec<Shared> {
    let out = Command::new(common::htl_bin())
        .args(["check", file, "--format", "json", "--no-cache"])
        .current_dir(cwd)
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}): {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let mut found: Vec<Shared> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["file"] == file)
        .map(|d| {
            (
                d["rule"].as_str().unwrap_or_default().to_string(),
                d["line"].as_u64().unwrap() - 1,
                d["col"].as_u64().unwrap() - 1,
                d["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    found.sort();
    found
}

/// What a `publishDiagnostics` carries, reduced the same way. The files these cases use are
/// ASCII, so a byte column and a UTF-16 column are the same number.
fn published(diagnostics: &[Value]) -> Vec<Shared> {
    let mut found: Vec<Shared> = diagnostics
        .iter()
        .map(|d| {
            assert_eq!(d["source"], "htl", "{d}");
            (
                d["code"].as_str().unwrap_or_default().to_string(),
                d["range"]["start"]["line"].as_u64().unwrap(),
                d["range"]["start"]["character"].as_u64().unwrap(),
                d["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    found.sort();
    found
}

/// A running `htl lsp` and the client's end of its pipes.
struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    stderr: PathBuf,
    next_id: u64,
    /// Notifications read while waiting for something else, in the order they came.
    backlog: Vec<Value>,
    /// Every request the server sent, in the order they came; a request is answered with
    /// [`Client::registration`] or not at all.
    server_requests: Vec<Value>,
}

impl Client {
    /// Spawn `htl lsp` in `cwd` and do the handshake; the `initialize` result comes back
    /// beside the client.
    fn start(cwd: &Path, scratch: &Path) -> (Self, Value) {
        Self::start_with(cwd, scratch, json!({}))
    }

    /// [`Client::start`] for a client that can take a watcher registration at run time.
    fn start_watching(cwd: &Path, scratch: &Path) -> (Self, Value) {
        Self::start_with(
            cwd,
            scratch,
            json!({ "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
        )
    }

    fn start_with(cwd: &Path, scratch: &Path, capabilities: Value) -> (Self, Value) {
        let stderr = scratch.join("htl-lsp.stderr");
        let mut child = Command::new(common::htl_bin())
            .arg("lsp")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length: ") {
                        length = Some(n.parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; length.expect("a Content-Length header")];
                if r.read_exact(&mut body).is_err() {
                    return;
                }
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            messages,
            stderr,
            next_id: 0,
            backlog: Vec::new(),
            server_requests: Vec::new(),
        };
        let init = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": uri(cwd), "capabilities": capabilities }),
        );
        client.notify("initialized", json!({}));
        (client, init)
    }

    fn send(&mut self, message: Value) {
        let mut message = message;
        message["jsonrpc"] = json!("2.0");
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "method": method, "params": params }));
    }

    /// The whole response, an error included, to a request.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "id": id, "method": method, "params": params }));
        loop {
            let m = self.read();
            if m["id"] == id && m.get("method").is_none() {
                return m;
            }
            self.backlog.push(m);
        }
    }

    fn read(&mut self) -> Value {
        match self.messages.recv_timeout(TIMEOUT) {
            Ok(m) => {
                if m.get("id").is_some() && m.get("method").is_some() {
                    self.server_requests.push(m.clone());
                }
                m
            }
            Err(e) => panic!(
                "no message from htl lsp ({e}); its stderr:\n{}",
                std::fs::read_to_string(&self.stderr).unwrap_or_default()
            ),
        }
    }

    /// The `client/registerCapability` request the server sends after the handshake, answered
    /// as a client that took it would.
    fn registration(&mut self) -> Value {
        let request = loop {
            if let Some(i) = self
                .server_requests
                .iter()
                .position(|r| r["method"] == "client/registerCapability")
            {
                break self.server_requests.remove(i);
            }
            let m = self.read();
            if m["method"] != "client/registerCapability" {
                self.backlog.push(m);
            }
        };
        self.send(json!({ "id": request["id"], "result": null }));
        request
    }

    /// The watched-files notification: each of `files` changed (`2`), created (`1`) or
    /// deleted (`3`).
    fn watched(&mut self, changes: &[(&Path, u8)]) {
        let changes: Vec<Value> = changes
            .iter()
            .map(|(f, t)| json!({ "uri": uri(f), "type": t }))
            .collect();
        self.notify(DidChangeWatchedFiles::METHOD, json!({ "changes": changes }));
    }

    fn open(&mut self, file: &Path) {
        let text = std::fs::read_to_string(file).unwrap();
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(file), "languageId": "teal", "version": 1, "text": text
            } }),
        );
    }

    /// A full-sync `didChange`: the client's buffer is now `text`, saved or not.
    fn change(&mut self, file: &Path, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri(file), "version": 2 },
                    "contentChanges": [{ "text": text }] }),
        );
    }

    fn save(&mut self, file: &Path) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri(file) } }),
        );
    }

    fn close(&mut self, file: &Path) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri(file) } }),
        );
    }

    /// The next `publishDiagnostics` for each of `files`, one per file: read until every
    /// one has been seen. A second for a file before the rest have come is a failure, so
    /// the case that expects one publish per file finds out when it gets two.
    fn publishes(&mut self, files: &[&Path]) -> HashMap<PathBuf, Vec<Value>> {
        let mut got: HashMap<PathBuf, Vec<Value>> = HashMap::new();
        let started = Instant::now();
        while got.len() < files.len() {
            assert!(started.elapsed() < TIMEOUT, "still waiting for {files:?}");
            let m = if self.backlog.is_empty() {
                self.read()
            } else {
                self.backlog.remove(0)
            };
            if m["method"] != "textDocument/publishDiagnostics" {
                continue;
            }
            let at = m["params"]["uri"].as_str().unwrap().to_string();
            let file = files
                .iter()
                .find(|f| uri(f) == at)
                .unwrap_or_else(|| panic!("a publish for {at}, which no one waits for"));
            let diagnostics = m["params"]["diagnostics"].as_array().unwrap().clone();
            let was = got.insert(file.to_path_buf(), diagnostics);
            assert!(was.is_none(), "{at} was published twice");
        }
        got
    }

    /// `shutdown`, then `exit`; the publishes that nobody read, and the exit status.
    fn finish(mut self) -> (Vec<Value>, Option<i32>) {
        let response = self.request("shutdown", Value::Null);
        assert_eq!(response["result"], Value::Null, "{response}");
        assert!(response.get("error").is_none(), "{response}");
        self.notify("exit", Value::Null);
        let started = Instant::now();
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            assert!(started.elapsed() < TIMEOUT, "htl lsp did not exit");
            std::thread::sleep(Duration::from_millis(10));
        };
        let unread = std::mem::take(&mut self.backlog)
            .into_iter()
            .filter(|m| m["method"] == "textDocument/publishDiagnostics")
            .collect();
        (unread, status.code())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn help_says_what_it_is() {
    let out = Command::new(common::htl_bin())
        .args(["lsp", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("language server"), "{help}");
    assert!(help.contains("htl check"), "{help}");
}

#[test]
fn the_handshake_asks_for_open_close_and_save_and_nothing_else() {
    let (root, _) = project("handshake");
    let (mut client, init) = Client::start(&root, &root);
    let sync = &init["result"]["capabilities"]["textDocumentSync"];
    assert_eq!(sync["openClose"], true, "{init}");
    assert_eq!(sync["change"], 1, "full text on every change: {init}");
    assert_eq!(sync["save"]["includeText"], false, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "htl", "{init}");

    // A request the server does not answer is refused as such, and the server goes on.
    let r = client.request("textDocument/completion", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    // A cancellation is accepted: it is a notification, and gets no answer to be wrong.
    client.notify("$/cancelRequest", json!({ "id": 1 }));
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

#[test]
fn exit_without_shutdown_is_a_failure() {
    let (root, _) = project("no-shutdown");
    let (mut client, _) = Client::start(&root, &root);
    client.notify("exit", Value::Null);
    let started = Instant::now();
    let status = loop {
        if let Some(s) = client.child.try_wait().unwrap() {
            break s;
        }
        assert!(started.elapsed() < TIMEOUT, "htl lsp did not exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(1));
}

/// Acceptance 1 and 2: a type error is published as `htl check` reports it, and fixing the
/// file and saving it publishes nothing.
#[test]
fn a_type_error_is_published_as_check_reports_it_and_a_fix_clears_it() {
    let (root, p) = project("type-error");
    let main = p.join("src/main.tl");
    write(
        &main,
        "local sample = require(\"sample\")\n\nlocal n: number = \"x\"\nprint(sample.greet(\"a\").text, n)\n",
    );
    // Started from the directory above the project: the server is not told where the project
    // is by where it runs, and a report spells a path relative to that directory.
    let (mut client, _) = Client::start(&root, &root);
    client.open(&main);
    let got = client.publishes(&[&main]);
    let expected = checked(&p, "src/main.tl");
    assert_eq!(expected.len(), 1, "the case has one error: {expected:?}");
    assert_eq!(expected[0].1, 2, "on the line of the declaration");
    assert_eq!(published(&got[&main]), expected);
    let d = &got[&main][0];
    assert_eq!(d["severity"], 1, "an error: {d}");
    // The range starts where check says, and covers something.
    assert!(
        d["range"]["end"]["character"].as_u64().unwrap()
            > d["range"]["start"]["character"].as_u64().unwrap(),
        "{d}"
    );

    write(
        &main,
        "local sample = require(\"sample\")\n\nlocal n: number = 1\nprint(sample.greet(\"a\").text, n)\n",
    );
    client.save(&main);
    let got = client.publishes(&[&main]);
    assert_eq!(got[&main], Vec::<Value>::new());
    assert!(checked(&p, "src/main.tl").is_empty());

    // Closing a file clears it once more, and a file that is not open is not published.
    client.close(&main);
    let got = client.publishes(&[&main]);
    assert_eq!(got[&main], Vec::<Value>::new());
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// Acceptance 2: a lint is reported with its rule as `code`, and an allow comment on the
/// line removes it, as it does for `htl check`.
#[test]
fn an_allow_comment_silences_a_lint_as_it_does_for_check() {
    let (root, p) = project("allow");
    let file = p.join("src/g.tl");
    write(&file, "global counter: number = 1\nreturn counter\n");
    let (mut client, _) = Client::start(&p, &root);
    client.open(&file);
    let got = client.publishes(&[&file]);
    let expected = checked(&p, "src/g.tl");
    assert_eq!(expected.len(), 1, "{expected:?}");
    assert_eq!(expected[0].0, "no-global");
    assert_eq!(published(&got[&file]), expected);
    // A lint is advice: a warning, not an error.
    assert_eq!(got[&file][0]["severity"], 2, "{}", got[&file][0]);

    write(
        &file,
        "global counter: number = 1  -- htl: allow(no-global)\nreturn counter\n",
    );
    client.save(&file);
    let got = client.publishes(&[&file]);
    assert_eq!(got[&file], Vec::<Value>::new());
    assert!(checked(&p, "src/g.tl").is_empty());
    let (unread, _) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
}

/// Acceptance 3: a field renamed in a required module is an error in the file that uses it,
/// without that file being saved, and the rename undone clears it.
#[test]
fn saving_a_required_module_rechecks_the_open_file_that_requires_it() {
    let (root, p) = project("dependents");
    let main = p.join("src/main.tl");
    let sample = p.join("src/sample/init.tl");
    let (mut client, _) = Client::start(&p, &root);
    client.open(&main);
    let got = client.publishes(&[&main]);
    assert!(got[&main].is_empty(), "{got:?}");
    // Opening a file the open one requires re-checks the one that requires it.
    client.open(&sample);
    let got = client.publishes(&[&main, &sample]);
    assert!(got[&main].is_empty() && got[&sample].is_empty(), "{got:?}");

    let original = std::fs::read_to_string(&sample).unwrap();
    // `Greeting.text` becomes `message`; `src/main.tl` still reads `.text`.
    write(&sample, &original.replace("text", "message"));
    client.save(&sample);
    let got = client.publishes(&[&main, &sample]);
    assert!(got[&sample].is_empty(), "{:?}", got[&sample]);
    let expected = checked(&p, "src/main.tl");
    assert_eq!(
        expected.len(),
        1,
        "the dependent now has an error: {expected:?}"
    );
    assert_eq!(published(&got[&main]), expected);
    assert_eq!(got[&main][0]["severity"], 1);

    // And back: the dependent is fresh again, not served from what it last saw.
    write(&sample, &original);
    client.save(&sample);
    let got = client.publishes(&[&main, &sample]);
    assert!(got[&main].is_empty() && got[&sample].is_empty(), "{got:?}");
    assert!(checked(&p, "src/main.tl").is_empty());

    // A file that is closed is no one's dependent: saving the module publishes the module only.
    client.close(&main);
    let got = client.publishes(&[&main]);
    assert!(got[&main].is_empty());
    write(&sample, &original.replace("text", "message"));
    client.save(&sample);
    let got = client.publishes(&[&sample]);
    assert!(got[&sample].is_empty());
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// An error in a module the open file requires is `htl check`'s error against that module;
/// the editor shows it on the first line of the file that requires it, pointing at the module.
#[test]
fn an_error_in_a_required_module_is_shown_on_the_file_that_requires_it() {
    let (root, p) = project("dependency-error");
    let main = p.join("src/main.tl");
    let sample = p.join("src/sample/init.tl");
    write(
        &sample,
        "local record sample\nend\n\nfunction sample.greet(who: string): number\n   return who\nend\n\nreturn sample\n",
    );
    let (mut client, _) = Client::start(&p, &root);
    client.open(&main);
    let got = client.publishes(&[&main]);
    let ds = &got[&main];
    assert_eq!(ds.len(), 2, "{ds:?}");
    let in_module = ds
        .iter()
        .find(|d| d["relatedInformation"].is_array())
        .unwrap_or_else(|| panic!("one points at the module: {ds:?}"));
    assert_eq!(
        in_module["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        in_module["relatedInformation"][0]["location"]["uri"],
        uri(&sample)
    );
    // The message is `htl check`'s for the module, with the place it is at.
    let out = Command::new(common::htl_bin())
        .args(["check", "src/main.tl", "--format", "json", "--no-cache"])
        .current_dir(&p)
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let theirs = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["file"] == "src/sample/init.tl")
        .unwrap();
    let message = format!(
        "src/sample/init.tl:{}:{}: {}",
        theirs["line"],
        theirs["col"],
        theirs["message"].as_str().unwrap()
    );
    assert_eq!(in_module["message"], message.as_str());
    // The module's own error is the module's: once it is open it reports it itself, and
    // the file that requires it is re-checked and stops repeating it.
    client.open(&sample);
    let got = client.publishes(&[&sample, &main]);
    assert_eq!(got[&sample].len(), 1, "{:?}", got[&sample]);
    assert_eq!(got[&sample][0]["range"]["start"]["line"], 4);
    assert_eq!(got[&main].len(), 1, "{:?}", got[&main]);
    assert!(
        got[&main][0]["relatedInformation"].is_null(),
        "{:?}",
        got[&main]
    );
    // Closed again, the module's error is the requiring file's to show once more.
    client.close(&sample);
    let got = client.publishes(&[&sample, &main]);
    assert!(got[&sample].is_empty(), "{:?}", got[&sample]);
    assert_eq!(got[&main].len(), 2, "{:?}", got[&main]);
    let (unread, _) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
}

/// A burst of saves is at most one check each, and the last publish is about the last state
/// on disk. (That a queued burst is one check is the unit test in `lsp.rs`, which can
/// arrange for the messages to be queued; a pipe cannot promise it.)
#[test]
fn the_last_publish_of_a_burst_of_saves_is_about_the_file_as_it_is() {
    let (root, p) = project("burst");
    let main = p.join("src/main.tl");
    let broken =
        "local sample = require(\"sample\")\n\nlocal n: number = \"x\"\nprint(n, sample)\n";
    let fixed = "local sample = require(\"sample\")\n\nlocal n: number = 1\nprint(n, sample)\n";
    write(&main, broken);
    let (mut client, _) = Client::start(&p, &root);
    client.open(&main);
    let first = client.publishes(&[&main]);
    assert_eq!(first[&main].len(), 1);

    write(&main, fixed);
    for _ in 0..4 {
        client.save(&main);
    }
    let (unread, code) = client.finish();
    assert_eq!(code, Some(0));
    // The first of the burst was read from the pipe by `finish`, so it is in `unread` with
    // the rest: between one and four publishes, all of them about the fixed file.
    assert!(!unread.is_empty() && unread.len() <= 4, "{}", unread.len());
    for m in &unread {
        assert_eq!(m["params"]["diagnostics"], json!([]), "{m}");
    }
}

// ---------------------------------------------------------------- formatting, code actions

/// The text after applying `edits` (LSP `TextEdit`s, positions in UTF-16 units of the
/// original) to `text`. The edits are applied back to front, so each is about the original;
/// two at one position keep their order.
fn apply(text: &str, edits: &[Value]) -> String {
    let offset = |pos: &Value| {
        let (line, unit) = (
            pos["line"].as_u64().unwrap() as usize,
            pos["character"].as_u64().unwrap() as usize,
        );
        let start: usize = text.split('\n').take(line).map(|l| l.len() + 1).sum();
        let mut units = 0;
        let mut at = start;
        for c in text[start..].chars() {
            if units >= unit || c == '\n' {
                break;
            }
            units += c.len_utf16();
            at += c.len_utf8();
        }
        at
    };
    let mut spans: Vec<(usize, usize, usize, &str)> = edits
        .iter()
        .enumerate()
        .map(|(i, e)| {
            (
                offset(&e["range"]["start"]),
                offset(&e["range"]["end"]),
                i,
                e["newText"].as_str().unwrap(),
            )
        })
        .collect();
    spans.sort_by_key(|&(s, _, i, _)| (std::cmp::Reverse(s), std::cmp::Reverse(i)));
    let mut out = text.to_string();
    for (s, e, _, new) in spans {
        out.replace_range(s..e, new);
    }
    out
}

/// What `htl <args>` leaves `src/<scratch>.tl` as, run from `cwd` on a copy of `text`
/// written there: the command the server's answer is held to, on the same project.
fn written_by(cwd: &Path, args: &[&str], scratch: &str, text: &str) -> String {
    let copy = cwd.join("src").join(scratch);
    write(&copy, text);
    let out = Command::new(common::htl_bin())
        .args(args)
        .arg(format!("src/{scratch}"))
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "htl {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read_to_string(copy).unwrap()
}

fn formatting(client: &mut Client, file: &Path) -> Value {
    client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri(file) },
                "options": { "tabSize": 3, "insertSpaces": true } }),
    )
}

/// The code actions at `range` (a whole-file range when `None`), of the kinds in `only`.
fn actions(
    client: &mut Client,
    file: &Path,
    range: Option<&Value>,
    only: Option<&[&str]>,
    diagnostics: &[Value],
) -> Vec<Value> {
    let whole = json!({ "start": { "line": 0, "character": 0 },
                        "end": { "line": 1000, "character": 0 } });
    // Invoked (1), as from a menu: the trigger that asks for fix-all without naming it.
    let mut context = json!({ "diagnostics": diagnostics, "triggerKind": 1 });
    if let Some(only) = only {
        context["only"] = json!(only);
    }
    let r = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri(file) },
                "range": range.unwrap_or(&whole), "context": context }),
    );
    assert!(r.get("error").is_none(), "{r}");
    r["result"].as_array().expect("an array").clone()
}

fn edits_of(action: &Value, file: &Path) -> Vec<Value> {
    action["edit"]["changes"][uri(file).as_str()]
        .as_array()
        .unwrap_or_else(|| panic!("an edit for the file: {action}"))
        .clone()
}

/// A file that needs two safe fixes (`explicit-number` twice).
const FIXABLE: &str = "local a = 0\na = a + 1.5\nlocal b = 0\nb = b + 2.5\nprint(a, b)\n";
/// A file that needs one unsafe fix (`no-global`).
const GLOBAL: &str = "global counter: number = 1\nreturn counter\n";

/// A `htl new` project with `explicit-number` on, and `extra` appended to its `htl.toml`.
fn lint_project(name: &str, extra: &str) -> (common::TempDir, PathBuf) {
    let (root, p) = project(name);
    let mut config = std::fs::read_to_string(p.join("htl.toml")).unwrap();
    config.push_str("\n[lint.rules]\nexplicit-number = \"warn\"\n");
    config.push_str(extra);
    write(&p.join("htl.toml"), &config);
    (root, p)
}

/// Acceptance 4: the handshake advertises both, and the edits formatting returns make the
/// file what `htl fmt` writes; a file already formatted gets none, and one the parser
/// rejects gets `null`. The file is never opened: formatting asks about the disk.
#[test]
fn formatting_returns_the_edits_that_make_the_file_what_fmt_writes() {
    let (root, p) = project("format");
    let file = p.join("src/messy.tl");
    let messy = "local function f(x: number): number\n        local y = x+1\n  return    y\n   end\nreturn f\n";
    write(&file, messy);
    let (mut client, init) = Client::start(&root, &root);
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["documentFormattingProvider"], true, "{init}");
    let kinds = caps["codeActionProvider"]["codeActionKinds"].clone();
    assert_eq!(kinds, json!(["quickfix", "source.fixAll.htl"]), "{init}");

    let expected = written_by(&p, &["fmt"], "expected.tl", messy);
    assert_ne!(expected, messy, "the case needs something to format");
    let r = formatting(&mut client, &file);
    let edits = r["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .clone();
    assert_eq!(apply(messy, &edits), expected);

    // Formatted: nothing to do. (The disk is the document; it is changed to be so.)
    write(&file, &expected);
    let r = formatting(&mut client, &file);
    assert_eq!(r["result"], json!([]), "{r}");

    // A file the parser rejects cannot be formatted: `null`, not an error.
    write(&file, "local x = = 1\n");
    let r = formatting(&mut client, &file);
    assert!(r.get("error").is_none() && r["result"].is_null(), "{r}");
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// The indent is the project's `[fmt] indent`, which `htl fmt` reads the same way.
#[test]
fn formatting_uses_the_projects_indent() {
    let (root, p) = project("format-indent");
    let mut config = std::fs::read_to_string(p.join("htl.toml")).unwrap();
    config = config.replace("indent = 3", "indent = 2");
    assert!(config.contains("indent = 2"));
    write(&p.join("htl.toml"), &config);
    let file = p.join("src/messy.tl");
    let messy = "local function f(x: number): number\n        return x+1\nend\nreturn f\n";
    write(&file, messy);
    let expected = written_by(&p, &["fmt"], "expected.tl", messy);
    assert!(expected.contains("\n  return"), "{expected}");
    let (mut client, _) = Client::start(&root, &root);
    let r = formatting(&mut client, &file);
    let edits = r["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .clone();
    assert_eq!(apply(messy, &edits), expected);
    client.finish();
}

/// Acceptance 5: a safe fix is a preferred quick fix on its diagnostic, linked to it, and
/// its edit is what `htl fix` writes.
#[test]
fn a_safe_fix_is_a_preferred_quick_fix_that_matches_fix() {
    let (root, p) = lint_project("quickfix-safe", "");
    let file = p.join("src/num.tl");
    let text = "local total = 0\ntotal = total + 1.5\nprint(total)\n";
    write(&file, text);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    let got = client.publishes(&[&file]);
    let lint = got[&file]
        .iter()
        .find(|d| d["code"] == "explicit-number")
        .unwrap_or_else(|| panic!("{:?}", got[&file]))
        .clone();

    let expected = written_by(
        &p,
        &["fix", "--allow-no-vcs", "--allow-dirty"],
        "expected.tl",
        text,
    );
    assert_ne!(expected, text);
    let found = actions(
        &mut client,
        &file,
        Some(&lint["range"]),
        Some(&["quickfix"]),
        std::slice::from_ref(&lint),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    let a = &found[0];
    assert_eq!(a["kind"], "quickfix", "{a}");
    assert_eq!(a["isPreferred"], true, "{a}");
    assert_eq!(a["title"], "Fix explicit-number", "{a}");
    assert_eq!(
        a["diagnostics"],
        json!([lint]),
        "linked to its diagnostic: {a}"
    );
    assert_eq!(apply(text, &edits_of(a, &file)), expected);

    // Elsewhere in the file (the error on the next line has no fix): no quick fix there.
    let elsewhere = json!({ "start": { "line": 2, "character": 0 },
                            "end": { "line": 2, "character": 5 } });
    let found = actions(
        &mut client,
        &file,
        Some(&elsewhere),
        Some(&["quickfix"]),
        &[],
    );
    assert!(found.is_empty(), "{found:?}");

    // A cursor at the very start of the name is on the finding.
    let cursor = json!({ "start": lint["range"]["start"], "end": lint["range"]["start"] });
    let found = actions(&mut client, &file, Some(&cursor), Some(&["quickfix"]), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    let (unread, _) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
}

/// An unsafe fix is offered, flagged in the title and not preferred; fix-all leaves it
/// alone, as `htl fix` does without `--unsafe`. Promoted by `[fix] unsafe` it is preferred
/// and part of fix-all, as it is for `htl fix`; disabled by `[fix] disable` it is offered
/// but not preferred, and not part of fix-all.
#[test]
fn an_unsafe_fix_is_offered_but_not_preferred_and_not_fixed_all() {
    let (root, p) = lint_project("quickfix-unsafe", "");
    let file = p.join("src/g.tl");
    write(&file, GLOBAL);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    client.publishes(&[&file]);
    let found = actions(&mut client, &file, None, None, &[]);
    assert_eq!(found.len(), 1, "no fix-all: {found:?}");
    let a = &found[0];
    assert_eq!(a["kind"], "quickfix", "{a}");
    assert_eq!(a["isPreferred"], false, "{a}");
    assert_eq!(a["title"], "Fix no-global (unsafe)", "{a}");
    let expected = written_by(
        &p,
        &["fix", "--unsafe", "--allow-no-vcs", "--allow-dirty"],
        "expected.tl",
        GLOBAL,
    );
    assert_ne!(expected, GLOBAL);
    assert_eq!(apply(GLOBAL, &edits_of(a, &file)), expected);
    let all = actions(&mut client, &file, None, Some(&["source.fixAll"]), &[]);
    assert!(all.is_empty(), "{all:?}");
    client.finish();

    // Promoted: `htl fix` applies it by default, so it is preferred and fixed all.
    let (root, p) = lint_project("quickfix-promoted", "\n[fix]\nunsafe = [\"no-global\"]\n");
    let file = p.join("src/g.tl");
    write(&file, GLOBAL);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    client.publishes(&[&file]);
    let expected = written_by(
        &p,
        &["fix", "--allow-no-vcs", "--allow-dirty"],
        "expected.tl",
        GLOBAL,
    );
    assert_ne!(expected, GLOBAL);
    let found = actions(&mut client, &file, None, None, &[]);
    let quick = found.iter().find(|a| a["kind"] == "quickfix").unwrap();
    assert_eq!(quick["isPreferred"], true, "{found:?}");
    assert_eq!(quick["title"], "Fix no-global", "{found:?}");
    let all = found
        .iter()
        .find(|a| a["kind"] == "source.fixAll.htl")
        .unwrap();
    assert_eq!(apply(GLOBAL, &edits_of(all, &file)), expected);
    client.finish();

    // Disabled: still a quick fix to ask for, never one to take without asking.
    let (root, _p) = lint_project(
        "quickfix-disabled",
        "\n[fix]\ndisable = [\"explicit-number\"]\n",
    );
    let file = _p.join("src/num.tl");
    write(&file, FIXABLE);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    client.publishes(&[&file]);
    let found = actions(&mut client, &file, None, None, &[]);
    assert!(
        !found.is_empty() && found.iter().all(|a| a["kind"] == "quickfix"),
        "{found:?}"
    );
    assert!(found.iter().all(|a| a["isPreferred"] == false), "{found:?}");
    client.finish();
}

/// Fix-all is `htl fix`'s result for the file, as one edit; it is offered only when it
/// changes something, is answered from the file as saved, and is the only action `only`
/// asks for.
#[test]
fn fix_all_is_what_fix_writes_and_only_when_it_changes_the_file() {
    let (root, p) = lint_project("fix-all", "");
    let file = p.join("src/num.tl");
    write(&file, FIXABLE);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    let got = client.publishes(&[&file]);
    let lints = got[&file]
        .iter()
        .filter(|d| d["code"] == "explicit-number")
        .count();
    assert_eq!(lints, 2, "{:?}", got[&file]);

    let expected = written_by(
        &p,
        &["fix", "--allow-no-vcs", "--allow-dirty"],
        "expected.tl",
        FIXABLE,
    );
    assert_eq!(expected.matches(": number").count(), 2, "{expected}");
    let all = actions(&mut client, &file, None, Some(&["source.fixAll"]), &[]);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0]["kind"], "source.fixAll.htl", "{all:?}");
    assert!(all[0]["isPreferred"].is_null(), "{all:?}");
    assert_eq!(apply(FIXABLE, &edits_of(&all[0], &file)), expected);
    // Asked again: the same answer, from the one computation.
    let again = actions(&mut client, &file, None, Some(&["source.fixAll.htl"]), &[]);
    assert_eq!(again, all);

    // `only` narrows: quick fixes alone, one per finding; neither kind for another.
    let quick = actions(&mut client, &file, None, Some(&["quickfix"]), &[]);
    assert_eq!(quick.len(), 2, "{quick:?}");
    assert!(quick.iter().all(|a| a["kind"] == "quickfix"));
    let none = actions(&mut client, &file, None, Some(&["refactor"]), &[]);
    assert!(none.is_empty(), "{none:?}");

    // Saved fixed: nothing left to fix, so no action, even though a save is right behind
    // the request (the request is answered about the file as that save left it).
    write(&file, &expected);
    client.change(&file, &expected);
    client.save(&file);
    let all = actions(&mut client, &file, None, None, &[]);
    assert!(all.is_empty(), "{all:?}");
    let got = client.publishes(&[&file]);
    assert!(
        got[&file].iter().all(|d| d["code"] != "explicit-number"),
        "{:?}",
        got[&file]
    );
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// A client that formats or fixes on save asks before it writes: the buffer is not the
/// file. Formatting formats the buffer, and an edit for the file would have reverted the
/// unsaved work; fixes belong to the text that was checked, so a buffer that has moved on
/// gets none, and gets them again once the change is saved and checked.
#[test]
fn a_buffer_that_differs_from_the_disk_is_formatted_and_gets_no_fixes() {
    let (root, p) = lint_project("buffer", "");
    let file = p.join("src/num.tl");
    write(&file, FIXABLE);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    client.publishes(&[&file]);
    assert!(!actions(&mut client, &file, None, None, &[]).is_empty());

    // Unsaved edits: messy, and a new line the disk does not have.
    let buffer =
        "-- unsaved\nlocal function f(x: number): number\n        return x+1\nend\nreturn f\n";
    client.change(&file, buffer);
    let expected = written_by(&p, &["fmt"], "expected.tl", buffer);
    assert_ne!(expected, buffer);
    let r = formatting(&mut client, &file);
    let edits = r["result"]
        .as_array()
        .unwrap_or_else(|| panic!("{r}"))
        .clone();
    assert_eq!(
        apply(buffer, &edits),
        expected,
        "formats the buffer, not the disk"
    );
    // And nothing was checked for the change.
    assert!(actions(&mut client, &file, None, None, &[]).is_empty());

    // Back to the checked text: the fixes are valid again; saved other text, they are the
    // new text's.
    client.change(&file, FIXABLE);
    assert!(!actions(&mut client, &file, None, None, &[]).is_empty());
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "no publish for a change: {unread:?}");
    assert_eq!(code, Some(0));
}

/// Fix-all costs a dry run of `htl fix`, so a client that did not ask for it by kind or by
/// invoking the menu (the lightbulb on a cursor move) is not given it.
#[test]
fn fix_all_is_not_computed_for_an_automatic_request() {
    let (root, p) = lint_project("automatic", "");
    let file = p.join("src/num.tl");
    write(&file, FIXABLE);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    client.publishes(&[&file]);
    let r = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri(&file) },
                "range": { "start": { "line": 0, "character": 0 },
                           "end": { "line": 1000, "character": 0 } },
                "context": { "diagnostics": [], "triggerKind": 2 } }),
    );
    let found = r["result"].as_array().unwrap();
    assert_eq!(found.len(), 2, "the quick fixes only: {found:?}");
    assert!(found.iter().all(|a| a["kind"] == "quickfix"), "{found:?}");
    client.finish();
}

/// A `[fix]` section `htl fix` refuses is a setup error on the file, and no fix is offered
/// while it stands.
#[test]
fn a_bad_fix_section_is_a_setup_error_and_offers_no_fix() {
    let (root, p) = lint_project("bad-fix", "\n[fix]\ndisable = [\"no-such-rule\"]\n");
    let file = p.join("src/num.tl");
    write(&file, FIXABLE);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    let got = client.publishes(&[&file]);
    let setup: Vec<&Value> = got[&file]
        .iter()
        .filter(|d| d["message"].as_str().unwrap().contains("no-such-rule"))
        .collect();
    assert_eq!(setup.len(), 1, "{:?}", got[&file]);
    assert_eq!(setup[0]["severity"], 1);
    assert!(actions(&mut client, &file, None, None, &[]).is_empty());
    client.finish();
}

/// Edits are positions in the buffer's UTF-16 columns, and a file with CRLF line ends is
/// fixed without disturbing them.
#[test]
fn a_fix_in_a_file_with_crlf_and_multibyte_text_matches_fix() {
    let (root, p) = lint_project("crlf", "");
    let file = p.join("src/num.tl");
    let text =
        "-- caf\u{e9} \u{1F600}\r\nlocal total = 0\r\ntotal = total + 1.5\r\nprint(total)\r\n";
    write(&file, text);
    let (mut client, _) = Client::start(&root, &root);
    client.open(&file);
    let got = client.publishes(&[&file]);
    let lint = got[&file]
        .iter()
        .find(|d| d["code"] == "explicit-number")
        .unwrap_or_else(|| panic!("{:?}", got[&file]))
        .clone();
    let expected = written_by(
        &p,
        &["fix", "--allow-no-vcs", "--allow-dirty"],
        "expected.tl",
        text,
    );
    assert_ne!(expected, text);
    let found = actions(
        &mut client,
        &file,
        Some(&lint["range"]),
        Some(&["quickfix"]),
        &[],
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(apply(text, &edits_of(&found[0], &file)), expected);
    let all = actions(&mut client, &file, None, Some(&["source.fixAll"]), &[]);
    assert_eq!(apply(text, &edits_of(&all[0], &file)), expected);
    client.finish();
}

/// `textDocument/hover` at a 0-based line and UTF-16 column.
fn hover(client: &mut Client, file: &Path, line: u32, character: u32) -> Value {
    let r = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri(file) },
                "position": { "line": line, "character": character } }),
    );
    assert!(r.get("error").is_none(), "{r}");
    r["result"].clone()
}

/// `textDocument/definition` at a 0-based line and UTF-16 column.
fn definition(client: &mut Client, file: &Path, line: u32, character: u32) -> Value {
    let r = client.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": uri(file) },
                "position": { "line": line, "character": character } }),
    );
    assert!(r.get("error").is_none(), "{r}");
    r["result"].clone()
}

/// The text a hover shows: the body of its markdown code block.
fn shown(h: &Value) -> String {
    assert_eq!(h["contents"]["kind"], "markdown", "{h}");
    let v = h["contents"]["value"].as_str().unwrap();
    v.strip_prefix("```teal\n")
        .and_then(|v| v.strip_suffix("\n```"))
        .unwrap_or_else(|| panic!("a teal code block: {v:?}"))
        .to_string()
}

fn range_of(h: &Value) -> (u64, u64, u64, u64) {
    let r = &h["range"];
    (
        r["start"]["line"].as_u64().unwrap(),
        r["start"]["character"].as_u64().unwrap(),
        r["end"]["line"].as_u64().unwrap(),
        r["end"]["character"].as_u64().unwrap(),
    )
}

/// The `htl new` scaffold, opened and checked: `src/main.tl` is
///
/// ```text
/// local sample = require("sample")
///
/// local g = sample.greet(arg and arg[1] or "teal")
/// print(g.text)
/// ```
fn opened_scaffold(name: &str) -> (common::TempDir, PathBuf, PathBuf, Client) {
    let (root, p) = project(name);
    let main = p.join("src/main.tl");
    assert_eq!(
        std::fs::read_to_string(&main).unwrap().lines().nth(3),
        Some("print(g.text)"),
        "the scaffold this case reads positions of"
    );
    let (mut client, init) = Client::start(&root, &root);
    let caps = &init["result"]["capabilities"];
    assert_eq!(caps["hoverProvider"], true, "{init}");
    assert_eq!(caps["definitionProvider"], true, "{init}");
    client.open(&main);
    client.publishes(&[&main]);
    (root, p, main, client)
}

/// Acceptance 6: a hover on a local bound to a record shows the record's type, on a function
/// its signature; a position in the middle of a name, or just after it, is on the name, and
/// one that is on no name answers `null`.
#[test]
fn hover_shows_the_type_of_the_name_under_the_cursor() {
    let (_root, _p, main, mut client) = opened_scaffold("hover");

    // `g` in `print(g.text)`: the record the call returns.
    let h = hover(&mut client, &main, 3, 6);
    assert!(shown(&h).contains("Greeting"), "{h}");
    assert_eq!(range_of(&h), (3, 6, 3, 7), "{h}");

    // `text`, from its first character, its middle and just after it: the field's type.
    for character in [8, 10, 12] {
        let h = hover(&mut client, &main, 3, character);
        assert_eq!(shown(&h), "string", "{character}: {h}");
        assert_eq!(range_of(&h), (3, 8, 3, 12), "{character}: {h}");
    }

    // `greet` in `sample.greet(..)`, a field that is a function.
    let h = hover(&mut client, &main, 2, 17);
    assert!(shown(&h).contains("function"), "{h}");
    assert_eq!(range_of(&h), (2, 17, 2, 22), "{h}");

    // The local that holds the module: a name in a declaration is a name too.
    let h = hover(&mut client, &main, 0, 8);
    assert!(shown(&h).contains("sample"), "{h}");

    // No name: a blank line, a space after `=`, a keyword, a number, a string's text.
    for (line, character) in [(1, 0), (2, 9), (0, 0), (0, 3), (2, 36), (2, 43)] {
        let h = hover(&mut client, &main, line, character);
        assert_eq!(h, Value::Null, "{line}:{character}");
    }
    // Past the end of the file or of a line is the same answer, not an error.
    assert_eq!(hover(&mut client, &main, 40, 0), Value::Null);
    assert_eq!(hover(&mut client, &main, 3, 90), Value::Null);

    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// Acceptance 6: a definition on the argument of `require` is the file `htl resolve` prints
/// for the module, and on a name the line it is declared on.
#[test]
fn definition_of_a_require_is_the_file_resolve_prints() {
    let (_root, p, main, mut client) = opened_scaffold("definition");
    let out = Command::new(common::htl_bin())
        .args(["resolve", "sample"])
        .current_dir(&p)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let resolved = stdout
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("htl resolve sample: "))
        .unwrap_or_else(|| panic!("the first line names the file: {stdout}"));
    let init = p.join(resolved);
    assert!(init.is_file(), "{resolved}");

    // The string: its opening quote, its text, its closing quote and just after that.
    for character in [23, 24, 27, 30, 31] {
        let d = definition(&mut client, &main, 0, character);
        assert_eq!(d["uri"], uri(&init).as_str(), "{character}: {d}");
        assert_eq!(
            d["range"],
            json!({ "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 0 } }),
            "{character}: {d}"
        );
    }

    // `g` in `g.text` is the `g` of `local g`: line 2 (0-based), the name.
    let d = definition(&mut client, &main, 3, 6);
    assert_eq!(d["uri"], uri(&main).as_str(), "{d}");
    assert_eq!(
        d["range"]["start"],
        json!({ "line": 2, "character": 6 }),
        "{d}"
    );
    // `sample` the module, used on line 2, is declared by the `local` on line 0.
    let d = definition(&mut client, &main, 2, 10);
    assert_eq!(d["uri"], uri(&main).as_str(), "{d}");
    assert_eq!(d["range"]["start"]["line"], 0, "{d}");

    // `greet` is `function sample.greet` in the module: the line the checker names, in the
    // file the require resolved to.
    let d = definition(&mut client, &main, 2, 17);
    assert_eq!(d["uri"], uri(&init).as_str(), "{d}");
    let declared = std::fs::read_to_string(&init).unwrap();
    let at = declared
        .lines()
        .position(|l| l.starts_with("function sample.greet"))
        .unwrap();
    assert_eq!(d["range"]["start"]["line"], at, "{d}");

    // `print` is declared in Teal's standard library, which is not a file: no answer.
    assert_eq!(definition(&mut client, &main, 3, 2), Value::Null);
    // Nor is there one where there is no name, or in a string that is not a `require`'s.
    assert_eq!(definition(&mut client, &main, 1, 0), Value::Null);
    assert_eq!(definition(&mut client, &main, 2, 36), Value::Null);

    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// A hover and a definition are positions in the text the check read. Once the buffer is
/// something else, the positions are in a text that is gone: both answer `null` until a save
/// is checked, whatever the position would have been on.
#[test]
fn hover_and_definition_answer_only_for_the_text_that_was_checked() {
    let (_root, _p, main, mut client) = opened_scaffold("hover-edited");
    let text = std::fs::read_to_string(&main).unwrap();
    assert_ne!(hover(&mut client, &main, 3, 6), Value::Null);

    // A line inserted above: what was at 3:6 is now at 4:6, and 3:6 is another line's.
    client.change(&main, &format!("-- a new first line\n{text}"));
    assert_eq!(hover(&mut client, &main, 4, 6), Value::Null);
    assert_eq!(hover(&mut client, &main, 3, 6), Value::Null);
    assert_eq!(definition(&mut client, &main, 4, 6), Value::Null);
    assert_eq!(definition(&mut client, &main, 1, 24), Value::Null);

    // The text put back is the text that was checked, and it answers again.
    client.change(&main, &text);
    assert!(shown(&hover(&mut client, &main, 3, 6)).contains("Greeting"));

    // Saved: what is on disk is checked, and the new text is the one that answers.
    write(&main, &format!("-- a new first line\n{text}"));
    client.change(&main, &format!("-- a new first line\n{text}"));
    client.save(&main);
    client.publishes(&[&main]);
    let h = hover(&mut client, &main, 4, 6);
    assert!(shown(&h).contains("Greeting"), "{h}");
    assert_eq!(range_of(&h), (4, 6, 4, 7), "{h}");
    let d = definition(&mut client, &main, 4, 6);
    assert_eq!(d["range"]["start"]["line"], 3, "{d}");

    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// Columns are UTF-16 units on the way in and out, and a name is found by its bytes: a line
/// with characters before the name, two bytes and then four, still has the name under the
/// cursor.
#[test]
fn hover_and_definition_count_columns_in_utf16_units() {
    let (_root, p, main, mut client) = opened_scaffold("hover-utf16");
    let line = "local note = \"\u{e9}\u{1F600}\"; local count: integer = 7; print(note, count)\n";
    let file = p.join("src/wide.tl");
    write(&file, line);
    client.open(&file);
    client.publishes(&[&file]);
    // `count` the second time: after `é` (1 unit) and the emoji (2 units).
    let units = |needle: &str, nth: usize| {
        let (byte, _) = line.match_indices(needle).nth(nth).unwrap();
        line[..byte].encode_utf16().count() as u32
    };
    let use_at = units("count", 1);
    let h = hover(&mut client, &file, 0, use_at + 2);
    assert_eq!(shown(&h), "integer", "{h}");
    assert_eq!(
        range_of(&h),
        (0, use_at as u64, 0, use_at as u64 + 5),
        "{h}"
    );
    let d = definition(&mut client, &file, 0, use_at + 2);
    assert_eq!(d["uri"], uri(&file).as_str(), "{d}");
    assert_eq!(
        d["range"]["start"],
        json!({ "line": 0, "character": units("count", 0) }),
        "{d}"
    );
    drop(main);
    let (_, code) = client.finish();
    assert_eq!(code, Some(0));
}

/// What a changed manifest does, for the cases below: a project with `src/g.tl`, which has
/// one `no-global` lint at the scaffold's levels, and `src/main.tl`, which has nothing.
fn global_project(name: &str) -> (common::TempDir, PathBuf, PathBuf, PathBuf) {
    let (root, p) = project(name);
    let g = p.join("src/g.tl");
    write(&g, "global counter: number = 1\nreturn counter\n");
    let main = p.join("src/main.tl");
    (root, p, g, main)
}

/// `htl.toml` with `no-global` set to `level`: the scaffold's, and the rule under
/// `[lint.rules]`, which the scaffold leaves out.
fn set_no_global(p: &Path, level: &str) {
    let config = std::fs::read_to_string(p.join("htl.toml")).unwrap();
    let config = config
        .split("\n[lint.rules]")
        .next()
        .unwrap()
        .trim_end()
        .to_string();
    write(
        &p.join("htl.toml"),
        &format!("{config}\n\n[lint.rules]\nno-global = \"{level}\"\n"),
    );
}

/// Acceptance 7, for a client that watches files: the server asks to watch what changes the
/// answer, and a change to `htl.toml` re-publishes every open file of the project, as `htl
/// check` now reports them, without a save.
#[test]
fn a_changed_htl_toml_rechecks_the_open_files_when_the_client_watches_files() {
    let (root, p, g, main) = global_project("watch-config");
    let (mut client, _) = Client::start_watching(&p, &root);
    let r = client.registration();
    let registration = &r["params"]["registrations"][0];
    assert_eq!(registration["method"], DidChangeWatchedFiles::METHOD, "{r}");
    let globs: Vec<&str> = registration["registerOptions"]["watchers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["globPattern"].as_str().unwrap())
        .collect();
    for wanted in [
        "**/htl.toml",
        "**/mlua-pkg.toml",
        "**/types/**/*.d.tl",
        "**/Cargo.toml",
    ] {
        assert!(globs.contains(&wanted), "{wanted} in {globs:?}");
    }
    assert!(globs.iter().any(|g| g.ends_with("/*.rs")), "{globs:?}");

    client.open(&g);
    client.open(&main);
    let got = client.publishes(&[&g, &main]);
    let before = checked(&p, "src/g.tl");
    assert_eq!(before.len(), 1, "{before:?}");
    assert_eq!(before[0].0, "no-global");
    assert_eq!(published(&got[&g]), before);
    assert_eq!(got[&g][0]["severity"], 2);

    // The rule is turned off. Nothing is saved: the client says the file changed.
    set_no_global(&p, "allow");
    client.watched(&[(&p.join("htl.toml"), 2)]);
    let got = client.publishes(&[&g, &main]);
    assert!(checked(&p, "src/g.tl").is_empty());
    assert_eq!(published(&got[&g]), checked(&p, "src/g.tl"));
    assert!(got[&g].is_empty() && got[&main].is_empty(), "{got:?}");

    // And turned up to `deny`: an error now, as `htl check` has it.
    set_no_global(&p, "deny");
    client.watched(&[(&p.join("htl.toml"), 2)]);
    let got = client.publishes(&[&g, &main]);
    let after = checked(&p, "src/g.tl");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(published(&got[&g]), after);
    assert_eq!(got[&g][0]["severity"], 1, "{}", got[&g][0]);

    // Many events for one project are one re-check of each file: a second publish of a
    // file before the rest is a failure of `publishes`.
    set_no_global(&p, "warn");
    client.watched(&[
        (&p.join("htl.toml"), 2),
        (&p.join("mlua-pkg.toml"), 2),
        (&p.join("types/README.md"), 2),
        (&p.join("htl.toml"), 2),
    ]);
    let got = client.publishes(&[&g, &main]);
    assert_eq!(published(&got[&g]), checked(&p, "src/g.tl"));
    assert_eq!(got[&g][0]["severity"], 2);

    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// Acceptance 7, for a client that does not watch files: no registration is sent, and the
/// next save of a file in the project sees the edited `htl.toml` (and re-checks the project's
/// other open files with it).
#[test]
fn a_changed_htl_toml_is_seen_at_the_next_save_when_the_client_does_not_watch_files() {
    let (root, p, g, main) = global_project("watch-fallback");
    let (mut client, _) = Client::start(&p, &root);
    client.open(&g);
    client.open(&main);
    let got = client.publishes(&[&g, &main]);
    assert_eq!(published(&got[&g]), checked(&p, "src/g.tl"));
    assert_eq!(got[&g][0]["severity"], 2);

    set_no_global(&p, "deny");
    client.save(&main);
    let got = client.publishes(&[&g, &main]);
    let after = checked(&p, "src/g.tl");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(published(&got[&g]), after);
    assert_eq!(got[&g][0]["severity"], 1, "{}", got[&g][0]);
    assert!(got[&main].is_empty(), "{:?}", got[&main]);

    // An `htl.toml` that is the same as before is not a reason to check what was not saved.
    set_no_global(&p, "deny");
    client.save(&main);
    let got = client.publishes(&[&main]);
    assert!(got[&main].is_empty());

    set_no_global(&p, "allow");
    client.save(&g);
    let got = client.publishes(&[&g, &main]);
    assert!(got[&g].is_empty() && got[&main].is_empty(), "{got:?}");

    // Everything before this was read, so a request the server made would have been seen.
    let r = client.request("textDocument/completion", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    assert!(
        client.server_requests.is_empty(),
        "a client that does not watch files is not asked to: {:?}",
        client.server_requests
    );
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// A `.d.tl` under `types/` is an input to the check of every file that requires it: one that
/// appears, one that is edited, one that goes.
#[test]
fn a_changed_declaration_under_types_rechecks_the_file_that_requires_it() {
    let (root, p) = project("watch-types");
    let u = p.join("src/u.tl");
    write(
        &u,
        "local x = require(\"xdecl\")\nlocal s: number = x.f()\nprint(s)\n",
    );
    let decl = p.join("types/xdecl.d.tl");
    let declare = |ret: &str| {
        write(
            &decl,
            &format!("local record xdecl\n   f: function(): {ret}\nend\nreturn xdecl\n"),
        );
    };
    let (mut client, _) = Client::start_watching(&p, &root);
    client.registration();
    client.open(&u);
    let got = client.publishes(&[&u]);
    let missing = checked(&p, "src/u.tl");
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0].3.contains("module not found"), "{missing:?}");
    assert_eq!(published(&got[&u]), missing);

    // Created: it declares what the file wants.
    declare("number");
    client.watched(&[(&decl, 1)]);
    let got = client.publishes(&[&u]);
    assert!(checked(&p, "src/u.tl").is_empty());
    assert!(got[&u].is_empty(), "{:?}", got[&u]);

    // Changed: it declares something else.
    declare("string");
    client.watched(&[(&decl, 2)]);
    let got = client.publishes(&[&u]);
    let wrong = checked(&p, "src/u.tl");
    assert_eq!(wrong.len(), 1, "{wrong:?}");
    assert_eq!(published(&got[&u]), wrong);

    // Deleted: the module is not found again.
    std::fs::remove_file(&decl).unwrap();
    client.watched(&[(&decl, 3)]);
    let got = client.publishes(&[&u]);
    assert_eq!(published(&got[&u]), missing);

    // Rust the project's crate does not read (and a file under `target/`) is no reason to.
    client.watched(&[(&p.join("target/debug/build/x/out/gen.rs"), 2)]);
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

/// A client that takes the registration and answers it with an error gets the fallback, and
/// the refusal is said on stderr.
#[test]
fn a_refused_registration_is_said_and_the_fallback_still_works() {
    let (root, p, g, _main) = global_project("watch-refused");
    let (mut client, _) = Client::start_watching(&p, &root);
    let request = loop {
        let m = client.read();
        if m["method"] == "client/registerCapability" {
            break m;
        }
        client.backlog.push(m);
    };
    client.send(json!({
        "id": request["id"],
        "error": { "code": -32601, "message": "no watching here" }
    }));
    client.open(&g);
    let got = client.publishes(&[&g]);
    assert_eq!(got[&g][0]["severity"], 2);
    set_no_global(&p, "deny");
    client.save(&g);
    let got = client.publishes(&[&g]);
    assert_eq!(got[&g][0]["severity"], 1, "{}", got[&g][0]);
    let stderr = client.stderr.clone();
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
    let said = std::fs::read_to_string(stderr).unwrap();
    assert!(said.contains("refused to watch files"), "{said}");
    assert!(said.contains("no watching here"), "{said}");
}

/// Rust that `htl dts` reads is an input too: a change under the crate's `src/` rebuilds the
/// project (which runs `htl dts` again) and re-checks the open file; the `.rs` files cargo
/// writes under `target/`, which it does not read, are not a reason to.
#[test]
fn a_changed_rust_source_rechecks_the_open_files_of_its_crate_and_target_does_not() {
    let root = tempdir("watch-rust");
    let out = Command::new(common::htl_bin())
        .args(["new", "sample", "--embed"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(out.status.success());
    let p = root.join("sample");
    let main = p.join("src/main.tl");
    let (mut client, _) = Client::start_watching(&p, &root);
    client.registration();
    client.open(&main);
    let got = client.publishes(&[&main]);
    assert_eq!(published(&got[&main]), checked(&p, "src/main.tl"));

    client.watched(&[(&p.join("target/debug/build/x/out/gen.rs"), 2)]);
    // An answer to a request is after everything before it was done.
    let r = client.request("textDocument/completion", json!({}));
    assert_eq!(r["error"]["code"], -32601, "{r}");
    assert!(
        client
            .backlog
            .iter()
            .all(|m| m["method"] != "textDocument/publishDiagnostics"),
        "a file under target/ is not one htl dts reads: {:?}",
        client.backlog
    );
    client.watched(&[(&p.join("src/lib.rs"), 2)]);
    let got = client.publishes(&[&main]);
    assert_eq!(published(&got[&main]), checked(&p, "src/main.tl"));
    let (unread, code) = client.finish();
    assert!(unread.is_empty(), "{unread:?}");
    assert_eq!(code, Some(0));
}

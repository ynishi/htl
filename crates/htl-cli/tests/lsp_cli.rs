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
}

impl Client {
    /// Spawn `htl lsp` in `cwd` and do the handshake; the `initialize` result comes back
    /// beside the client.
    fn start(cwd: &Path, scratch: &Path) -> (Self, Value) {
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
        };
        let init = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": uri(cwd), "capabilities": {} }),
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
            Ok(m) => m,
            Err(e) => panic!(
                "no message from htl lsp ({e}); its stderr:\n{}",
                std::fs::read_to_string(&self.stderr).unwrap_or_default()
            ),
        }
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
    assert_eq!(sync["change"], 0, "no didChange: {init}");
    assert_eq!(sync["save"]["includeText"], false, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "htl", "{init}");

    // A request the server does not answer is refused as such, and the server goes on.
    let r = client.request("textDocument/hover", json!({}));
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

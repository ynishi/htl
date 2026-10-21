//! `htl lsp`: a language server on stdio whose diagnostics are `htl check`'s (#500).
//!
//! An editor that runs `htl lsp` is told what `htl check` would say about the file it has
//! open — the project's resolution rules, its lints and their `-- htl: allow(..)` comments,
//! the `.d.tl` files `htl dts` writes — because the server does not reimplement any of it.
//! It builds the same things `htl check` builds for a project (the config, the
//! [model](htl::model), the lint [`Scope`](project::Scope), the checker
//! [`project::checker`] returns) and asks the same per-file question of them
//! ([`project::file_findings`], the part of [`project::check_one`] that decides what a file
//! reports). What is the server's own is the protocol: which file, when, and how a
//! [`htl::Diagnostic`] becomes an LSP one.
//!
//! # Shape
//!
//! One thread reads messages and owns every [`Htl`] state. mlua is built without its
//! `send` feature, so a state is `!Send` and has to stay on the thread that made it; a
//! synchronous loop over `lsp-server`'s channel (rust-analyzer's, which ruff and gleam use
//! as well) is the shape that needs no lock and no runtime. The alternative that was
//! weighed, `tower-lsp`, runs handlers concurrently and can apply ordered notifications out
//! of order, which is the wrong default for a server whose answer depends on the order of
//! saves.
//!
//! # What is checked, and when
//!
//! A file is checked when the client opens it and when it saves it, from disk, never from a
//! buffer: the server asks for open / close / save notifications and no `didChange`
//! (`textDocumentSync.change` is `None`), so what it reports is always about a text that
//! exists as a file. Diagnostics while typing need the checker to answer for text that is
//! not on disk and is often not a program, which is a later step.
//!
//! **Every check is a fresh one.** The check is [`Htl::check_written`], the call `htl fix`
//! measures a file by after writing it: a cold environment, nothing seeded from the
//! checker's store and nothing put into it. [`Htl::check`] would serve a module the checker
//! has already loaded, and a server that outlives the edit is exactly the caller that
//! answer is wrong for — it would describe the file as it was when the session started.
//! The price is that each check walks the file's whole `require` closure again; on a small
//! project that is milliseconds (the state itself, built once per project, is most of the
//! cost of a first check).
//!
//! **Dependents.** A check records the files the checked one reached through `require`
//! ([`htl::CheckInfo::deps`], the closure). When a file is saved, every other open file
//! whose recorded closure holds it is checked again as well, so a field renamed in a
//! required module is an error in the file that uses it without that file being saved. An
//! open or a close re-checks them too: an open file reports its own errors and a closed one
//! is reported through the files that require it, as in a walk of the open files.
//! Because each check is cold, the dependent sees the new text of the dependency without
//! any invalidation to get right. A module that did not exist when the dependent was last
//! checked is not in its closure, so creating it does not re-check the file that was
//! waiting for it; saving that file does.
//!
//! **Superseded work.** Messages are taken off the channel in batches: one blocking read,
//! then everything already queued. A batch is applied in order to the set of open files and
//! produces one set of files to check, each once, so three saves of one file that arrived
//! while a check was running become one check of what is on disk now. "Already queued"
//! needs a queue: `lsp-server`'s stdio reader hands messages over a zero-capacity channel,
//! so its reader would sit on the second message and leave the rest in the pipe, and every
//! save would be a check of its own. A thread moves them into an unbounded channel as they
//! are read, which is what the batch is drained from. A `shutdown` in a batch is answered
//! after the checks the messages before it asked for, so a client that waits for the
//! answer has had every publish those messages lead to.
//! `$/cancelRequest` is accepted and does nothing: this server answers no request that
//! takes any time (a check is triggered by a notification, and a notification has no
//! response to cancel), so there is never a running request to stop.
//!
//! # What a diagnostic becomes
//!
//! - `message` is [`htl::Diagnostic::message`], the sentence `htl check` prints, and `code` is
//!   its [`rule`](htl::Diagnostic::rule) (`nil-index`, `tl:unused`, `tl:error`, ...), the name
//!   `-- htl: allow(..)` and `htl check --explain` take. `source` is `htl`.
//! - The range starts where `htl check` says (`line` and `col`, 1-based, `col` counted in
//!   bytes) converted to LSP's 0-based line and UTF-16 column. The checker reports a point;
//!   the range ends after the identifier that starts there, or after one character if what
//!   is there is not part of one, so that a client has something to underline.
//! - Severity: an `error` is an error; a Teal `warning` and an htl `lint` are warnings,
//!   except that a finding under a rule the project sets to `deny`, or any finding of a
//!   project with `[lint] strict`, is an error, because that is what makes `htl check` fail
//!   on it ([`htl::verdict::verdict`]).
//! - A finding about another file — an error in a module the file requires, which `htl
//!   check` reports against that module — is shown on the first line of the file that
//!   requires it, with the dependency's own position in `relatedInformation`; the
//!   dependency reports itself when it is open. A finding with no position is on the
//!   first line too.
//! - A file that is not on disk (a buffer opened before its first save, a file deleted
//!   while open) has nothing to report until it is saved, and is published empty.
//!
//! # What it does not do yet
//!
//! A project's `htl.toml`, `mlua-pkg.toml`, `types/` and the Rust sources `htl dts` reads
//! are read once, when the first file of the project is opened; a change there needs the
//! server restarted until they are watched. The findings no single file carries (a require
//! cycle, a contract no host enforces) are not reported: they need the whole walk, and the
//! scope here is the open files and what they require.

use super::{auto_dts, load_config, warn_cargo_htl_mismatch};
use anyhow::{Context, Result};
use htl::Htl;
use htl::project::{self, Collect, Sink};
use lsp_server::{Connection, ErrorCode, Message, Notification, Response};
use lsp_types::notification::{
    Cancel, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Exit,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Request as _, Shutdown};
use lsp_types::{
    DiagnosticRelatedInformation, DiagnosticSeverity, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, Location, NumberOrString, Position,
    PublishDiagnosticsParams, Range, SaveOptions, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Uri,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

/// Run the server on stdio until the client says `exit` (or goes away).
///
/// The exit code is the one the protocol asks for: 0 after a `shutdown` request, 1 when the
/// client exits without one.
pub fn serve() -> Result<ExitCode> {
    let (stdio, io_threads) = Connection::stdio();
    // The stdio reader hands each message over a zero-capacity channel and reads the next
    // only once it is taken, so behind a running check there is never more than one
    // message queued. Moved into an unbounded channel as they are read, the ones that
    // arrive during a check are all there for the next batch (module doc, "Superseded
    // work"). The thread ends with the reader, at `exit` or the end of stdin.
    let (tx, receiver) = crossbeam_channel::unbounded();
    let source = stdio.receiver;
    std::thread::spawn(move || {
        for message in source {
            if tx.send(message).is_err() {
                break;
            }
        }
    });
    let connection = Connection {
        sender: stdio.sender,
        receiver,
    };
    let code = run_on(&connection)?;
    // The writer thread ends when the last sender is gone.
    drop(connection);
    io_threads.join().context("closing stdio")?;
    Ok(code)
}

/// The handshake, then the loop, over any [`Connection`]: stdio for the server, an
/// in-memory pair for a test.
fn run_on(connection: &Connection) -> Result<ExitCode> {
    let (id, _params) = connection
        .initialize_start()
        .context("waiting for the client's initialize request")?;
    let capabilities = ServerCapabilities {
        // Open, close and save, and nothing about edits: a file is checked as saved.
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::NONE),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                will_save: None,
                will_save_wait_until: None,
            },
        )),
        ..ServerCapabilities::default()
    };
    connection
        .initialize_finish(
            id,
            serde_json::json!({
                "capabilities": capabilities,
                "serverInfo": { "name": "htl", "version": env!("CARGO_PKG_VERSION") },
            }),
        )
        .context("finishing the handshake")?;
    Server::new(connection).run()
}

/// What the server knows about a project, built when its first file is opened.
///
/// The things `cmd_check` builds once per run, held for the session instead.
struct Project {
    cfg: project::Config,
    model: Option<htl::model::Project>,
    scope: project::Scope,
    origins: project::Origins,
    /// How `htl check` judges the project's findings: `[lint] strict` makes every one of
    /// them fail the run, so every one of them is an error here.
    policy: htl::verdict::Policy,
    /// The state every check of the project runs in. Built once ([`project::checker`]
    /// costs about as much as a small project's whole check) and never asked to
    /// remember anything: checks go through [`Htl::check_written`].
    checker: Htl,
}

impl Project {
    /// The project `file` belongs to, set up as `htl check` sets one up: config, then the
    /// declarations generated from Rust, then the model, the lint scope, the checker.
    fn load(file: &Path) -> Result<Self> {
        let cfg = load_config(file)?;
        warn_cargo_htl_mismatch(file);
        // A file `htl dts` writes is an input to the check; the failure to write one is
        // said and does not stop the file being checked.
        if let Err(e) = auto_dts(file) {
            eprintln!("htl lsp: {e:#}");
        }
        let model = project::model_of(&cfg, file)?;
        let scope = project::Scope::new(&cfg, model.as_ref(), file, None)?;
        let origins = project::Origins::new(model.as_ref());
        let policy = htl::verdict::Policy::resolve(cfg.as_ref().map(|(_, _, c)| c), false);
        let checker = project::checker(
            model.as_ref(),
            scope.lints.selection(),
            &project::lang_of(&cfg),
        )?;
        Ok(Self {
            cfg,
            model,
            scope,
            origins,
            policy,
            checker,
        })
    }

    /// Check `file` as it is on disk, the way `htl check` reports it: everything the file
    /// and this layer have to say about it, and the files its check reached.
    ///
    /// `open` is every file the client has open, which the sink treats as walked: a
    /// dependency error in one of them is that file's to report, as it is for the files
    /// of an `htl check` walk.
    fn check(&self, file: &Path, open: &[PathBuf]) -> Result<(Vec<htl::Diagnostic>, Vec<PathBuf>)> {
        let mut sink = Sink::new(Collect::default());
        sink.walking(open);
        let walk = self
            .scope
            .walk(&self.cfg, self.model.as_ref(), &self.origins);
        // The per-file question `htl check`'s walk asks, from disk.
        let (c, _) =
            project::check_file(&self.checker, &mut sink, file, &walk, project::Fresh::Disk)?;
        Ok((sink.out().take(), c.deps))
    }
}

/// A document the client has open.
struct Doc {
    path: PathBuf,
    /// Where `path` really is, for comparing it with the files a check reached.
    canonical: PathBuf,
    /// The key of the [`Project`] it belongs to, once the walk up for it has answered. A
    /// walk that failed (an `htl.toml` that does not parse) is asked again at the next
    /// check, so the file is not filed under its directory for the rest of the session.
    project: Option<PathBuf>,
    /// The files its last check reached through `require` (canonical): what a save of any
    /// of them makes it stale for.
    deps: HashSet<PathBuf>,
}

/// What a batch of messages left to check.
#[derive(Default)]
struct Stale {
    /// The files to check, each once, in the order the batch first named them.
    files: Vec<Uri>,
    /// Files (canonical) whose change makes the open files that require them stale: the
    /// ones saved, and the ones opened or closed — an open file reports its own errors,
    /// a closed one is reported through the files that require it, as in an `htl check`
    /// walk of the open files, so which files are open changes what a dependent says.
    changed: Vec<PathBuf>,
}

struct Server<'c> {
    connection: &'c Connection,
    /// By project root; a file in no project is keyed by its directory.
    projects: HashMap<PathBuf, Project>,
    /// By the URI the client used, which is the one a publish has to name.
    open: HashMap<Uri, Doc>,
    /// Set by the `shutdown` request: from then on only `exit` is answered.
    shutting_down: bool,
}

impl<'c> Server<'c> {
    fn new(connection: &'c Connection) -> Self {
        Self {
            connection,
            projects: HashMap::new(),
            open: HashMap::new(),
            shutting_down: false,
        }
    }

    /// Read a batch, apply it, check what it made stale; until `exit`.
    fn run(&mut self) -> Result<ExitCode> {
        loop {
            let Ok(first) = self.connection.receiver.recv() else {
                eprintln!("htl lsp: the client closed the connection without `exit`");
                return Ok(ExitCode::FAILURE);
            };
            // Everything already queued comes with it: a save that was superseded while
            // the last check ran is applied in the same batch as the save that replaced it
            // and checked once.
            let mut batch = vec![first];
            while let Ok(next) = self.connection.receiver.try_recv() {
                batch.push(next);
            }
            let mut stale = Stale::default();
            for message in batch {
                // What the messages before a `shutdown` asked for is done before it is
                // answered: from then on nothing is checked.
                if !self.shutting_down
                    && matches!(&message, Message::Request(r) if r.method == Shutdown::METHOD)
                {
                    self.check_all(&std::mem::take(&mut stale));
                }
                if let Some(code) = self.apply(message, &mut stale) {
                    return Ok(code);
                }
            }
            if !self.shutting_down {
                self.check_all(&stale);
            }
        }
    }

    /// Apply one message. Files to check are added to `stale`; `Some` is the exit code once
    /// `exit` has come.
    fn apply(&mut self, message: Message, stale: &mut Stale) -> Option<ExitCode> {
        match message {
            Message::Request(req) => {
                let response = if req.method == Shutdown::METHOD {
                    self.shutting_down = true;
                    Response::new_ok(req.id, ())
                } else if self.shutting_down {
                    Response::new_err(
                        req.id,
                        ErrorCode::InvalidRequest as i32,
                        "the server is shutting down".to_string(),
                    )
                } else {
                    let what = format!("htl lsp does not answer {}", req.method);
                    Response::new_err(req.id, ErrorCode::MethodNotFound as i32, what)
                };
                self.send(response.into());
            }
            Message::Notification(n) => return self.notification(n, stale),
            // The server sends no request, so there is nothing a response answers.
            Message::Response(_) => {}
        }
        None
    }

    fn notification(&mut self, n: Notification, stale: &mut Stale) -> Option<ExitCode> {
        match n.method.as_str() {
            Exit::METHOD => {
                return Some(if self.shutting_down {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                });
            }
            // Accepted and ignored: see the module doc.
            Cancel::METHOD => {}
            DidOpenTextDocument::METHOD => {
                if let Some(p) = params::<DidOpenTextDocumentParams>(n) {
                    self.opened(p.text_document.uri, stale, false);
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Some(p) = params::<DidSaveTextDocumentParams>(n) {
                    // A save of a file never opened is a file to check like any other.
                    self.opened(p.text_document.uri, stale, true);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Some(p) = params::<DidCloseTextDocumentParams>(n) {
                    let uri = p.text_document.uri;
                    stale.files.retain(|u| *u != uri);
                    if let Some(doc) = self.open.remove(&uri) {
                        stale.changed.push(doc.canonical);
                        self.publish(uri, Vec::new());
                    }
                }
            }
            // Other notifications (`initialized` was consumed by the handshake, the
            // `$/` ones are optional by the protocol) are not this server's.
            _ => {}
        }
        None
    }

    /// `uri` is to be checked, because it was opened or (`saved`) saved: make it an open
    /// document if it is not one yet.
    fn opened(&mut self, uri: Uri, stale: &mut Stale, saved: bool) {
        let Some(path) = path_of(&uri) else {
            eprintln!("htl lsp: not a file: {}", uri.as_str());
            return;
        };
        let newly_open = !self.open.contains_key(&uri);
        if newly_open {
            let project = project_key(&path).ok();
            self.open.insert(
                uri.clone(),
                Doc {
                    canonical: canonical(&path),
                    path,
                    project,
                    deps: HashSet::new(),
                },
            );
        }
        if let Some(doc) = self.open.get(&uri)
            && (saved || newly_open)
        {
            stale.changed.push(doc.canonical.clone());
        }
        if !stale.files.contains(&uri) {
            stale.files.push(uri);
        }
    }

    /// Check what the batch left stale, and the open files that require a file it saved,
    /// opened or closed, each once.
    fn check_all(&mut self, stale: &Stale) {
        let changed: HashSet<PathBuf> = stale.changed.iter().cloned().collect();
        // Who needs a re-check is read before any check runs, from the closures the last
        // checks recorded. In URI order, so that a run is the same run every time.
        let mut dependents: Vec<Uri> = self
            .open
            .iter()
            .filter(|(u, d)| !stale.files.contains(u) && !d.deps.is_disjoint(&changed))
            .map(|(u, _)| u.clone())
            .collect();
        dependents.sort();
        for uri in stale.files.iter().chain(&dependents) {
            self.check_one(uri);
        }
    }

    /// Check one open document and publish what it reports.
    fn check_one(&mut self, uri: &Uri) {
        let open: Vec<PathBuf> = self.open.values().map(|d| d.path.clone()).collect();
        let Some(doc) = self.open.get(uri) else {
            return;
        };
        let (path, known) = (doc.path.clone(), doc.project.clone());
        // Not on disk: nothing has been saved to check, and the checker's answer for a
        // file it cannot open is not a diagnostic (see the module doc).
        if !path.is_file() {
            if let Some(doc) = self.open.get_mut(uri) {
                doc.deps.clear();
            }
            self.publish(uri.clone(), Vec::new());
            return;
        }
        let key = match known {
            Some(key) => key,
            None => match project_key(&path) {
                Ok(key) => {
                    if let Some(doc) = self.open.get_mut(uri) {
                        doc.project = Some(key.clone());
                    }
                    key
                }
                // Asked again next time; the load below says why it failed.
                Err(_) => htl::parent_dir(&path),
            },
        };
        let outcome = self.project(&key, &path).and_then(|p| {
            let (found, deps) = p.check(&path, &open)?;
            Ok((found, deps, p.scope.lints.selection().clone(), p.policy))
        });
        let (diagnostics, deps) = match outcome {
            Ok((found, deps, levels, policy)) => (convert(&path, &found, &levels, &policy), deps),
            // Nothing could be said about the file: say that, on the file, so the editor
            // shows why it has no diagnostics instead of looking clean.
            Err(e) => (vec![failure(&e)], Vec::new()),
        };
        if let Some(doc) = self.open.get_mut(uri) {
            doc.deps = deps.iter().map(|d| canonical(d)).collect();
        }
        self.publish(uri.clone(), diagnostics);
    }

    /// The project under `key`, loaded for `file` if the session has not met it yet. A
    /// project that failed to load is not remembered, so the next save tries again.
    fn project(&mut self, key: &Path, file: &Path) -> Result<&Project> {
        if !self.projects.contains_key(key) {
            let loaded = Project::load(file)?;
            self.projects.insert(key.to_path_buf(), loaded);
        }
        Ok(&self.projects[key])
    }

    fn publish(&self, uri: Uri, diagnostics: Vec<lsp_types::Diagnostic>) {
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            // The text checked is the file's, not a version of a buffer.
            version: None,
        };
        self.send(Notification::new(PublishDiagnostics::METHOD.to_string(), params).into());
    }

    fn send(&self, message: Message) {
        // A closed connection is noticed by the next read.
        let _ = self.connection.sender.send(message);
    }
}

/// A notification's parameters, or a line on stderr saying they were not what the method
/// takes.
fn params<P: serde::de::DeserializeOwned>(n: Notification) -> Option<P> {
    match serde_json::from_value(n.params) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("htl lsp: malformed {}: {e}", n.method);
            None
        }
    }
}

/// The file a `file:` URI names, percent-decoded. `None` for any other scheme.
fn path_of(uri: &Uri) -> Option<PathBuf> {
    url::Url::parse(uri.as_str()).ok()?.to_file_path().ok()
}

/// The `file:` URI of a path, percent-encoded.
fn uri_of(path: &Path) -> Option<Uri> {
    let url = url::Url::from_file_path(path).ok()?;
    Uri::from_str(url.as_str()).ok()
}

/// Which project `file` belongs to: its root, or the file's directory when it is in none.
fn project_key(file: &Path) -> Result<PathBuf> {
    Ok(match htl::model::Project::find_root(file)? {
        Some(found) => found.root,
        None => htl::parent_dir(file),
    })
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The diagnostic for a check that could not be made (an `htl.toml` that does not parse,
/// a toolchain pin this binary does not satisfy): the reason, at the top of the file.
fn failure(e: &anyhow::Error) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: Range::default(),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("htl".to_string()),
        message: format!("{e:#}"),
        ..Default::default()
    }
}

/// `htl check`'s findings for `file`, as the client shows them.
fn convert(
    file: &Path,
    found: &[htl::Diagnostic],
    levels: &htl::lint::Selection,
    policy: &htl::verdict::Policy,
) -> Vec<lsp_types::Diagnostic> {
    // What the positions are read against: the file as the check saw it.
    let text = read_lossy(file);
    let lines = lines_of(&text);
    let cwd = std::env::current_dir().unwrap_or_default();
    let file_canonical = canonical(file);
    found
        .iter()
        .map(|d| {
            // A report spells a path relative to the working directory when it is under
            // it ([`htl::Diagnostic::spelled`]); undone to compare it with the file.
            let at = cwd.join(&d.file);
            let own = d.file.is_empty() || at == file || canonical(&at) == file_canonical;
            let (range, related) = if own {
                (range_at(&lines, d.line, d.col), None)
            } else {
                // A module the file requires: its position is in that module, and the
                // requiring file has nowhere better to show it than its first line. The
                // column is converted against that module's text, as the file's own are.
                let related = uri_of(&at).map(|uri| {
                    let theirs = read_lossy(&at);
                    let start = range_at(&lines_of(&theirs), d.line, d.col).start;
                    vec![DiagnosticRelatedInformation {
                        location: Location {
                            uri,
                            range: Range::new(start, start),
                        },
                        message: d.message.clone(),
                    }]
                });
                (Range::default(), related)
            };
            let message = if own {
                d.message.clone()
            } else {
                format!("{}:{}:{}: {}", d.file, d.line, d.col, d.message)
            };
            lsp_types::Diagnostic {
                range,
                severity: Some(severity(d, levels, policy)),
                code: d.rule.clone().map(NumberOrString::String),
                source: Some("htl".to_string()),
                message,
                related_information: related,
                ..Default::default()
            }
        })
        .collect()
}

/// A file's text for reading positions against; empty when it cannot be read.
fn read_lossy(file: &Path) -> String {
    std::fs::read(file)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// `text`'s lines as the checker numbers them, a `\r` before each `\n` left out.
fn lines_of(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// An error is an error. A Teal warning and an htl lint are warnings, unless they are what
/// makes `htl check` fail ([`htl::verdict::verdict`]): their rule is at `deny`, or the
/// project is `strict` and every finding fails it.
fn severity(
    d: &htl::Diagnostic,
    levels: &htl::lint::Selection,
    policy: &htl::verdict::Policy,
) -> DiagnosticSeverity {
    match d.severity {
        htl::Severity::Error => DiagnosticSeverity::ERROR,
        htl::Severity::Warning | htl::Severity::Lint
            if !policy.capped && (policy.strict || htl::verdict::is_denied(d, levels)) =>
        {
            DiagnosticSeverity::ERROR
        }
        htl::Severity::Warning | htl::Severity::Lint => DiagnosticSeverity::WARNING,
    }
}

/// The range for the point a check reports: `line` counted from 1 and `col` from 1 in
/// bytes (the checker counts bytes), as LSP's 0-based line and UTF-16 column. The range
/// runs to the end of the identifier that starts at the point, or over one character if
/// what is there is not part of one, and is empty at the end of a line. No position
/// (`line` 0) is the start of the file.
fn range_at(lines: &[&str], line: usize, col: usize) -> Range {
    if line == 0 {
        return Range::default();
    }
    let n = (line - 1).min(lines.len().saturating_sub(1));
    let text = lines.get(n).copied().unwrap_or("");
    let mut start = col.saturating_sub(1).min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut end = start;
    let mut rest = text[start..].chars();
    if let Some(first) = rest.next() {
        end += first.len_utf8();
        if word(first) {
            end += rest
                .take_while(|c| word(*c))
                .map(char::len_utf8)
                .sum::<usize>();
        }
    }
    let units = |upto: usize| text[..upto].encode_utf16().count() as u32;
    Range::new(
        Position::new(n as u32, units(start)),
        Position::new(n as u32, units(end)),
    )
}

/// The temp directory the integration tests use, linked in by path as
/// `tests/common/mod.rs` does ([`core_common::tempdir`] says what it promises).
#[cfg(test)]
#[allow(dead_code)]
#[path = "../../htl-core/tests/common/mod.rs"]
mod core_common;

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::Request;
    use std::time::Duration;

    #[test]
    fn a_column_in_bytes_becomes_a_column_in_utf16_units() {
        // `é` is two bytes and one unit, the emoji four bytes and two units: `n` is at
        // byte 26 and unit 23.
        let lines = ["local s = \"é😀\"; local n"];
        let byte = lines[0].rfind('n').unwrap();
        assert_eq!(byte, 26);
        let r = range_at(&lines, 1, byte + 1);
        assert_eq!(r.start, Position::new(0, 23));
        assert_eq!(r.end, Position::new(0, 24));
    }

    #[test]
    fn a_range_covers_the_identifier_that_starts_at_the_point() {
        let lines = ["local name_1 = 3", ""];
        let r = range_at(&lines, 1, 7);
        assert_eq!(
            (r.start, r.end),
            (Position::new(0, 6), Position::new(0, 12))
        );
        // A symbol is one character; the end of a line is empty; no position is 0:0.
        let r = range_at(&lines, 1, 13);
        assert_eq!(
            (r.start, r.end),
            (Position::new(0, 12), Position::new(0, 13))
        );
        let r = range_at(&lines, 2, 1);
        assert_eq!((r.start, r.end), (Position::new(1, 0), Position::new(1, 0)));
        assert_eq!(range_at(&lines, 0, 0), Range::default());
    }

    #[test]
    fn a_column_past_the_line_or_inside_a_character_is_clamped() {
        let lines = ["é"];
        let r = range_at(&lines, 1, 2);
        assert_eq!((r.start, r.end), (Position::new(0, 0), Position::new(0, 1)));
        let r = range_at(&lines, 1, 40);
        assert_eq!((r.start, r.end), (Position::new(0, 1), Position::new(0, 1)));
    }

    #[test]
    fn a_path_and_its_uri_name_each_other() {
        let path = Path::new("/work/my project/é.tl");
        let uri = uri_of(path).unwrap();
        assert_eq!(uri.as_str(), "file:///work/my%20project/%C3%A9.tl");
        assert_eq!(path_of(&uri).as_deref(), Some(path));
        assert_eq!(
            path_of(&Uri::from_str("untitled:Untitled-1").unwrap()),
            None
        );
    }

    fn notify(method: &str, params: serde_json::Value) -> Message {
        Message::Notification(Notification {
            method: method.to_string(),
            params,
        })
    }

    /// Saves of one file that are queued together are one check. The server is started
    /// after the messages are in its channel, so they are one batch however the threads
    /// are scheduled.
    #[test]
    fn saves_queued_together_are_checked_once() {
        let dir = super::core_common::tempdir("htl-cli-lsp", "superseded");
        std::fs::write(dir.join("htl.toml"), "[layout]\nsource = \".\"\n").unwrap();
        std::fs::write(dir.join("a.tl"), "local n: number = \"x\"\nreturn n\n").unwrap();
        let uri = uri_of(&dir.join("a.tl")).unwrap();
        let doc = serde_json::json!({ "textDocument": { "uri": uri.as_str() } });

        let (server_side, client) = Connection::memory();
        let send = |m: Message| client.sender.send(m).unwrap();
        send(Message::Request(Request::new(
            1.into(),
            "initialize".to_string(),
            serde_json::json!({ "capabilities": {} }),
        )));
        send(notify("initialized", serde_json::json!({})));
        send(notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": uri.as_str(), "languageId": "teal", "version": 1, "text": ""
            } }),
        ));
        for _ in 0..3 {
            send(notify("textDocument/didSave", doc.clone()));
        }
        // Started once everything is queued: the open and the three saves are one batch.
        let server = std::thread::spawn(move || run_on(&server_side).unwrap());
        let is_publish = |m: &Message| matches!(m, Message::Notification(n) if n.method == PublishDiagnostics::METHOD);
        // The first publish is waited for; nothing is waiting behind it.
        loop {
            let m = client
                .receiver
                .recv_timeout(Duration::from_secs(60))
                .unwrap();
            if is_publish(&m) {
                break;
            }
        }
        send(Message::Request(Request::new(
            2.into(),
            "shutdown".to_string(),
            serde_json::Value::Null,
        )));
        send(notify("exit", serde_json::Value::Null));
        let code = server.join().unwrap();
        assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::SUCCESS));
        // The server has gone, so anything it was going to say is in the channel: a second
        // check of the file would be a second publish.
        let rest: Vec<Message> = client.receiver.try_iter().collect();
        assert!(!rest.iter().any(is_publish), "{rest:?}");
    }
}

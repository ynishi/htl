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
//! buffer, so what it reports is always about a text that exists as a file. The server does
//! take every change (`textDocumentSync.change` is `Full`) and keeps each open document's
//! buffer, but only to answer formatting, code actions, hover, definition and the outline about
//! the text the client holds; a `didChange` triggers no check. Diagnostics while typing need the checker to answer for
//! text that is not on disk and is often not a program, which is a later step.
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
//! `$/cancelRequest` is accepted and does nothing: a check is triggered by a notification,
//! which has no response to cancel, and the requests it answers (formatting, code actions)
//! are answered on this one thread, so by the time a cancellation is read the request it
//! names has been answered or is the next message.
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
//! # Formatting and code actions
//!
//! A client formats on save, or runs its fixes on save, by asking *before* it writes: the
//! buffer is then not the file, and an edit computed from the file would put the file's text
//! back over everything not yet saved. So the server keeps each open document's buffer
//! (the `didOpen` text, then the text of every full-sync `didChange`) and answers about
//! that. A document the client never opened (a save is all the server heard of it) has its
//! disk for a buffer. The version a request or a change carries is not read: a change is
//! stored as it arrives and the last one is the buffer.
//!
//! Every request is answered about the files as the messages before it left them: a
//! request is preceded by the checks those messages made stale, as a `shutdown` is. (That
//! is true of the requests below; any other is refused as unknown.)
//!
//! Every edit is one `TextEdit` over the span in which the buffer and the new text differ
//! (the text they share at both ends is left out, so the cursor, folds and marks outside it
//! survive); applied, it gives the new text exactly.
//!
//! **Formatting.** `textDocument/formatting` returns the edits that make the file what
//! `htl fmt` writes for the buffer's text ([`Htl::format_text`]), with the indent `htl fmt`
//! resolves for it (`[fmt] indent` of the file's project, else 3; the request's `options`
//! are not read). The answer is `[]` when the buffer is already formatted. A file `htl fmt` cannot format (a parse
//! error) is answered `null`, not an error response: most clients show an error response
//! as a failed command, and a file that does not parse is already a diagnostic. The reason
//! goes to stderr. The file does not have to be open (its disk is the buffer then). `textDocument/rangeFormatting` is
//! not offered: `htl fmt` formats files.
//!
//! **Quick fixes.** A check keeps, with each diagnostic it publishes, the
//! [`htl::Fix`] the checker attached to it, and a `textDocument/codeAction` request is
//! answered from those: nothing is checked again. For every finding whose range touches
//! the request's range and that has a fix, there is a `quickfix` action titled `Fix <rule>`
//! (` (unsafe)` after it for an unsafe fix; `htl fix` prints no description of a fix, so
//! the rule is all there is to name), whose `diagnostics` is that finding and whose edit
//! is the text [`htl::fix::apply_fix`] makes of the buffer with that fix, the code `htl fix`
//! applies its edits with, as a span of the buffer. The action is tied to its diagnostic by the server's own record of
//! which finding it came from, so nothing is matched on range, code and message and
//! `Diagnostic.data` is left empty. `isPreferred` is set for a fix `htl fix` applies without
//! being asked ([`htl::fix::FixOptions::disposition`] is that question, asked of the
//! project's `[fix]`: a `safe` fix, or one `[fix] unsafe` promotes, of a rule `[fix]
//! disable` does not name). An unsafe fix and a disabled one are offered, not preferred (the person
//! asked for it; nothing takes it unasked). A `suggest` fix is not offered: what it writes
//! is a placeholder for the author to replace (`htl_fixme(..)`), which `htl fix` shows
//! and never applies. Only the file's own findings carry a fix: the edit of a finding
//! about a required module is a position in that module.
//!
//! **Fix all.** The action of kind `source.fixAll.htl` (so a client asking for
//! `source.fixAll` gets it) is one edit to what `htl fix <file>` leaves the file as: safe
//! fixes, with the project's `[fix] unsafe` and `[fix] disable`, to a fixpoint of up to
//! [`htl::fix::MAX_PASSES`] passes, reverting what made the file worse. It is computed by
//! [`htl::fix::fix_file`] as a dry run (`htl fix --dry-run`, minus the git guard, which
//! protects a file that is about to be written), in a checker state of its own, because
//! `fix_file` begins with a check that reads the state's store and the shared checker's
//! store must not hold anything (see "Every check is a fresh one"). That costs a check or
//! two of the file, so it is computed only for a request that asks for it (`only` names
//! `source.fixAll`, or the menu was invoked rather than opened by a cursor move), only
//! when a fix `htl fix` applies is on file, and once per check: the answer is kept until
//! the next one. It is offered only when it changes the file.
//!
//! **Only for the text that was checked.** A fix is positions in the text a check read.
//! Quick fixes and fix-all are offered only while the buffer is that text (and, for
//! fix-all, which `fix_file` reads from disk, the disk too); a buffer that has been edited
//! since the last save gets none, because the positions belong to another text. That
//! includes the request a client makes on save before it writes: formatting works on the
//! buffer and is the part that can be done then; fixes follow once the save has been
//! checked. A `[fix]` section `htl fix` would refuse (a rule that is none) is published
//! with the file's diagnostics as the setup error it is, and no fix is offered while it
//! stands.
//!
//! # Hover and definition
//!
//! Both are read off the check that was made of the file, as quick fixes are: the check is
//! [`Htl::check_indexed`], a [`Htl::check_written`] that also copies out of the checker's type
//! report what an editor asks about a position ([`htl::typeindex`]; its module doc says what
//! a question is and what the report cannot answer). The copy is made once per check and no
//! check is made for a request. It is one pass over the report's positions, small beside
//! the check it belongs to (`HTL_PROFILE=1` prints it as `type_index`, beside `check`),
//! which is why it is made on every check and not on the first request: a request would
//! otherwise be a check of the file, which is what the server avoids by checking on open
//! and on save.
//!
//! **Hover** is the type of the name under the cursor, as Teal writes it
//! (`function(string): sample.Greeting`), as a `teal` code block, over the range of the
//! name. The cursor may be anywhere in the name or just after it. `null`, not an error, for
//! anything that is not a name — whitespace, a bracket, a keyword, a number, the inside of a
//! comment or of a string — and for a name the checker has no type for.
//!
//! **Definition** is, for the string of a literal `require("m")` (the quotes, and just after
//! the closing one), the file htl's resolution picked for it — the same
//! [`RequireSite::path`](htl::RequireSite::path) `htl resolve m` explains — at its first
//! character. For a name, it is the declaration the report names: a local, a parameter or a
//! local function or type in scope at the cursor, or, for a function reached through a field
//! (`m.hello`), the function in the file that declares it. What the report cannot name is
//! answered `null`: a field of a primitive type (`g.text`, which Teal shares one `string` for),
//! a name of the standard library (its declarations are in a file Teal carries inside itself,
//! not one on disk), a name the report has nothing for. The location is a point: the line and
//! column Teal records (the name for a variable or a parameter, the start of the statement for a
//! function or a record), converted to LSP's UTF-16 column against the declaring file as it is
//! on disk now.
//!
//! **Only for the text that was checked.** Both are positions in the text the check read, so
//! they are answered only while the client's buffer is that text; a buffer that has been
//! edited since the last save gets `null` from both, not an answer about a text it no
//! longer holds. Both are answered again after the next save is checked, or if the edit is
//! undone. (Diagnostics while typing, which would let them follow the buffer, are a later
//! step.)
//!
//! # Outline
//!
//! `textDocument/documentSymbol` lists what a file declares, as a tree of `DocumentSymbol`s,
//! from the same index ([`htl::typeindex::TypeIndex::declarations`]): the declarations of the
//! file's top level, and for a record its fields, the types nested in it and the functions
//! declared on it anywhere in the file. A module record's `function M.f` is written after the
//! record's `end` and is its child all the same, so a child's range may lie outside its
//! parent's. The kinds: a record is a `Struct`, an interface an `Interface`, an enum an `Enum`,
//! a type alias a `TypeParameter`, a variable a `Variable` (a `Function` if it holds one), a
//! field a `Field`; `function R.f` and a field typed as a function are `Function`s and
//! `function R:m` a `Method`. `detail` is the type as Teal writes it. `range` runs from the
//! first token of the declaration to its `end` where the checker records one and otherwise
//! to the end of the line the declaration starts on (the index's module doc lists which is
//! which), and `selectionRange` is the name. Columns are UTF-16 units.
//!
//! It is answered about the text that was checked, like hover and definition: `null` for a
//! buffer that has been edited since, and also `null`, never `[]`, for a file the server
//! cannot say anything about. That is a file the client has not opened (nothing has been
//! checked for it; the server does not check one for a request), one not on disk, and one
//! that does not parse (the checker keeps no tree for it). `[]` is what a file that parses
//! and declares nothing gets. An open file whose check found type errors still has its
//! outline, as it still has hover.
//!
//! # Workspace symbols
//!
//! The workspace-symbol request searches the declarations of the whole
//! project by name, so a client finds `greet` without knowing which file declares it. The
//! answer is a flat list of `SymbolInformation`: the name (`R.f` for a record function whose
//! record is declared in another file), the kind (the outline's), the file and the range of
//! the name, and as `containerName` the record a field or a function is declared on.
//!
//! **What is indexed.** The `.tl` files `htl check <root>` walks for the project: the
//! project's own sources, its tests and the modules it patches, not an installed or vendored
//! dependency, and no file that no module of the project holds (the walk is
//! [`Project::source_files`], made of the calls `htl check` makes, so the two cannot
//! disagree). For each file, the tree of declarations the type index keeps for an open file
//! ([`htl::typeindex::TypeIndex::declarations`]), flattened. A file that is not open has no
//! check, and is not checked for this: [`Htl::declarations_of`] parses it and walks its
//! top-level statements, which costs a parse of the file and nothing of what it requires
//! (no type check, no `require` followed). What that
//! leaves out is the type strings, which this answer has no field for, and one kind: a
//! variable that holds a function it did not write (`local f = m.hello`) is a `Variable`
//! until a check says it is a function. A file that does not parse has no declarations,
//! and is not an error: its syntax error is a diagnostic of its own. A file with a type
//! error is listed.
//!
//! **Which projects.** Those of the open files, so a request about a file of one project is
//! not an answer about another; when no file is open, the projects of the workspace folders
//! the client named at `initialize`. A directory that is no project (a file opened outside any
//! `htl.toml`) has its open files and nothing else, for there is no walk to make. The index
//! of a project is built at the first request that asks about it, not before, so a session
//! that never searches pays nothing.
//!
//! **Kept current.** Each file's entry holds the stamp (modification time and length) of the
//! file and a hash of the text its symbols were read from; an entry is replaced only when the
//! text moved, so a refresh parses the files that changed and no others.
//!
//! - A check of an open file (open, save, or a dependency's save) puts the declarations of
//!   that check in as the file's entry: fresher than a read of the disk, and kinds the types
//!   decide. A file whose last check read the text now on disk gets its entry from that check
//!   when the index is built, too.
//! - A client that watches files (the registration was accepted) says when a `.tl` under the
//!   project is made, changed or deleted (`**/*.tl` is in the registration). A file the
//!   index holds is read again at the next request, or dropped if it is gone; a file it does
//!   not hold makes the next request walk the project again. A request answered between
//!   events reads no file, and stats only the files its hits are in: a watcher names files,
//!   so a directory deleted or renamed is one event the glob does not match, and a hit in a
//!   file that is gone makes the project walked again and the search made once more.
//! - A client that does not (it never said it could, or refused) is not told, and the index
//!   cannot know: before each answer the project is walked again and every file stamped, and a
//!   file whose stamp moved, one that appeared and one that went are put right. That is a
//!   directory walk and a `stat` per file on every request, however large the project, and
//!   nothing is read or parsed that did not change: the price of an answer that is not stale.
//!   A stamp is not proof of the text: one write can land in the same tick of the file
//!   system's clock as the one before and leave the same length, so a file written in the
//!   last two seconds is read and compared with the text its entry came from, however its
//!   stamp looks (git's index treats such "racily clean" entries alike). A directory that is
//!   no project is listed this way whether or not the client watches, since which files are
//!   open is what lists it and no watcher says that.
//! - A project that is dropped because a manifest changed ("Files that change the answer")
//!   takes its index with it, and the next request builds it again. One dropped for a
//!   declaration under `types/` or a Rust source keeps it: what a file declares is its
//!   syntax, which neither changes.
//!
//! The index is of what is on disk, or what the last check read. The text of an unsaved
//! buffer is not in it, as it is in no diagnostic.
//!
//! **The bound.** At most [`symbols::MAX_RESULTS`] symbols are answered (1000), because the
//! protocol has no paging for this request. The order is by how well the name matches, the
//! query being a case-insensitive substring: the name itself first (`greet`, `GREET`), then
//! names that start with the query, then names that hold it; inside each by path, line and
//! column, so the same query on the same files is the same answer. A project with more
//! matches than the bound gets the best of them and a line on stderr says how many there
//! were; a client that wants one of the rest narrows the query. An empty query matches
//! everything and so lists the first thousand by path. The match is on the name as written
//! (`f` of an `R.f`, and the whole `R.f` too).
//!
//! **`null`, not `[]`.** `[]` is "an index was built and nothing in it matches". `null` is
//! "there is no index to ask": no file is open and the workspace folders are in no project,
//! or the project cannot be loaded, or `htl check` would refuse to walk it (a flat layout
//! without `[layout] source = "."`). The reason is on stderr. An empty answer a client reads
//! as "there is none" is the failure this distinction is for.
//!
//! # Files that change the answer
//!
//! A file's diagnostics depend on more than the file: the project's `htl.toml` (lint levels,
//! the resolution rules, `[fix]`, `[fmt]`), its `mlua-pkg.toml` and `mlua-pkg.lock` (which
//! modules a `require` can reach), the declarations under `types/`, and the Rust that `htl
//! dts` writes declarations from. A [`Project`] is built from those once, so a change to any
//! of them has to drop it, and the open files of the project have to be checked again —
//! without being saved, since it is not they that changed. There are two ways the server
//! learns of it, and either alone is enough for the manifests.
//!
//! **The client says so** (the `didChangeWatchedFiles` notification). If the client's `initialize`
//! says it takes a dynamic registration for it, the server sends one
//! `client/registerCapability` request after the handshake, with these globs ([`WATCHED`]):
//! `**/htl.toml`, `**/mlua-pkg.toml`, `**/mlua-pkg.lock`, `**/types/**/*.d.tl`,
//! `**/*.tl` (the symbol index's, see "Workspace symbols"), `**/Cargo.toml` and
//! `**/{src,examples,tests,benches}/**/*.rs`. The last two are what
//! `htl dts` reads ([`htl::dts::generate_crate_to`] walks those four directories under the
//! crate's `Cargo.toml`), and nothing else is registered, so a client does not watch every
//! `.rs` of a workspace (nor the `target/` that holds most of them). A change to a file that
//! `htl dts` would not read is dropped on arrival ([`Watched::reads_rust`]), for a client that
//! cannot narrow its globs. Which projects a file reaches:
//!
//! - a manifest: the projects whose root is under its directory or above it, and every open
//!   file under that directory, which has its project looked up again (a new `htl.toml` in a
//!   subdirectory makes a project of the files under it);
//! - a `.d.tl` under a `types` directory: the projects it is in (the model scans `types/` when
//!   it is built, so a declaration that appears or goes is as much a change as one that is
//!   edited; a project whose `[layout] types` is elsewhere is reached by the fallback below
//!   if at all);
//! - Rust: the projects whose crate `htl dts` reads that file for. They are rebuilt, which
//!   runs `htl dts` again, and a `.d.tl` it writes is another event and another rebuild that
//!   writes nothing (it writes a file only when it differs), so it settles at once.
//!
//! The events of a batch drop projects and nothing else, so a hundred events for one project
//! (a branch switch) are one rebuild, made by the checks the batch leads to; the open files of
//! the project are each checked once.
//!
//! **The server looks** (the fallback, and what makes this work on a client that does not
//! watch files, or refuses the registration — which is said on stderr). Before it checks
//! the files of a project, and before it formats one, it reads the project's `htl.toml`,
//! `mlua-pkg.toml` and `mlua-pkg.lock` and compares them with what the project was built
//! from ([`Manifests`]); if any differs, the project is rebuilt, and every open file of it is
//! checked, not only the one that asked. The cost is reading three small files per check (a
//! few kilobytes, microseconds beside a check). It sees a manifest at the next open or save
//! of any file of the project, not at the moment it is edited: a client that does not watch
//! files has nothing to say when it changes. It does not see `types/` or Rust, which are not
//! the project's own files at its root; a client that does not watch files has those at the
//! next time the project is rebuilt for some other reason.
//!
//! # What it does not do yet
//!
//! The findings no single file carries (a require cycle, a contract no host enforces) are not
//! reported: they need the whole walk, and the scope here is the open files and what they
//! require. A `---@contract` type, which `htl dts` publishes as a declaration when the project
//! is built, is published again only when it is: a save of the `.tl` that declares it does not
//! rebuild the project.

mod symbols;

use super::{auto_dts, load_config, warn_cargo_htl_mismatch};
use anyhow::{Context, Result};
use htl::Htl;
use htl::project::{self, Collect, Sink};
use lsp_server::{Connection, ErrorCode, Message, Notification, RequestId, Response};
use lsp_types::notification::{
    Cancel, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, DidSaveTextDocument, Exit, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    RegisterCapability, Request as _, Shutdown, WorkspaceSymbolRequest,
};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionTriggerKind, DiagnosticRelatedInformation,
    DiagnosticSeverity, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams, DocumentSymbol,
    DocumentSymbolParams, DocumentSymbolResponse, FileSystemWatcher, GlobPattern,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, Location, MarkupContent, MarkupKind, NumberOrString, OneOf, Position,
    PublishDiagnosticsParams, Range, Registration, RegistrationParams, SaveOptions,
    ServerCapabilities, SymbolInformation, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use symbols::{Entry, Stamp, SymbolIndex};

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
    let (id, params) = connection
        .initialize_start()
        .context("waiting for the client's initialize request")?;
    // A client that can take a registration at run time says so; one that cannot is not
    // sent a request it would answer with an error (module doc, "Files that change the
    // answer").
    let dynamic_watch = params
        .pointer("/capabilities/workspace/didChangeWatchedFiles/dynamicRegistration")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let capabilities = ServerCapabilities {
        // Open, close and save to know when to check; the text of every change to answer
        // formatting about the buffer. A change is stored and not checked (module doc).
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                will_save: None,
                will_save_wait_until: None,
            },
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX, fix_all_kind()]),
            work_done_progress_options: Default::default(),
            resolve_provider: Some(false),
        })),
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
    Server::new(connection, dynamic_watch, workspace_of(&params)).run()
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
    /// What `htl fix` with no flags would do in this project: safe fixes only, the rules
    /// `[fix] unsafe` promotes counted as safe, the ones `[fix] disable` names left alone.
    /// A dry run, since the server never writes.
    fix: htl::fix::FixOptions,
    /// Why `fix` cannot be trusted ([`htl::fix::FixOptions::validate`]: `htl fix` refuses
    /// such a configuration outright), said with every check of the project. While it is
    /// set no fix is offered: which fixes `htl fix` would apply is not a question with an
    /// answer.
    fix_error: Option<String>,
    /// The manifest files as they were when the rest was built from them, to tell whether
    /// they still are (module doc, "Files that change the answer").
    manifests: Manifests,
}

/// The files at a project's root that decide what a [`Project`] is, and a hash of each as
/// it was read: `htl.toml` (lint levels, resolution, `[fix]`, `[fmt]`, the toolchain pin),
/// `mlua-pkg.toml` (the dependencies and so the modules a name resolves to) and
/// `mlua-pkg.lock` (which version of them).
///
/// Compared by content and not by modification time: they are a few kilobytes, reading
/// three of them is what a check spends nothing on, and a content hash has no clock
/// granularity to be wrong about (an edit that keeps the size and lands in the same tick of
/// a coarse file system is still seen).
struct Manifests {
    seen: Vec<(PathBuf, Option<u64>)>,
}

impl Manifests {
    /// The names, as the other crates spell them.
    fn names() -> [&'static str; 3] {
        [
            htl::config::CONFIG_NAME,
            htl::pkg::MANIFEST_NAME,
            htl::pkg::LOCKFILE_NAME,
        ]
    }

    fn read(root: &Path) -> Self {
        let seen = Self::names()
            .iter()
            .map(|n| {
                let p = root.join(n);
                let h = hash_of(&p);
                (p, h)
            })
            .collect();
        Self { seen }
    }

    /// Whether any of them reads differently now (or has appeared or gone).
    fn changed(&self) -> bool {
        self.seen.iter().any(|(p, h)| hash_of(p) != *h)
    }
}

/// A hash of the file's bytes; `None` when it cannot be read (no such file).
fn hash_of(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    Some(h.finish())
}

impl Project {
    /// The project `file` belongs to (`root` is its key), set up as `htl check` sets one
    /// up: config, then the declarations generated from Rust, then the model, the lint
    /// scope, the checker.
    fn load(file: &Path, root: &Path) -> Result<Self> {
        // Taken before anything is read: an edit that lands while the project loads then
        // leaves the manifests unlike this, and the next check loads again.
        let manifests = Manifests::read(root);
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
        let fix = htl::fix::FixOptions {
            unsafe_fixes: false,
            promoted: cfg
                .as_ref()
                .map(|(_, _, c)| c.fix.unsafe_.clone())
                .unwrap_or_default(),
            disabled: cfg
                .as_ref()
                .map(|(_, _, c)| c.fix.disable.clone())
                .unwrap_or_default(),
            only: Vec::new(),
            dry_run: true,
        };
        let fix_error = fix.validate().err().map(|e| format!("{e:#}"));
        Ok(Self {
            cfg,
            model,
            scope,
            origins,
            policy,
            checker,
            fix,
            fix_error,
            manifests,
        })
    }

    /// `text`, the contents of `file`, as `htl fmt` would write it: the project's
    /// `[fmt] indent`, or 3.
    fn format(&self, file: &Path, text: &str) -> Result<String> {
        let indent = self
            .cfg
            .as_ref()
            .and_then(|(_, _, c)| c.fmt.indent)
            .unwrap_or(3);
        self.checker.format_text(text, file, indent)
    }

    /// The `.tl` files the symbol index covers: the ones `htl check` walks for the project
    /// (`htl check <root>`: the walk of the project root, without the dependencies the model
    /// says are not the project's own, and keeping what a module of the project holds), so the
    /// index and the checker never disagree about which files are the project's. A project
    /// `htl check` refuses to walk (one laid out flat without `[layout] source = "."`) is an
    /// error here too.
    ///
    /// A directory that is no project has no walk to answer with: its files are the open ones
    /// (`open`, the paths of the open files of that directory).
    fn source_files(&self, root: &Path, open: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let Some(model) = self.model.as_ref() else {
            return Ok(open.to_vec());
        };
        let paths = [root.to_path_buf()];
        let skip = project::not_walked(Some(model), &paths, htl::model::Purpose::Check);
        let (files, _outside) = project::held_by_modules(
            Some(model),
            &paths,
            htl::collect_tl_skipping(&paths, &skip)?,
        )?;
        Ok(files)
    }

    /// The declarations of `path`, whose text is `text`, as flat symbols: from the last check
    /// of the file if it is open and that check read this text, else from the parse of the
    /// file that [`Htl::declarations_of`] makes. A file that does not parse has none.
    fn symbols_of(
        &self,
        open: &[&Doc],
        canonical: &Path,
        path: &Path,
        text: &str,
    ) -> Vec<symbols::Sym> {
        let checked = open
            .iter()
            .find(|d| d.canonical == canonical)
            .and_then(|d| d.checked.as_ref())
            .filter(|c| c.text == text);
        if let Some(decls) = checked.and_then(|c| c.types.declarations()) {
            return symbols::flatten(text, decls);
        }
        match self.checker.declarations_of(path) {
            Ok(Some(decls)) => symbols::flatten(text, &decls),
            Ok(None) => Vec::new(),
            Err(e) => {
                eprintln!("htl lsp: no symbols for {}: {e:#}", path.display());
                Vec::new()
            }
        }
    }

    /// `file` as `htl fix <file>` would leave it, or `None` when it would not change it.
    ///
    /// A dry run of [`htl::fix::fix_file`] (which checks, applies, re-checks and reverts what
    /// made the file worse, exactly as the command does) in a state of its own. The shared
    /// [`checker`](Self::checker) cannot be used: `fix_file` begins with [`Htl::check`],
    /// which serves a module from the state's store, and a store that outlives an edit
    /// describes the file as it was. A new state has an empty store, so that first check is
    /// of what is on disk now. It costs what a first check costs, which is why the answer is
    /// kept until the next check of the file ([`Checked::fix_all`]).
    fn fixed(&self, file: &Path) -> Result<Option<String>> {
        let h = project::checker(
            self.model.as_ref(),
            self.scope.lints.selection(),
            &project::lang_of(&self.cfg),
        )?;
        project::file_view(&h, self.model.as_ref(), file)?;
        Ok(htl::fix::fix_file(&h, file, &self.fix)?.contents)
    }

    /// Check `file` as it is on disk, the way `htl check` reports it: everything the file
    /// and this layer have to say about it, and the files its check reached.
    ///
    /// `open` is every file the client has open, which the sink treats as walked: a
    /// dependency error in one of them is that file's to report, as it is for the files
    /// of an `htl check` walk.
    fn check(&self, file: &Path, open: &[PathBuf]) -> Result<Done> {
        let mut sink = Sink::new(Collect::default());
        sink.walking(open);
        let walk = self
            .scope
            .walk(&self.cfg, self.model.as_ref(), &self.origins);
        // The per-file question `htl check`'s walk asks, from disk, keeping what the
        // checker knows of the file's names for hover and definition.
        let (c, _) = project::check_file(
            &self.checker,
            &mut sink,
            file,
            &walk,
            project::Fresh::DiskIndexed,
        )?;
        Ok(Done {
            found: sink.out().take(),
            deps: c.deps,
            requires: c.requires,
            types: c.types,
        })
    }
}

/// What [`Project::check`] made of a file.
struct Done {
    found: Vec<htl::Diagnostic>,
    /// The files its check reached through `require`.
    deps: Vec<PathBuf>,
    /// Its literal `require`s and the files they resolved to.
    requires: Vec<htl::RequireSite>,
    /// What the checker knows of its names, by position.
    types: htl::typeindex::TypeIndex,
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
    /// The text the client holds: what it opened the file with, then what its last
    /// `didChange` said. `None` for a file known only from a save, whose buffer is then
    /// the disk. It is what formatting formats and what an edit is computed against; a
    /// check never reads it.
    buffer: Option<String>,
    /// Set when the client sent a change this server cannot apply (an incremental one): the
    /// client holds a text the server does not know, so neither `buffer` nor the disk is
    /// what an edit may be computed against, and formatting and code actions answer nothing
    /// until a `didOpen` or a full change says what the text is again.
    unsynced: bool,
    /// What the last check of it found, as it was found; `None` before the first check and
    /// for a file that was not on disk.
    checked: Option<Checked>,
}

/// One check's result for a document, kept for the requests that follow it (a code action
/// is answered from this, not from a check of its own).
struct Checked {
    /// The file's text when it was checked. A fix is a set of positions in it, so a request
    /// offers one only while the buffer is still this text, and is answered with nothing
    /// rather than with edits for a text that is gone.
    text: String,
    findings: Vec<Finding>,
    /// The file's literal `require`s and where htl's resolution put each: what go-to-definition
    /// on a `require`'s string answers.
    requires: Vec<htl::RequireSite>,
    /// What the checker knows of the names in `text`, by position: what hover and
    /// go-to-definition answer.
    types: htl::typeindex::TypeIndex,
    /// What `htl fix` would make of the file: not asked for until a request wants it, and
    /// asked once (`Some(None)` is "asked, and it changes nothing").
    fix_all: Option<Option<String>>,
}

/// A diagnostic as published, with the fix the checker attached to it.
struct Finding {
    lsp: lsp_types::Diagnostic,
    /// Present only for a finding in the file itself: the edits of a fix are positions in
    /// the file it is attached to, and a finding about a required module is shown here
    /// but is not this file's to rewrite.
    fix: Option<htl::Fix>,
    /// What `htl fix --rule` and `[fix]` name the fix by.
    rule: String,
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
    /// Whether the client takes a `client/registerCapability` for file watchers.
    dynamic_watch: bool,
    /// Whether the client took the registration: only then are the `didChangeWatchedFiles`
    /// notifications taken to say every change to a `.tl` (module doc, "Workspace symbols").
    watching: bool,
    /// The folders the client opened the workspace on, for a symbol search made before any
    /// file is open.
    workspace: Vec<PathBuf>,
    /// The symbols of each project a workspace-symbol request has asked about, by project
    /// root. A project dropped because a manifest moved takes its index with it; one dropped
    /// for a declaration or a Rust source keeps it (what a file declares does not depend on
    /// either), and is loaded again by the next request.
    indices: HashMap<PathBuf, SymbolIndex>,
}

/// The id of the one request the server sends, so that its response is known when it comes.
const REGISTER_ID: &str = "htl-watched-files";

/// What the client is asked to watch (module doc, "Files that change the answer").
const WATCHED: [&str; 7] = [
    "**/htl.toml",
    "**/mlua-pkg.toml",
    "**/mlua-pkg.lock",
    "**/types/**/*.d.tl",
    // The sources whose declarations the workspace-symbol request lists.
    "**/*.tl",
    // What `htl dts` reads to write declarations: a crate's manifest and the Rust under the
    // four directories it walks ([`htl::dts::generate_crate_to`]), nowhere else.
    "**/Cargo.toml",
    "**/{src,examples,tests,benches}/**/*.rs",
];

impl<'c> Server<'c> {
    fn new(connection: &'c Connection, dynamic_watch: bool, workspace: Vec<PathBuf>) -> Self {
        Self {
            connection,
            projects: HashMap::new(),
            open: HashMap::new(),
            shutting_down: false,
            dynamic_watch,
            watching: false,
            workspace,
            indices: HashMap::new(),
        }
    }

    /// Ask the client to tell us when the files in [`WATCHED`] change. The answer comes back
    /// as a `Message::Response` ([`Self::apply`]).
    fn register_watchers(&self) {
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: WATCHED
                .iter()
                .map(|g| FileSystemWatcher {
                    glob_pattern: GlobPattern::String((*g).to_string()),
                    // Create, change and delete: a manifest that appears or goes changes the
                    // answer as much as one that is edited.
                    kind: None,
                })
                .collect(),
        };
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: REGISTER_ID.to_string(),
                method: DidChangeWatchedFiles::METHOD.to_string(),
                register_options: serde_json::to_value(options).ok(),
            }],
        };
        let request = lsp_server::Request::new(
            RequestId::from(REGISTER_ID.to_string()),
            RegisterCapability::METHOD.to_string(),
            params,
        );
        self.send(request.into());
    }

    /// Read a batch, apply it, check what it made stale; until `exit`.
    fn run(&mut self) -> Result<ExitCode> {
        if self.dynamic_watch {
            self.register_watchers();
        }
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
                } else if [
                    Formatting::METHOD,
                    CodeActionRequest::METHOD,
                    HoverRequest::METHOD,
                    GotoDefinition::METHOD,
                    DocumentSymbolRequest::METHOD,
                    WorkspaceSymbolRequest::METHOD,
                ]
                .contains(&req.method.as_str())
                {
                    // Answered about the files as the messages before it left them: what
                    // they made stale is checked first, as before a `shutdown`. A code
                    // action, a hover, a definition and an outline are read off the last
                    // check, so one asked right behind a save would otherwise describe the
                    // file as it was before.
                    self.check_all(&std::mem::take(stale));
                    self.document_request(req)
                } else {
                    let what = format!("htl lsp does not answer {}", req.method);
                    Response::new_err(req.id, ErrorCode::MethodNotFound as i32, what)
                };
                self.send(response.into());
            }
            Message::Notification(n) => return self.notification(n, stale),
            // The only request the server sends is the watcher registration. A client that
            // refuses it still gets the fallback (module doc); the refusal is said.
            Message::Response(r) => {
                if r.id == RequestId::from(REGISTER_ID.to_string()) {
                    match r.response_result {
                        Ok(_) => self.watching = true,
                        Err(e) => eprintln!(
                            "htl lsp: the client refused to watch files ({}): {}; the manifests \
                             are compared at each check instead",
                            e.code, e.message
                        ),
                    }
                }
            }
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
                    let d = p.text_document;
                    self.opened(d.uri, stale, false, Some(d.text));
                }
            }
            // Stored, not checked: the diagnostics are about what is saved (module doc).
            DidChangeTextDocument::METHOD => {
                if let Some(p) = params::<DidChangeTextDocumentParams>(n) {
                    self.changed(&p.text_document.uri, p.content_changes);
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Some(p) = params::<DidSaveTextDocumentParams>(n) {
                    // A save of a file never opened is a file to check like any other.
                    self.opened(p.text_document.uri, stale, true, None);
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
            DidChangeWatchedFiles::METHOD => {
                if let Some(p) = params::<DidChangeWatchedFilesParams>(n) {
                    let paths: Vec<PathBuf> =
                        p.changes.iter().filter_map(|c| path_of(&c.uri)).collect();
                    self.watched(&paths, stale);
                }
            }
            // Other notifications (`initialized` was consumed by the handshake, the
            // `$/` ones are optional by the protocol) are not this server's.
            _ => {}
        }
        None
    }

    /// A `didChange`: the buffer is now the text the (full-sync) change carries. A change
    /// for a document that is not open is a client that has not said it opened it, and is
    /// not this server's to keep.
    fn changed(&mut self, uri: &Uri, changes: Vec<lsp_types::TextDocumentContentChangeEvent>) {
        // Full sync: the last event is the whole text. One with a range is a client that
        // did not take the capability, and is not applied to a text it does not describe.
        let Some(last) = changes.into_iter().next_back() else {
            return;
        };
        if let Some(doc) = self.open.get_mut(uri) {
            if last.range.is_none() {
                doc.buffer = Some(last.text);
                doc.unsynced = false;
            } else {
                // Falling back to the disk here would hand out edits for the saved text over
                // an unsaved buffer: nothing is answered until the text is known again.
                eprintln!("htl lsp: an incremental change, which was not asked for; ignored");
                doc.buffer = None;
                doc.unsynced = true;
            }
        }
    }

    /// `uri` is to be checked, because it was opened (with `text`) or (`saved`) saved: make
    /// it an open document if it is not one yet.
    fn opened(&mut self, uri: Uri, stale: &mut Stale, saved: bool, text: Option<String>) {
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
                    buffer: None,
                    unsynced: false,
                    checked: None,
                },
            );
        }
        if let Some(text) = text
            && let Some(doc) = self.open.get_mut(&uri)
        {
            doc.buffer = Some(text);
            doc.unsynced = false;
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

    /// Files the client says changed: drop the projects they belong to, so the next check
    /// builds them again, and make the open files of those projects stale.
    ///
    /// Dropping is all that is done here, so any number of events for one project (a `git
    /// checkout` touches hundreds of files) is one rebuild, at the check that follows the
    /// batch. Which projects and which files each kind of change reaches is [`Watched`].
    fn watched(&mut self, paths: &[PathBuf], stale: &mut Stale) {
        let mut gone: Vec<PathBuf> = Vec::new();
        // The projects a manifest drops: the only ones whose symbol index goes with them.
        let mut gone_manifest: Vec<PathBuf> = Vec::new();
        let mut manifest_dirs: Vec<PathBuf> = Vec::new();
        let mut hit: Vec<Uri> = Vec::new();
        // Where each open document's crate is, asked once per document for the batch.
        let mut crates: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        // A project's root and a crate's are found by walking up from a canonical
        // directory, and a key outside any project is the file's directory as the client
        // named it; the paths of the events are the client's. Everything is compared in
        // canonical form, or a project under a symlinked directory is reached by nothing.
        let mut keys: HashMap<PathBuf, PathBuf> = HashMap::new();
        let mut canonical_key = |k: &Path| -> PathBuf {
            keys.entry(k.to_path_buf())
                .or_insert_with(|| canonical(k))
                .clone()
        };
        for path in paths {
            // A `.tl` is the symbol index's, and nothing else's: its check is the save's.
            if is_tl(path) {
                let path = canonical_dir_of(path);
                for (root, index) in &mut self.indices {
                    if path.starts_with(canonical_key(root)) {
                        index.event(&path);
                    }
                }
            }
            let Some(kind) = Watched::of(path) else {
                continue;
            };
            let path = canonical_dir_of(path);
            let dir = htl::parent_dir(&path);
            if kind == Watched::Manifest {
                manifest_dirs.push(dir.clone());
            }
            for (uri, doc) in &mut self.open {
                let key = doc.project.clone();
                let reached = match kind {
                    // A manifest decides the project of every file under its directory,
                    // including one that has none yet or whose project is another: the
                    // walk up is asked again for those. A project whose root is on the
                    // way up from the manifest (or under it) is rebuilt.
                    Watched::Manifest => {
                        let under = doc.canonical.starts_with(&dir) || doc.path.starts_with(&dir);
                        if under {
                            doc.project = None;
                        }
                        under
                            || key.as_ref().is_some_and(|k| {
                                let k = canonical_key(k);
                                k.starts_with(&dir) || dir.starts_with(&k)
                            })
                    }
                    Watched::Types => key
                        .as_ref()
                        .is_some_and(|k| path.starts_with(canonical_key(k))),
                    Watched::Rust => crates
                        .entry(doc.path.clone())
                        .or_insert_with(|| htl::dts::find_cargo_package_root(&doc.path))
                        .as_deref()
                        .is_some_and(|c| Watched::reads_rust(&path, c)),
                };
                if reached {
                    if kind == Watched::Manifest {
                        gone_manifest.extend(key.clone());
                    }
                    gone.extend(key);
                    if !hit.contains(uri) {
                        hit.push(uri.clone());
                    }
                }
            }
        }
        self.projects.retain(|k, _| {
            let ck = canonical_key(k);
            !gone.contains(k)
                && !manifest_dirs
                    .iter()
                    .any(|d| ck.starts_with(d) || d.starts_with(&ck))
        });
        // A declaration under `types/` or a Rust source changes what a check says, not what a
        // file declares or which files the walk visits: the index is kept, and the project
        // is loaded again by the next request that asks it. A manifest decides the walk and
        // `[lang]`, so the index of a project it reaches goes with the project.
        self.indices.retain(|k, _| {
            let ck = canonical_key(k);
            !gone_manifest.contains(k)
                && !manifest_dirs
                    .iter()
                    .any(|d| ck.starts_with(d) || d.starts_with(&ck))
        });
        hit.sort();
        for uri in hit {
            if !stale.files.contains(&uri) {
                stale.files.push(uri);
            }
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
        let mut files: Vec<Uri> = stale.files.iter().chain(&dependents).cloned().collect();
        // A document whose walk up has not answered yet is asked here, so that the project
        // its check will use is among the ones compared.
        let keys: HashSet<PathBuf> = files
            .iter()
            .filter_map(|u| {
                let d = self.open.get(u)?;
                d.project.clone().or_else(|| project_key(&d.path).ok())
            })
            .collect();
        for uri in self.drop_moved(keys) {
            if !files.contains(&uri) {
                files.push(uri);
            }
        }
        for uri in &files {
            self.check_one(uri);
        }
    }

    /// The projects among `keys` whose manifests read differently than when they were
    /// built (module doc, "Files that change the answer"): dropped, so that the next use
    /// builds them again, and their open files returned in URI order, since what such a
    /// project says changes for every open file of it and not only for the one that asked.
    fn drop_moved(&mut self, keys: impl IntoIterator<Item = PathBuf>) -> Vec<Uri> {
        let moved: Vec<PathBuf> = keys
            .into_iter()
            .filter(|k| self.projects.get(k).is_some_and(|p| p.manifests.changed()))
            .collect();
        if moved.is_empty() {
            return Vec::new();
        }
        for k in &moved {
            self.projects.remove(k);
            self.indices.remove(k);
        }
        let mut files: Vec<Uri> = self
            .open
            .iter()
            .filter(|(_, d)| d.project.as_ref().is_some_and(|k| moved.contains(k)))
            .map(|(u, _)| u.clone())
            .collect();
        files.sort();
        files
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
                doc.checked = None;
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
        // Read before the check, and again after it (below): a write that lands while the
        // check runs then leaves the disk unlike `text`, and the check's fixes are dropped,
        // where a text read only after would match the disk and hand out the old text's
        // fixes.
        let stamp = Stamp::of(&path);
        let text = read_lossy(&path);
        let outcome = self.project(&key, &path).and_then(|p| {
            let done = p.check(&path, &open)?;
            Ok((
                done,
                p.scope.lints.selection().clone(),
                p.policy,
                p.fix_error.clone(),
            ))
        });
        let (findings, deps, requires, types) = match outcome {
            Ok((done, levels, policy, fix_error)) => {
                let mut findings = convert(&path, &text, &done.found, &levels, &policy);
                // A `[fix]` that `htl fix` refuses is a setup error, said like one.
                if let Some(e) = fix_error {
                    findings.push(Finding {
                        lsp: failure(&anyhow::anyhow!("{e}")),
                        fix: None,
                        rule: String::new(),
                    });
                }
                (findings, done.deps, done.requires, done.types)
            }
            // Nothing could be said about the file: say that, on the file, so the editor
            // shows why it has no diagnostics instead of looking clean.
            Err(e) => (
                vec![Finding {
                    lsp: failure(&e),
                    fix: None,
                    rule: String::new(),
                }],
                Vec::new(),
                Vec::new(),
                htl::typeindex::TypeIndex::default(),
            ),
        };
        // A write that landed while the check ran leaves the checker's positions in a text
        // that may not be `text`; the buffer can still be `text` (an external write), so the
        // comparison in `code_actions` would not catch it. Its fixes, the index of its
        // names and its `require` sites are dropped; the save that follows the write checks
        // again.
        let (mut findings, mut requires, mut types) = (findings, requires, types);
        if read_lossy(&path) != text {
            for f in &mut findings {
                f.fix = None;
            }
            requires.clear();
            types = htl::typeindex::TypeIndex::default();
        }
        let diagnostics = findings.iter().map(|f| f.lsp.clone()).collect();
        // The symbol index, if the project has one, takes this check's declarations for the
        // file: fresher than a read of the disk, and kinds the types decide (module doc,
        // "Workspace symbols").
        if let Some(index) = self.indices.get_mut(&key)
            && let Some(stamp) = stamp
            && let Some(decls) = types.declarations()
        {
            let syms = symbols::flatten(&text, decls);
            index.put(
                &canonical(&path),
                Entry::new(path.clone(), stamp, &text, syms),
            );
        }
        if let Some(doc) = self.open.get_mut(uri) {
            doc.deps = deps.iter().map(|d| canonical(d)).collect();
            doc.checked = Some(Checked {
                text,
                findings,
                requires,
                types,
                fix_all: None,
            });
        }
        self.publish(uri.clone(), diagnostics);
    }

    /// Answer `textDocument/formatting`, `codeAction`, `hover`, `definition`,
    /// `documentSymbol` or the workspace-symbol request.
    fn document_request(&mut self, req: lsp_server::Request) -> Response {
        let id = req.id.clone();
        let bad = |e: serde_json::Error| {
            Response::new_err(
                id.clone(),
                ErrorCode::InvalidParams as i32,
                format!("malformed {}: {e}", req.method),
            )
        };
        let params = req.params.clone();
        match req.method.as_str() {
            Formatting::METHOD => {
                match serde_json::from_value::<DocumentFormattingParams>(params) {
                    Ok(p) => Response::new_ok(id.clone(), self.formatting(&p.text_document.uri)),
                    Err(e) => bad(e),
                }
            }
            HoverRequest::METHOD => match serde_json::from_value::<HoverParams>(params) {
                Ok(p) => {
                    let at = p.text_document_position_params;
                    Response::new_ok(id.clone(), self.hover(&at.text_document.uri, at.position))
                }
                Err(e) => bad(e),
            },
            GotoDefinition::METHOD => {
                match serde_json::from_value::<GotoDefinitionParams>(params) {
                    Ok(p) => {
                        let at = p.text_document_position_params;
                        Response::new_ok(
                            id.clone(),
                            self.definition(&at.text_document.uri, at.position),
                        )
                    }
                    Err(e) => bad(e),
                }
            }
            DocumentSymbolRequest::METHOD => {
                match serde_json::from_value::<DocumentSymbolParams>(params) {
                    Ok(p) => {
                        Response::new_ok(id.clone(), self.document_symbols(&p.text_document.uri))
                    }
                    Err(e) => bad(e),
                }
            }
            WorkspaceSymbolRequest::METHOD => {
                match serde_json::from_value::<WorkspaceSymbolParams>(params) {
                    Ok(p) => Response::new_ok(id.clone(), self.workspace_symbols(&p.query)),
                    Err(e) => bad(e),
                }
            }
            _ => match serde_json::from_value::<CodeActionParams>(params) {
                Ok(p) => Response::new_ok(id.clone(), self.code_actions(&p)),
                Err(e) => bad(e),
            },
        }
    }

    /// What the last check of `uri` read, while the client's buffer is still that text:
    /// positions in the check's answers are positions in it. `None` for a document that was
    /// never checked, one whose buffer has moved on, and one whose buffer the server lost
    /// track of.
    fn checked_buffer(&self, uri: &Uri) -> Option<&Checked> {
        let doc = self.open.get(uri).filter(|d| !d.unsynced)?;
        let checked = doc.checked.as_ref()?;
        let buffer = match &doc.buffer {
            Some(buffer) => buffer.as_str(),
            None => &read_lossy(&doc.path),
        };
        (buffer == checked.text).then_some(checked)
    }

    /// The type of the name at `at`, as Teal writes it; `null` when there is no name there
    /// or the checker has no type for it.
    fn hover(&self, uri: &Uri, at: Position) -> Option<Hover> {
        let checked = self.checked_buffer(uri)?;
        let lines = lines_of(&checked.text);
        let (line, col) = byte_position(&lines, at)?;
        let found = checked.types.at(&checked.text, line, col)?;
        let ty = found.ty?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```teal\n{ty}\n```"),
            }),
            range: Some(range_at(&lines, found.line, found.col)),
        })
    }

    /// Where the thing at `at` is declared: for the string of a `require`, the file htl's
    /// resolution picked; for a name, the declaration the checker's report points to. `null`
    /// when neither is known, or the declaration is in a file that is not on disk.
    fn definition(&self, uri: &Uri, at: Position) -> Option<GotoDefinitionResponse> {
        let checked = self.checked_buffer(uri)?;
        let lines = lines_of(&checked.text);
        let (line, col) = byte_position(&lines, at)?;
        let here = path_of(uri)?;
        if let Some(site) = required_at(&lines, &checked.requires, line, col) {
            // The module's first character: a `require` is of the file, not of a line in it.
            let file = site.path.as_ref()?;
            let target = location_of(&here, uri, file, &checked.text, None)?;
            return Some(GotoDefinitionResponse::Scalar(target));
        }
        let place = checked.types.at(&checked.text, line, col)?.declared?;
        let target = location_of(
            &here,
            uri,
            &place.file,
            &checked.text,
            Some((place.line, place.col)),
        )?;
        Some(GotoDefinitionResponse::Scalar(target))
    }

    /// The declarations of the file as a tree, from the index of the last check of it. `null`
    /// when that is not known: the file is not open, it was never checked or is not on disk,
    /// the buffer is not the text the check read, or it did not parse. `[]` only for a file
    /// that parses and declares nothing.
    fn document_symbols(&self, uri: &Uri) -> Option<DocumentSymbolResponse> {
        let checked = self.checked_buffer(uri)?;
        let decls = checked.types.declarations()?;
        let lines = lines_of(&checked.text);
        Some(DocumentSymbolResponse::Nested(
            decls.iter().map(|d| symbol_of(&lines, d)).collect(),
        ))
    }

    /// The declarations of the project's files whose name has `query` in it, as the answer to
    /// a workspace-symbol request. The projects asked are those of the open files, else those
    /// of the workspace folders. `null` when no project's index could be made (none to ask, or
    /// the one there is cannot be loaded or walked: the reason is on stderr); `[]` when an
    /// index was made and nothing in it matches.
    fn workspace_symbols(&mut self, query: &str) -> Option<WorkspaceSymbolResponse> {
        let mut docs: Vec<(&Uri, &Doc)> = self.open.iter().collect();
        docs.sort_by(|a, b| a.0.cmp(b.0));
        let mut asked: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (_, d) in docs {
            let key = d
                .project
                .clone()
                .or_else(|| project_key(&d.path).ok())
                .unwrap_or_else(|| htl::parent_dir(&d.path));
            if !asked.iter().any(|(k, _)| *k == key) {
                asked.push((key, d.path.clone()));
            }
        }
        if asked.is_empty() {
            for dir in &self.workspace {
                match htl::model::Project::find_root(dir) {
                    Ok(Some(found)) => {
                        if !asked.iter().any(|(k, _)| *k == found.root) {
                            asked.push((found.root, dir.clone()));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("htl lsp: no symbols for {}: {e:#}", dir.display()),
                }
            }
            if asked.is_empty() {
                eprintln!(
                    "htl lsp: no symbols to search: no file is open and no workspace folder \
                     is in a project"
                );
                return None;
            }
        }
        // A project whose manifests moved is built again, and its open files are checked
        // against it, before it is asked (as before a formatting request).
        for other in self.drop_moved(asked.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()) {
            self.check_one(&other);
        }
        let mut built: Vec<&PathBuf> = Vec::new();
        for (key, file) in &asked {
            match self.refresh_symbols(key, file) {
                Ok(()) => built.push(key),
                Err(e) => eprintln!("htl lsp: no symbols for {}: {e:#}", key.display()),
            }
        }
        if built.is_empty() {
            return None;
        }
        // A watcher names files, not the directories they were in: a directory deleted or
        // renamed is one event for the directory, which `**/*.tl` does not match, and its
        // files would stay in the index. A hit on a file that is gone says so; the indices
        // that hold it walk their project again, and the search is made once more.
        let mut out = self.symbol_answer(&built, query);
        if !out.1.is_empty() {
            let mut again: Vec<&PathBuf> = Vec::new();
            for key in &built {
                if let Some(index) = self.indices.get_mut(*key)
                    && index.entries().any(|e| out.1.contains(&e.path))
                {
                    index.relist();
                    again.push(*key);
                }
            }
            for key in again {
                let file = &asked.iter().find(|(k, _)| k == key).expect("asked").1;
                if let Err(e) = self.refresh_symbols(key, file) {
                    eprintln!("htl lsp: no symbols for {}: {e:#}", key.display());
                }
            }
            out = self.symbol_answer(&built, query);
        }
        Some(WorkspaceSymbolResponse::Flat(out.0))
    }

    /// The search of a workspace-symbol request over the indices of `built`, and the files
    /// among its hits that are no longer there (left out of the answer).
    fn symbol_answer(
        &self,
        built: &[&PathBuf],
        query: &str,
    ) -> (Vec<SymbolInformation>, Vec<PathBuf>) {
        let (hits, total) = symbols::search(
            built
                .iter()
                .filter_map(|k| self.indices.get(*k))
                .flat_map(SymbolIndex::entries),
            query,
        );
        if total > hits.len() {
            eprintln!(
                "htl lsp: {total} symbols match {query:?}; the first {} are answered",
                hits.len()
            );
        }
        let mut gone: Vec<PathBuf> = Vec::new();
        let mut uri: Option<(&Path, Option<Uri>)> = None;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            if uri.as_ref().is_none_or(|(p, _)| *p != h.path) {
                let target = if h.path.is_file() {
                    std::path::absolute(h.path).ok().and_then(|p| uri_of(&p))
                } else {
                    if !gone.iter().any(|g| g == h.path) {
                        gone.push(h.path.to_path_buf());
                    }
                    None
                };
                uri = Some((h.path, target));
            }
            let Some((_, Some(target))) = &uri else {
                continue;
            };
            #[allow(deprecated)]
            out.push(SymbolInformation {
                name: h.sym.name.clone(),
                kind: h.sym.kind,
                tags: None,
                deprecated: None,
                location: Location {
                    uri: target.clone(),
                    range: h.sym.range,
                },
                container_name: h.sym.container.clone(),
            });
        }
        (out, gone)
    }

    /// Bring the symbol index of the project under `key` up to date, making it (and loading
    /// the project, from `file`) if the session has not met it.
    fn refresh_symbols(&mut self, key: &Path, file: &Path) -> Result<()> {
        self.project(key, file)?;
        let mut index = self.indices.remove(key).unwrap_or_default();
        let project = &self.projects[key];
        let open: Vec<&Doc> = self.open.values().collect();
        // The files of a directory that is no project: the open ones.
        let own: Vec<PathBuf> = open
            .iter()
            .filter(|d| d.project.as_deref() == Some(key) || htl::parent_dir(&d.path) == key)
            .map(|d| d.path.clone())
            .collect();
        // A directory that is no project is listed by its open files, which no watcher
        // reports: it is listed again each time (that costs a `stat` per open file).
        let sweep = !self.watching || project.model.is_none();
        let result = index.refresh(
            sweep,
            || project.source_files(key, &own),
            &mut |canonical, path, text| project.symbols_of(&open, canonical, path, text),
        );
        self.indices.insert(key.to_path_buf(), index);
        result
    }

    /// The edits that make the buffer what `htl fmt` writes for it: `[]` when it already is,
    /// one edit over the changed span when not, and `null` when `htl fmt` could not format
    /// it.
    fn formatting(&mut self, uri: &Uri) -> Option<Vec<TextEdit>> {
        let path = path_of(uri)?;
        let key = project_key(&path).unwrap_or_else(|_| htl::parent_dir(&path));
        // The buffer if the client has one; a file it has not opened is its disk. A buffer
        // the server lost track of is neither, and gets no edits.
        if self.open.get(uri).is_some_and(|d| d.unsynced) {
            eprintln!(
                "htl lsp: not formatting {}: its buffer is not known",
                path.display()
            );
            return None;
        }
        let buffer = self.open.get(uri).and_then(|d| d.buffer.clone());
        // A project whose manifests moved is built again for this answer; its open files
        // are checked against the new one first, as a check of any of them would have them,
        // or they would keep what the old one said with nothing left to notice the change.
        for other in self.drop_moved([key.clone()]) {
            self.check_one(&other);
        }
        let result = (|| {
            let before = match buffer {
                Some(text) => text,
                None => std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
            };
            let after = self.project(&key, &path)?.format(&path, &before)?;
            Ok::<_, anyhow::Error>((before, after))
        })();
        match result {
            Ok((before, after)) => Some(minimal_edit(&before, &after).into_iter().collect()),
            // Not a formatting problem the client can do anything about (a file the parser
            // rejects is already a diagnostic): no edits, and the reason where the
            // server's other complaints go.
            Err(e) => {
                eprintln!("htl lsp: not formatting {}: {e:#}", path.display());
                None
            }
        }
    }

    /// The quick fixes for the findings at the request's range, and "fix all".
    fn code_actions(&mut self, p: &CodeActionParams) -> Vec<CodeActionOrCommand> {
        let uri = &p.text_document.uri;
        let wanted = |kind: &CodeActionKind| match &p.context.only {
            None => true,
            Some(only) => only.iter().any(|o| kind_within(kind, o)),
        };
        let quick = wanted(&CodeActionKind::QUICKFIX);
        // Fix-all costs a dry run of `htl fix`, so it is computed for a client that asked
        // for it: by kind (a code action on save), or by invoking the menu. A lightbulb
        // that asks on every cursor move asks for neither.
        let all = match &p.context.only {
            Some(only) => only.iter().any(|o| kind_within(&fix_all_kind(), o)),
            None => p.context.trigger_kind == Some(CodeActionTriggerKind::INVOKED),
        };
        let Some(doc) = self.open.get(uri).filter(|d| !d.unsynced) else {
            return Vec::new();
        };
        let (Some(checked), Some(key)) = (&doc.checked, doc.project.clone()) else {
            return Vec::new();
        };
        // A fix is positions in the text that was checked: the buffer must still be it
        // (module doc, "Formatting and code actions"). A buffer the server never saw
        // change is the disk, which is what the check read.
        let buffer = doc.buffer.clone().unwrap_or_else(|| read_lossy(&doc.path));
        if buffer != checked.text {
            return Vec::new();
        }
        let Some(project) = self.projects.get(&key) else {
            return Vec::new();
        };
        if project.fix_error.is_some() {
            return Vec::new();
        }
        let mut actions = Vec::new();
        if quick {
            for f in &checked.findings {
                let Some(fix) = &f.fix else { continue };
                if !overlaps(&f.lsp.range, &p.range) {
                    continue;
                }
                let class = project.fix.effective(&f.rule, fix);
                if class == htl::Applicability::Suggest {
                    continue;
                }
                // The text this fix alone leaves, which is `htl fix`'s to say.
                let Some(after) = htl::fix::apply_fix(&buffer, fix) else {
                    continue;
                };
                let Some(edit) = minimal_edit(&buffer, &after) else {
                    continue;
                };
                let applied = project.fix.disposition(&f.rule, fix)
                    == htl::fix::Disposition::Apply(htl::Applicability::Safe);
                let unsafe_ = class == htl::Applicability::Unsafe;
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: format!("Fix {}{}", f.rule, if unsafe_ { " (unsafe)" } else { "" }),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![f.lsp.clone()]),
                    edit: Some(edit_of(uri, vec![edit])),
                    is_preferred: Some(applied),
                    ..Default::default()
                }));
            }
        }
        // `htl fix` has something to apply only if a fix it applies is on file; without
        // one the dry run is not worth its cost.
        let applicable = checked.findings.iter().any(|f| {
            f.fix.as_ref().is_some_and(|fix| {
                matches!(
                    project.fix.disposition(&f.rule, fix),
                    htl::fix::Disposition::Apply(_)
                )
            })
        });
        // `fix_file` reads the disk, so the disk must be the text too.
        if all && applicable && read_lossy(&doc.path) == checked.text {
            let path = doc.path.clone();
            // A failure is said and not kept: the next request asks again.
            let fixed = match checked.fix_all.clone() {
                Some(known) => Some(known),
                None => match project.fixed(&path) {
                    Ok(known) => {
                        if let Some(doc) = self.open.get_mut(uri)
                            && let Some(checked) = &mut doc.checked
                        {
                            checked.fix_all = Some(known.clone());
                        }
                        Some(known)
                    }
                    Err(e) => {
                        eprintln!("htl lsp: no fix-all for {}: {e:#}", path.display());
                        None
                    }
                },
            }
            .flatten();
            if let Some(edit) = fixed.and_then(|text| minimal_edit(&buffer, &text)) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Fix all htl problems that are safe to fix".to_string(),
                    kind: Some(fix_all_kind()),
                    edit: Some(edit_of(uri, vec![edit])),
                    ..Default::default()
                }));
            }
        }
        actions
    }

    /// The project under `key`, loaded for `file` if the session has not met it yet. A
    /// project that failed to load is not remembered, so the next save tries again.
    ///
    /// Whether its manifests have changed is asked by the callers, before they come here
    /// ([`Self::drop_moved`]): a project rebuilt here would leave its other open files with
    /// what the old one said.
    fn project(&mut self, key: &Path, file: &Path) -> Result<&Project> {
        if !self.projects.contains_key(key) {
            let loaded = Project::load(file, key)?;
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

/// What a changed file is to a project (module doc, "Files that change the answer").
#[derive(Clone, Copy, PartialEq, Eq)]
enum Watched {
    /// `htl.toml`, `mlua-pkg.toml` or `mlua-pkg.lock`.
    Manifest,
    /// A `.d.tl` under a directory named `types`.
    Types,
    /// A Rust source or `Cargo.toml` (whether `htl dts` reads it is [`Self::reads_rust`]).
    Rust,
}

impl Watched {
    fn of(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if Manifests::names().contains(&name) {
            Some(Self::Manifest)
        } else if name.ends_with(".d.tl") && path.components().any(|c| c.as_os_str() == "types") {
            Some(Self::Types)
        } else if name == "Cargo.toml" || name.ends_with(".rs") {
            Some(Self::Rust)
        } else {
            None
        }
    }

    /// Whether `htl dts` reads `path` for the crate at `root`: its `Cargo.toml`, or a `.rs`
    /// under `src`, `examples`, `tests` or `benches` (the walk of
    /// [`htl::dts::generate_crate_to`]). The `.rs` files cargo writes under `target/` are
    /// not, which is most of the Rust events a build makes.
    fn reads_rust(path: &Path, root: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(root) else {
            return false;
        };
        let mut parts = rel.components().map(|c| c.as_os_str());
        matches!(
            (parts.next().and_then(|p| p.to_str()), parts.next()),
            (Some("Cargo.toml"), None) | (Some("src" | "examples" | "tests" | "benches"), Some(_))
        )
    }
}

/// The 1-based line and byte column of an LSP position in `lines`; `None` for a line the
/// text does not have. A character past the end of its line is the end.
fn byte_position(lines: &[&str], at: Position) -> Option<(usize, usize)> {
    let text = lines.get(at.line as usize)?;
    let mut units = 0u32;
    let mut byte = text.len();
    for (i, c) in text.char_indices() {
        if units >= at.character {
            byte = i;
            break;
        }
        units += c.len_utf16() as u32;
    }
    Some((at.line as usize + 1, byte + 1))
}

/// The `require` site, among `sites`, whose module string the cursor (`line`, byte `col`)
/// is in, the quotes included and the end of the closing one too, as a cursor after the
/// last character is on it.
///
/// A site is where the checker saw the call; its string is the first quote after that
/// point on the line. A call spelled across lines is not found.
fn required_at<'a>(
    lines: &[&str],
    sites: &'a [htl::RequireSite],
    line: usize,
    col: usize,
) -> Option<&'a htl::RequireSite> {
    let text = lines.get(line - 1)?;
    sites.iter().filter(|s| s.line == line).find(|s| {
        let from = (s.col.saturating_sub(1)).min(text.len());
        let Some(open) = text.get(from..).and_then(|t| t.find(['"', '\''])) else {
            return false;
        };
        let open = from + open;
        let quote = text.as_bytes()[open];
        let close = text.as_bytes()[open + 1..]
            .iter()
            .position(|c| *c == quote)
            .map_or(text.len(), |p| open + 1 + p + 1);
        (open + 1..=close + 1).contains(&col)
    })
}

/// A `Location` for `file`, as a report spells it (relative to the working directory, or
/// absolute, `./` and all), at `place` (1-based line and byte column) or at the start. The
/// file the request was made in answers with the URI the client used for it. `None` for a
/// file that is not on disk: the standard library's declarations are named by a file Teal
/// carries inside itself.
fn location_of(
    here: &Path,
    uri: &Uri,
    file: &Path,
    text: &str,
    place: Option<(usize, usize)>,
) -> Option<Location> {
    let mut path = PathBuf::new();
    for part in std::env::current_dir()
        .unwrap_or_default()
        .join(file)
        .components()
    {
        if part != std::path::Component::CurDir {
            path.push(part);
        }
    }
    let target = if path == here {
        uri.clone()
    } else if path.is_file() {
        uri_of(&path)?
    } else {
        return None;
    };
    let start = match place {
        None => Position::new(0, 0),
        Some((line, col)) => {
            let theirs;
            let theirs_text = if path == here {
                text
            } else {
                theirs = read_lossy(&path);
                &theirs
            };
            range_at(&lines_of(theirs_text), line, col).start
        }
    };
    Some(Location {
        uri: target,
        range: Range::new(start, start),
    })
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

/// Whether `path` names a Teal source (`m.tl`, not the declaration file `m.d.tl`).
fn is_tl(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".tl") && !n.ends_with(".d.tl"))
}

/// The folders a client opened the workspace on: `workspaceFolders`, else `rootUri`, else
/// `rootPath`, in the `initialize` parameters.
fn workspace_of(params: &serde_json::Value) -> Vec<PathBuf> {
    let uri_path = |v: &serde_json::Value| {
        v.as_str()
            .and_then(|u| Uri::from_str(u).ok())
            .and_then(|u| path_of(&u))
    };
    let folders: Vec<PathBuf> = params
        .get("workspaceFolders")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|f| uri_path(&f["uri"])).collect())
        .unwrap_or_default();
    if !folders.is_empty() {
        return folders;
    }
    if let Some(root) = uri_path(&params["rootUri"]) {
        return vec![root];
    }
    params["rootPath"]
        .as_str()
        .map(|p| vec![PathBuf::from(p)])
        .unwrap_or_default()
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

/// `p` in its canonical directory, with its own name kept: a file a watcher reports may be
/// gone (a delete), and a manifest that is a symlink is still the manifest of the directory
/// it is in, not of the one it points into.
fn canonical_dir_of(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) => canonical(dir).join(name),
        _ => p.to_path_buf(),
    }
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
    text: &str,
    found: &[htl::Diagnostic],
    levels: &htl::lint::Selection,
    policy: &htl::verdict::Policy,
) -> Vec<Finding> {
    // What the positions are read against: the file as the check saw it.
    let lines = lines_of(text);
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
            let lsp = lsp_types::Diagnostic {
                range,
                severity: Some(severity(d, levels, policy)),
                code: d.rule.clone().map(NumberOrString::String),
                source: Some("htl".to_string()),
                message,
                related_information: related,
                ..Default::default()
            };
            // The fixes `htl fix` looks at are the file's own errors' and lints' (a Teal
            // warning is none of them), named by the rule or, for an error the checker
            // gave none, by `tl:error` (`htl_core::fix`'s `rule_of`).
            let fix = d
                .fix
                .clone()
                .filter(|_| own && d.severity != htl::Severity::Warning);
            Finding {
                lsp,
                fix,
                rule: htl::fix::rule_of(d),
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

/// The kind of the action that applies every fix `htl fix` would.
fn fix_all_kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.htl")
}

/// Whether `kind` is `only` or one of its sub-kinds (`source.fixAll.htl` is within
/// `source.fixAll` and `source`, not within `source.fix`): the filter's rule.
fn kind_within(kind: &CodeActionKind, only: &CodeActionKind) -> bool {
    let (kind, only) = (kind.as_str(), only.as_str());
    kind == only || kind.strip_prefix(only).is_some_and(|r| r.starts_with('.'))
}

/// Whether two ranges share a point, the ends included: a cursor at either end of a
/// finding is on it, and an empty range (a cursor) is a point.
fn overlaps(a: &Range, b: &Range) -> bool {
    a.start <= b.end && b.start <= a.end
}

/// A `WorkspaceEdit` of `edits` in the one file `uri`.
fn edit_of(uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), edits)])),
        ..Default::default()
    }
}

/// One edit that turns `before` into `after`, over the span in which they differ: the
/// text they share at the start and at the end is left out, so an editor keeps its cursors,
/// folds and marks everywhere else. `None` when they are the same text.
///
/// Positions are 0-based lines and UTF-16 columns of `before`, its lines ended by `\n`,
/// `\r\n` or a lone `\r` as LSP counts them. The span is kept off the middle of a `\r\n`,
/// which a position is not allowed to be in.
fn minimal_edit(before: &str, after: &str) -> Option<TextEdit> {
    if before == after {
        return None;
    }
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    if start > 0
        && before.as_bytes()[start - 1] == b'\r'
        && before.as_bytes().get(start) == Some(&b'\n')
    {
        start -= 1;
    }
    let (rb, ra) = (&before.as_bytes()[start..], &after.as_bytes()[start..]);
    let mut shared = rb
        .iter()
        .rev()
        .zip(ra.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    // The tail starts on a character of both, and at a `\n` only if its `\r` is in it too.
    while shared > 0
        && (!before.is_char_boundary(before.len() - shared)
            || !after.is_char_boundary(after.len() - shared)
            || (before.as_bytes()[before.len() - shared] == b'\n'
                && before.len() - shared > start
                && before.as_bytes()[before.len() - shared - 1] == b'\r'))
    {
        shared -= 1;
    }
    let (end_before, end_after) = (before.len() - shared, after.len() - shared);
    // A line ends at `\n`, `\r\n` or a lone `\r`: LSP counts all three, so a client
    // reads a position past a lone `\r` as on the next line.
    let at = |upto: usize| {
        let b = before.as_bytes();
        let (mut line, mut line_start) = (0u32, 0usize);
        for i in 0..upto {
            if b[i] == b'\n' || (b[i] == b'\r' && b.get(i + 1) != Some(&b'\n')) {
                line += 1;
                line_start = i + 1;
            }
        }
        let col = before[line_start..upto].encode_utf16().count();
        Position::new(line, col as u32)
    };
    Some(TextEdit::new(
        Range::new(at(start), at(end_before)),
        after[start..end_after].to_string(),
    ))
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

/// The LSP position of a point the checker reports: `line` counted from 1 and `col` from 1
/// in bytes, as a 0-based line and a UTF-16 column. A column past the end of the line is its
/// end, and a line past the end of the text is its last line's.
fn position_at(lines: &[&str], line: usize, col: usize) -> Position {
    let (n, text, at) = clamped(lines, line, col);
    Position::new(n as u32, text[..at].encode_utf16().count() as u32)
}

/// The point at `line` (from 1) and byte `col` (from 1) as the 0-based line, its text and a
/// byte offset in it on a character boundary: a column past the end is the end, one inside a
/// character the start of that character, and a line past the end of the text the last line.
fn clamped<'a>(lines: &[&'a str], line: usize, col: usize) -> (usize, &'a str, usize) {
    let n = line.saturating_sub(1).min(lines.len().saturating_sub(1));
    let text = lines.get(n).copied().unwrap_or("");
    let mut at = col.saturating_sub(1).min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    (n, text, at)
}

/// One declaration of the index as an LSP `DocumentSymbol`, its children with it.
///
/// The kinds: a record is a `Struct`, an interface an `Interface`, an enum an `Enum`; a
/// type alias a `TypeParameter` (rust-analyzer's choice for one); a variable a `Variable`; a
/// record's field that is not a function a `Field`; any function, `function R.f` and a
/// function-typed field included, a `Function`, and `function R:m` (or a field typed as a
/// method) a `Method`. `detail` is Teal's type string. `range` is the declaration's extent as
/// far as the index knows it ([`htl::typeindex`], "Declarations"; an unknown end is the end of
/// the line) and `selectionRange` is the name, which `range` is made to contain.
fn symbol_of(lines: &[&str], d: &htl::typeindex::Decl) -> DocumentSymbol {
    let start = position_at(lines, d.line, d.col);
    let end = position_at(lines, d.end_line, d.end_col.unwrap_or(usize::MAX));
    let name_start = position_at(lines, d.name_line, d.name_col);
    let name_end = position_at(
        lines,
        d.name_line,
        d.name_col.saturating_add(d.short_name().len()),
    );
    let children: Vec<DocumentSymbol> = d.children.iter().map(|c| symbol_of(lines, c)).collect();
    #[allow(deprecated)]
    DocumentSymbol {
        name: d.name.clone(),
        detail: d.detail.clone(),
        kind: symbols::kind_of(d.kind),
        tags: None,
        deprecated: None,
        range: Range::new(start.min(name_start), end.max(name_end)),
        selection_range: Range::new(name_start, name_end),
        children: (!children.is_empty()).then_some(children),
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
    let (n, text, start) = clamped(lines, line, col);
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

    /// `edit` applied to `before`, by the positions it carries.
    fn applied(before: &str, edit: &TextEdit) -> String {
        let at = |p: Position| {
            let line_start: usize = before
                .split('\n')
                .take(p.line as usize)
                .map(|l| l.len() + 1)
                .sum();
            let mut units = 0;
            let mut off = line_start;
            for c in before[line_start..].chars() {
                if units >= p.character || c == '\n' {
                    break;
                }
                units += c.len_utf16() as u32;
                off += c.len_utf8();
            }
            off
        };
        let mut out = before.to_string();
        out.replace_range(at(edit.range.start)..at(edit.range.end), &edit.new_text);
        out
    }

    #[test]
    fn an_edit_covers_only_the_span_that_differs() {
        assert!(minimal_edit("a\nb\n", "a\nb\n").is_none());
        let e = minimal_edit("a\nbcd\ne\n", "a\nbXd\ne\n").unwrap();
        assert_eq!(e.new_text, "X");
        assert_eq!(
            (e.range.start, e.range.end),
            (Position::new(1, 1), Position::new(1, 2))
        );
        // An insertion is an empty range; a deletion an empty text.
        let e = minimal_edit("ab", "aXb").unwrap();
        assert_eq!(
            (e.range.start, e.range.end),
            (Position::new(0, 1), Position::new(0, 1))
        );
        let e = minimal_edit("aXb", "ab").unwrap();
        assert_eq!(
            (e.new_text.as_str(), e.range.end),
            ("", Position::new(0, 2))
        );
        // Repeated text: the shared prefix and suffix may not overlap.
        let e = minimal_edit("aaa", "aa").unwrap();
        assert_eq!(applied("aaa", &e), "aa");
    }

    #[test]
    fn an_edit_is_in_utf16_and_not_in_the_middle_of_a_character_or_a_crlf() {
        // `é` and the emoji share their first bytes with other characters.
        for (a, b) in [
            ("x \u{e9}y", "x \u{e8}y"),
            ("\u{1F600}a", "\u{1F601}a"),
            ("a\u{1F600}", "a\u{1F601}"),
            ("l1\r\nl2\r\n", "l1\r\nl2\r\nl3\r\n"),
            ("l1\r\nl2\r\n", "l1\nl2\n"),
            ("a\r\nb", "a\r\n\r\nb"),
            ("a\r\n", "a\r\n\r\n"),
            ("", "x"),
            ("x", ""),
        ] {
            let e = minimal_edit(a, b).unwrap();
            assert_eq!(applied(a, &e), b, "{a:?} -> {b:?}: {e:?}");
        }
        let e = minimal_edit("\u{1F600}a", "\u{1F601}a").unwrap();
        assert_eq!(
            (e.range.start, e.range.end),
            (Position::new(0, 0), Position::new(0, 2))
        );
        // No position between the `\r` and the `\n` of a line end.
        let e = minimal_edit("l1\r\nl2\r\n", "l1\r\nl2\r\nl3\r\n").unwrap();
        assert_eq!(e.range.start, Position::new(2, 0));
        // A lone `\r` ends a line for the client: past it is the next line's start.
        let e = minimal_edit("a\rb", "a\r\nb").unwrap();
        assert_eq!(
            (e.range.start, e.new_text.as_str()),
            (Position::new(1, 0), "\n")
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

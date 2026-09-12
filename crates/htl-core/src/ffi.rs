//! The C ABI an htl host hands to a caller that is not written in Rust.
//!
//! A host built with [`host_module`](crate::dts) has three ways out today — the Rust
//! binary, generated Lua, and `.d.tl` declarations — and all three assume the caller
//! links Rust. A Unity script, a Swift app or a Python REPL needs a C ABI, and writing
//! one by hand per project is where the drift the rest of htl removes comes back: a
//! rename on the Rust side is not seen by the header, and each host language has a
//! "most natural" way of reading a `char *` that corrupts the heap or leaks every call.
//!
//! This module is the half of that layer which is the same for every project: the
//! handle, the error slot, the string hand-over, the panic guard and the version
//! functions. The half that is not — one wrapper per method, and the header — is
//! written by `#[c_export]`, which calls into here.
//!
//! # The shape of the ABI
//!
//! Everything a caller holds is one of three things: an opaque handle pointer, a
//! `char *` this library allocated, or an `int`. There are no structs by value, no
//! `bool`, no bare enums and no variadics, because those are what a host on the other
//! side of the boundary gets wrong — C# marshals a returned `string` and then frees it
//! with `CoTaskMemFree`, Python's `restype = c_char_p` copies and leaks the original,
//! and neither language agrees with Rust on the width of a `bool`.
//!
//! - **`char *` is ours, freeing it is theirs.** Every string this library returns was
//!   allocated by [`give`] and must come back to `<prefix>_free`. Nothing else may.
//! - **`int` is a status** ([`Status`]), never a value. A call that has both a value and
//!   a status writes the value through an out-parameter, so no function packs three
//!   meanings into one `int` the way a hand-written layer tends to.
//! - **A `const char *` argument is borrowed** for the duration of the call: it is
//!   copied before anything can hold on to it, and the caller may free it on return.
//!
//! # Threads
//!
//! One handle belongs to one thread: the one that opened it. [`Handle`] records that
//! thread and every entry checks it, so using a handle from a second thread is
//! [`Status::WrongThread`] rather than a data race — mlua's `send` feature is not
//! enabled and a `Lua` is not `Sync`. `<prefix>_threadsafe()` reports this the way
//! SQLite's `sqlite3_threadsafe()` does, and answers `0`.
//!
//! The one exception is [`Handle::interrupt`], which is callable from any thread: it
//! touches only an atomic in a separate allocation that the owning thread never
//! mutates. An [`Interrupt`] installed on the state ([`Interrupt::install`]) turns that
//! atomic into a Lua error at the next hook tick, which is how a runaway mod is stopped
//! from a host that cannot preempt it.
//!
//! # Panics
//!
//! Since Rust 1.81 a panic that reaches an `extern "C"` frame aborts the process, which
//! for a Unity plugin means taking the editor down. [`guard`] and [`Handle::enter`] wrap
//! the call in `catch_unwind`, record the payload as the last error, and hand back a
//! sentinel. A panic also **poisons** the handle: every later entry answers
//! [`Status::Panic`] without running anything, because a panic out of the middle of an
//! mlua call leaves a state nobody has reasoned about.
//!
//! # Errors
//!
//! The last error is a per-thread slot, in the SQLite style: a failed call returns its
//! sentinel (`NULL`, or a status), and the message is read afterwards with
//! `<prefix>_last_error()` — a pointer owned by this library, valid until the next call
//! on the same thread — or copied into the caller's own buffer with
//! `<prefix>_last_error_into(buf, len)`, which is what a host that cannot hold a foreign
//! pointer safely should use. `<prefix>_last_status()` is the status that went with it,
//! which is how the `char *` shape (whose failure is a bare `NULL`) says *why*.
//!
//! # Payloads
//!
//! A method returning a `String` hands back that text as it is. A method returning
//! anything else hands back JSON ([`json`]), so the caller needs a JSON parser and
//! nothing else, and an object payload carries `"v"` — [`PAYLOAD_VERSION`] — so the
//! schema of the payload can move without moving [`ABI_VERSION`], which is the shape of
//! the functions themselves.

use crate::Htl;
use std::any::Any;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

/// The shape of the generated functions: their names, their arguments and what each one
/// means. A caller that holds a library built against a different value holds a library
/// it cannot call, and `<prefix>_abi_version()` is how it finds out — a plugin host such
/// as Unity never unloads a native library until the editor restarts, so "the file on
/// disk changed" is not something the caller can see any other way.
///
/// It moves when a function is removed, when an argument list changes, or when a status
/// code changes meaning. It does not move when a method is added, and it does not move
/// when the *contents* of a JSON payload change — that is [`PAYLOAD_VERSION`].
pub const ABI_VERSION: c_int = 1;

/// The schema of the JSON payloads, carried as `"v"` on every object payload (see
/// [`json`]). Separate from [`ABI_VERSION`] on purpose: a field added to a record is not
/// a change to the functions.
pub const PAYLOAD_VERSION: i64 = 1;

/// What an `int`-returning function answers. Every one of these crosses as a plain
/// `c_int`, and the generated header defines them as `<PREFIX>_OK` and friends.
///
/// The set is closed: a wrapper maps whatever the Rust side failed with onto one of
/// these, and the detail goes in the last-error message rather than into new codes,
/// because a caller written against `<prefix>.h` cannot switch on a code it has never
/// heard of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    /// The call did what it says. Any out-parameter has been written.
    Ok = 0,
    /// The call failed and said why; read the message with `<prefix>_last_error()`.
    Err = 1,
    /// The handle was `NULL`, was never returned by `<prefix>_open`, or has been closed.
    BadHandle = 2,
    /// What was asked for is not there. Distinct from [`Status::Err`] so that "no such
    /// thing" does not have to be read out of a message.
    NotFound = 3,
    /// A Lua error reached the boundary: the message is the one Lua raised.
    Lua = 4,
    /// A Rust panic was caught. The handle is poisoned and answers this from now on.
    Panic = 5,
    /// The handle belongs to another thread. Nothing was run.
    WrongThread = 6,
    /// The script was stopped by `<prefix>_interrupt` (see [`Interrupt`]).
    Interrupted = 7,
}

impl Status {
    /// Every status, in code order. The generated header walks this, so the C constants
    /// and this enum cannot drift apart.
    pub const ALL: &'static [Status] = &[
        Status::Ok,
        Status::Err,
        Status::BadHandle,
        Status::NotFound,
        Status::Lua,
        Status::Panic,
        Status::WrongThread,
        Status::Interrupted,
    ];

    /// The `int` that crosses.
    pub const fn code(self) -> c_int {
        self as c_int
    }

    /// The word that names it in the header, without the prefix (`OK`, `BAD_HANDLE`).
    pub const fn name(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Err => "ERR",
            Status::BadHandle => "BAD_HANDLE",
            Status::NotFound => "NOT_FOUND",
            Status::Lua => "LUA",
            Status::Panic => "PANIC",
            Status::WrongThread => "WRONG_THREAD",
            Status::Interrupted => "INTERRUPTED",
        }
    }
}

// ------------------------------------------------------------------ last error

thread_local! {
    /// The slot `<prefix>_last_error()` reads. A `CString` rather than a `String`
    /// because the pointer we hand out has to stay NUL-terminated and stay put until the
    /// next call replaces it.
    static LAST: RefCell<Option<CString>> = const { RefCell::new(None) };
    static LAST_STATUS: Cell<c_int> = const { Cell::new(0) };
}

/// Record a failure on this thread. Interior NULs in the message are replaced rather
/// than refused: an error message is not the place to fail a second time.
pub fn set_error(status: Status, msg: impl fmt::Display) {
    let text = msg.to_string().replace('\0', "?");
    LAST.with(|c| *c.borrow_mut() = CString::new(text).ok());
    LAST_STATUS.with(|c| c.set(status.code()));
}

/// Forget the last error on this thread. Every entry point does this before it runs, so
/// a message never outlives the call that produced it.
pub fn clear_error() {
    LAST.with(|c| *c.borrow_mut() = None);
    LAST_STATUS.with(|c| c.set(Status::Ok.code()));
}

/// The last error on this thread, or `NULL` if the last call succeeded.
///
/// # Safety
///
/// The pointer is owned by this library and is valid until the next call on this thread
/// — the caller copies what it needs and does **not** free it. A host that cannot hold a
/// foreign pointer that long uses [`last_error_into`] instead.
pub fn last_error_ptr() -> *const c_char {
    LAST.with(|c| match &*c.borrow() {
        Some(s) => s.as_ptr(),
        None => std::ptr::null(),
    })
}

/// The status that went with the last error ([`Status::Ok`] if there was none). This is
/// how a `char *`-returning function, whose failure is a bare `NULL`, says which kind of
/// failure it was.
pub fn last_status() -> c_int {
    LAST_STATUS.with(|c| c.get())
}

/// Copy the last error into the caller's buffer, NUL-terminated and truncated to fit.
///
/// Returns the length of the message in bytes, not counting the NUL — so a return of
/// `len` or more means it was truncated and a bigger buffer would hold it. `-1` means
/// the arguments were unusable (`buf` is `NULL`, or `len` is not positive).
///
/// # Safety
///
/// `buf` must be writable for `len` bytes.
pub unsafe fn last_error_into(buf: *mut c_char, len: c_int) -> c_int {
    if buf.is_null() || len <= 0 {
        return -1;
    }
    let cap = len as usize;
    LAST.with(|c| {
        let borrowed = c.borrow();
        let bytes = borrowed.as_ref().map(|s| s.as_bytes()).unwrap_or(b"");
        let n = bytes.len().min(cap - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
        bytes.len() as c_int
    })
}

// ------------------------------------------------------------------ strings

/// Hand a string to the caller as a `char *` it must return to `<prefix>_free`.
///
/// A string with an interior NUL is **refused** — the error is recorded and this answers
/// `NULL` — rather than silently truncated at the NUL or stripped of it. Both of those
/// hand the caller a different string than the one the host produced, and a JSON payload
/// that lost a byte in the middle is worse than a call that failed.
pub fn give(s: impl Into<Vec<u8>>) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(e) => {
            set_error(
                Status::Err,
                format!(
                    "the value contains a NUL byte at index {} and cannot cross as a C string",
                    e.nul_position()
                ),
            );
            std::ptr::null_mut()
        }
    }
}

/// Free a string that came from [`give`]. A `NULL` pointer is a no-op, so a caller may
/// free unconditionally in a `finally` / `defer` without checking first.
///
/// # Safety
///
/// `p` must be `NULL` or a pointer this library returned and that has not been freed
/// yet. Anything else — a pointer from `malloc`, a string literal, a second free — is
/// undefined behaviour, and no check here can catch it.
pub unsafe fn free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}

/// Borrow a `const char *` argument as `&str` for the length of the call.
///
/// `None` (with the error recorded) if the pointer is `NULL` or the bytes are not UTF-8;
/// `what` names the argument in that message. The lifetime is the caller's to keep
/// short: the generated wrappers copy or parse before returning.
///
/// # Safety
///
/// `p` must be `NULL` or a NUL-terminated string that stays alive and unmodified for the
/// duration of the call.
pub unsafe fn arg_str<'a>(p: *const c_char, what: &str) -> Option<&'a str> {
    if p.is_null() {
        set_error(Status::Err, format!("argument `{what}` is NULL"));
        return None;
    }
    match unsafe { CStr::from_ptr(p) }.to_str() {
        Ok(s) => Some(s),
        Err(e) => {
            set_error(Status::Err, format!("argument `{what}` is not UTF-8: {e}"));
            None
        }
    }
}

/// Serialize a value as the JSON that crosses the boundary.
///
/// An object payload gains `"v": PAYLOAD_VERSION` unless it already has a `v` of its
/// own, so a caller can tell which schema it is reading without the host remembering to
/// add the field to every record. Anything that is not an object (an array, a number)
/// crosses as it is — there is nowhere to put the version, and a payload that needs one
/// should be an object.
pub fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<String, String> {
    let mut v = serde_json::to_value(value).map_err(|e| format!("serializing the result: {e}"))?;
    if let serde_json::Value::Object(map) = &mut v {
        map.entry("v")
            .or_insert_with(|| serde_json::Value::from(PAYLOAD_VERSION));
    }
    serde_json::to_string(&v).map_err(|e| format!("serializing the result: {e}"))
}

/// [`json`] then [`give`]: what a wrapper whose Rust return type is a record does. The
/// two failures it can have — a value that will not serialize, and one whose JSON holds
/// a NUL — are recorded, and the caller gets `NULL`.
pub fn give_json<T: serde::Serialize + ?Sized>(value: &T) -> *mut c_char {
    match json(value) {
        Ok(s) => give(s),
        Err(e) => {
            set_error(Status::Err, e);
            std::ptr::null_mut()
        }
    }
}

/// Parse a JSON argument. `what` names the argument in the error.
pub fn from_json<T: serde::de::DeserializeOwned>(s: &str, what: &str) -> Result<T, String> {
    serde_json::from_str(s).map_err(|e| format!("argument `{what}` is not valid JSON: {e}"))
}

/// Record a failed call and answer the [`Status`] that goes with it.
///
/// The status is read off the error's own type rather than off its text, as far as the
/// types this crate knows reach: an `mlua::Error` is [`Status::Lua`] (and
/// [`Status::Interrupted`] if it is the one [`Interrupt`] raises), a `NotFound` I/O error
/// is [`Status::NotFound`], an `anyhow::Error` is whichever of those it wraps, and
/// anything else is [`Status::Err`] with its `Display` as the message. A caller that
/// wants a finer distinction reads the message.
pub fn fail<E: fmt::Display + Any>(e: E) -> Status {
    let (status, message) = classify(&e as &dyn Any);
    set_error(status, message.unwrap_or_else(|| e.to_string()));
    status
}

/// The status, and the message when the error's own `Display` is not the one to show.
///
/// An mlua error's `Display` is the innermost cause plus Lua's `stack traceback:` block,
/// which is for a log and not for the message a host puts in front of a player;
/// [`crate::user_message_lua`] drops the frames and is what crosses here. A C caller is
/// an embedding host, so this side takes the same answer the Rust side's
/// [`crate::user_message`] gives — not [`crate::developer_message`], which is what the
/// development commands `htl run` and `htl test` print.
fn classify(any: &dyn Any) -> (Status, Option<String>) {
    if let Some(m) = any.downcast_ref::<mlua::Error>() {
        return (lua_status(m), Some(crate::user_message_lua(m)));
    }
    if let Some(io) = any.downcast_ref::<std::io::Error>() {
        return (io_status(io), None);
    }
    if let Some(a) = any.downcast_ref::<anyhow::Error>() {
        if let Some(m) = a.downcast_ref::<mlua::Error>() {
            return (lua_status(m), Some(crate::user_message(a)));
        }
        if let Some(io) = a.downcast_ref::<std::io::Error>() {
            return (io_status(io), Some(crate::user_message(a)));
        }
        return (Status::Err, Some(crate::user_message(a)));
    }
    (Status::Err, None)
}

fn lua_status(e: &mlua::Error) -> Status {
    if e.downcast_ref::<Interrupted>().is_some() {
        Status::Interrupted
    } else {
        Status::Lua
    }
}

fn io_status(e: &std::io::Error) -> Status {
    if e.kind() == std::io::ErrorKind::NotFound {
        Status::NotFound
    } else {
        Status::Err
    }
}

/// The error [`Interrupt`] raises inside Lua. A concrete type rather than a message, so
/// that [`fail`] can recognise it after mlua has wrapped it in its own layers and answer
/// [`Status::Interrupted`] instead of [`Status::Lua`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

// ------------------------------------------------------------------ panics

/// Run `f`, catching a panic and answering `sentinel` instead of unwinding into C.
///
/// The panic message becomes the last error with [`Status::Panic`]. Used for the entry
/// points that have no handle to poison — `<prefix>_open` and the version functions;
/// everything that takes a handle goes through [`Handle::enter`], which poisons it too.
///
/// Unwind safety is asserted rather than proved: what a caught panic may have left
/// half-written is a `Handle`, and the answer to that is the poison flag, not a
/// `RefCell`-shaped bound on every closure the macro writes.
pub fn guard<R>(sentinel: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(p) => {
            set_error(Status::Panic, panic_message(&p));
            sentinel
        }
    }
}

/// The best text available for a caught panic. `panic!("...")` arrives as a `String` or
/// a `&str`; anything else (a `panic_any`) has no text at all.
fn panic_message(p: &Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("panic: {s}")
    } else {
        "panic: (no message)".to_string()
    }
}

// ------------------------------------------------------------------ interrupt

/// The flag `<prefix>_interrupt` sets and a Lua hook reads.
///
/// `mlua::Lua::set_interrupt` is Luau-only, so on Lua 5.4 the route is a debug hook that
/// runs every *n*th instruction: cheap when nothing is set, and the only preemption
/// point a plain Lua VM offers. Install it once, on the thread that owns the state,
/// while the handle is being opened — the opener is handed one of these.
///
/// Firing consumes the flag: one `<prefix>_interrupt` stops one run, and the next call
/// starts clean. A flag set while nothing is running stops the next thing that runs,
/// which is what a host clicking "stop" between frames means by it.
#[derive(Clone, Debug)]
pub struct Interrupt(Arc<AtomicBool>);

/// How often the hook looks at the flag. Small enough that a tight `while true do end`
/// stops in well under a frame, large enough not to show up in a profile.
const HOOK_EVERY: u32 = 10_000;

impl Interrupt {
    /// Ask for the run in progress to stop. Callable from any thread.
    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether a stop is pending.
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Drop a pending stop.
    pub fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    /// Install the hook that turns this flag into a Lua error.
    ///
    /// Call it from the opener, on the state the handle will run. Replaces any hook
    /// already on the state — a Lua state has one.
    pub fn install(&self, h: &Htl) -> mlua::Result<()> {
        self.install_every(h, HOOK_EVERY)
    }

    /// [`Interrupt::install`] with the instruction count spelled out, for a host that
    /// has measured its own scripts.
    pub fn install_every(&self, h: &Htl, every: u32) -> mlua::Result<()> {
        let flag = self.0.clone();
        h.lua().set_hook(
            mlua::HookTriggers::new().every_nth_instruction(every),
            move |_lua, _debug| {
                if flag.swap(false, Ordering::SeqCst) {
                    Err(mlua::Error::external(Interrupted))
                } else {
                    Ok(mlua::VmState::Continue)
                }
            },
        )
    }
}

// ------------------------------------------------------------------ handle

/// The part of a handle that another thread may look at: never mutated after the handle
/// is built, so `<prefix>_interrupt` can read it while the owning thread is inside a
/// call.
#[derive(Debug)]
struct Shared {
    magic: u64,
    owner: ThreadId,
    interrupt: Arc<AtomicBool>,
}

/// The part only the owning thread touches.
struct Slot<T> {
    value: T,
    /// A panic came out of a call on this handle; nothing may run on it again.
    poisoned: bool,
    /// A call is in progress. Re-entering (a Lua callback that calls back into the C
    /// ABI on the same handle) would alias the `&mut` below, so it is refused.
    entered: bool,
}

/// What `<prefix>_open` returns and every other function takes: an owning pointer to
/// one `T`, plus the thread it belongs to and the flag that stops it.
///
/// The caller sees `typedef struct <prefix>_handle <prefix>_handle;` — an incomplete
/// type it can only hold a pointer to. On this side it is a `Box` this module made, and
/// the `magic` word means a pointer that never came from `<prefix>_open` is answered
/// with [`Status::BadHandle`] rather than dereferenced.
pub struct Handle<T> {
    shared: Shared,
    slot: UnsafeCell<Slot<T>>,
}

/// A per-prefix word stamped into every handle, so that a pointer from another library
/// (or a stale one from a previous `dlopen`) is caught instead of followed. FNV-1a over
/// the prefix: `const` so the macro can put the result in a constant.
pub const fn magic(prefix: &str) -> u64 {
    let b = prefix.as_bytes();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    // Never zero: a zeroed allocation must not read as a valid handle.
    if h == 0 { 1 } else { h }
}

impl<T> Handle<T> {
    /// Build the value and hand back the pointer the caller holds, or `NULL` with the
    /// error recorded.
    ///
    /// `make` gets the [`Interrupt`] for this handle before the value exists, which is
    /// the only order that works: the flag has to live in the handle's own allocation
    /// for another thread to be allowed to touch it, and the state the hook goes on
    /// exists only once `make` has built it.
    pub fn open(magic: u64, make: impl FnOnce(Interrupt) -> Result<T, String>) -> *mut c_void {
        guard(std::ptr::null_mut(), || {
            clear_error();
            let interrupt = Interrupt(Arc::new(AtomicBool::new(false)));
            match make(interrupt.clone()) {
                Ok(value) => {
                    let h = Box::new(Handle {
                        shared: Shared {
                            magic,
                            owner: std::thread::current().id(),
                            interrupt: interrupt.0,
                        },
                        slot: UnsafeCell::new(Slot {
                            value,
                            poisoned: false,
                            entered: false,
                        }),
                    });
                    Box::into_raw(h) as *mut c_void
                }
                Err(e) => {
                    set_error(Status::Err, e);
                    std::ptr::null_mut()
                }
            }
        })
    }

    /// Check the pointer and run `f` on the value, or record why not and answer
    /// `sentinel`.
    ///
    /// The checks, in order: not `NULL`, stamped with this library's `magic`, opened by
    /// this thread, not poisoned by an earlier panic, not already inside a call. A panic
    /// out of `f` is caught, recorded, and poisons the handle.
    ///
    /// # Safety
    ///
    /// `ptr` must be `NULL` or a pointer from [`Handle::open`] with the same `T` that
    /// has not been closed. A pointer to a `Handle<U>` that happens to carry the same
    /// magic is undefined behaviour; the magic is per prefix, and a prefix names one
    /// type.
    pub unsafe fn enter<R>(
        ptr: *mut c_void,
        magic: u64,
        sentinel: R,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        clear_error();
        let Some(h) = (unsafe { Self::check(ptr, magic) }) else {
            return sentinel;
        };
        let slot = h.slot.get();
        // SAFETY: `check` established that this thread owns the handle, and `entered`
        // rules out a second `&mut` from a re-entrant call on the same thread. Nothing
        // else can be looking at the slot.
        unsafe {
            if (*slot).poisoned {
                set_error(
                    Status::Panic,
                    "the handle was poisoned by an earlier panic and cannot be used",
                );
                return sentinel;
            }
            if (*slot).entered {
                set_error(
                    Status::Err,
                    "the handle is already inside a call (re-entering it is not supported)",
                );
                return sentinel;
            }
            (*slot).entered = true;
            let out = catch_unwind(AssertUnwindSafe(|| f(&mut (*slot).value)));
            (*slot).entered = false;
            match out {
                Ok(v) => v,
                Err(p) => {
                    (*slot).poisoned = true;
                    set_error(Status::Panic, panic_message(&p));
                    sentinel
                }
            }
        }
    }

    /// [`Handle::enter`] for a wrapper that returns a status: when the handle itself is
    /// the problem, the status is the one the check recorded rather than a fixed
    /// sentinel, so `BAD_HANDLE` and `WRONG_THREAD` are told apart by the caller.
    ///
    /// A status is never negative, which is what makes `-1` usable as the sentinel here.
    ///
    /// # Safety
    ///
    /// As [`Handle::enter`].
    pub unsafe fn enter_status(
        ptr: *mut c_void,
        magic: u64,
        f: impl FnOnce(&mut T) -> c_int,
    ) -> c_int {
        let out = unsafe { Self::enter(ptr, magic, -1, f) };
        if out < 0 { last_status() } else { out }
    }

    /// Ask the run in progress on this handle to stop, from any thread.
    ///
    /// Reads only the handle's shared state, which nothing mutates after the handle is
    /// built, so this
    /// does not race with the owning thread being inside [`Handle::enter`]. It answers
    /// [`Status::Ok`] once the flag is set — whether anything was running, and whether
    /// the opener installed the hook, is not knowable from here.
    ///
    /// # Safety
    ///
    /// `ptr` must be `NULL` or a live pointer from [`Handle::open`]. Interrupting a
    /// handle while another thread closes it is a use-after-free that no check here can
    /// catch: close on the owning thread, after the caller has stopped calling.
    pub unsafe fn interrupt(ptr: *mut c_void, magic: u64) -> c_int {
        clear_error();
        if ptr.is_null() {
            set_error(Status::BadHandle, "the handle is NULL");
            return Status::BadHandle.code();
        }
        let h = unsafe { &*(ptr as *const Handle<T>) };
        if h.shared.magic != magic {
            set_error(
                Status::BadHandle,
                "the handle was not returned by this library",
            );
            return Status::BadHandle.code();
        }
        h.shared.interrupt.store(true, Ordering::SeqCst);
        Status::Ok.code()
    }

    /// Drop the value and free the handle. A `NULL` pointer is a no-op.
    ///
    /// Closing from a thread that does not own the handle does nothing and records
    /// [`Status::WrongThread`]: `T` holds a `Lua`, which may not be dropped anywhere but
    /// where it was made.
    ///
    /// # Safety
    ///
    /// `ptr` must be `NULL` or a pointer from [`Handle::open`] that has not been closed,
    /// and no other thread may be inside a call on it.
    pub unsafe fn close(ptr: *mut c_void, magic: u64) {
        clear_error();
        if unsafe { Self::check(ptr, magic) }.is_none() {
            return;
        }
        // The value's `Drop` runs here: a panic in it must not cross into C either.
        guard((), || {
            drop(unsafe { Box::from_raw(ptr as *mut Handle<T>) });
        });
    }

    /// The three cheap checks shared by `enter` and `close`, each recording its own
    /// error. `None` means the caller answers its sentinel.
    ///
    /// # Safety
    ///
    /// As [`Handle::enter`].
    unsafe fn check<'a>(ptr: *mut c_void, magic: u64) -> Option<&'a Handle<T>> {
        if ptr.is_null() {
            set_error(Status::BadHandle, "the handle is NULL");
            return None;
        }
        let h = unsafe { &*(ptr as *const Handle<T>) };
        if h.shared.magic != magic {
            set_error(
                Status::BadHandle,
                "the handle was not returned by this library",
            );
            return None;
        }
        if h.shared.owner != std::thread::current().id() {
            set_error(
                Status::WrongThread,
                "the handle belongs to the thread that opened it; \
                 open one per thread (see <prefix>_threadsafe)",
            );
            return None;
        }
        Some(h)
    }
}

/// What `#[c_export]` implements on the type it is written on: the identity of the
/// generated ABI, available to Rust code (a test, a build script that copies the header)
/// without reading the header file back.
pub trait CExport {
    /// The prefix every generated symbol carries (`hello` -> `hello_open`).
    const PREFIX: &'static str;
    /// The generated header, exactly as `header = "..."` writes it.
    const HEADER: &'static str;
    /// [`ABI_VERSION`] as this library was built with it.
    const ABI_VERSION: c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The status codes are what the header will say they are: a renumbering here is a
    /// silent break for every caller already compiled against the old header, so the
    /// numbers are pinned by a test rather than by the declaration order.
    #[test]
    fn status_codes_are_pinned() {
        let pairs: Vec<(&str, c_int)> = Status::ALL.iter().map(|s| (s.name(), s.code())).collect();
        assert_eq!(
            pairs,
            vec![
                ("OK", 0),
                ("ERR", 1),
                ("BAD_HANDLE", 2),
                ("NOT_FOUND", 3),
                ("LUA", 4),
                ("PANIC", 5),
                ("WRONG_THREAD", 6),
                ("INTERRUPTED", 7),
            ]
        );
    }

    #[test]
    fn give_refuses_an_interior_nul_and_says_where() {
        let p = give("ab\0cd".to_string());
        assert!(p.is_null(), "a string with a NUL must not cross");
        assert_eq!(last_status(), Status::Err.code());
        let msg = unsafe { CStr::from_ptr(last_error_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(msg.contains("NUL byte at index 2"), "{msg}");
    }

    #[test]
    fn free_of_null_is_a_no_op() {
        unsafe { free(std::ptr::null_mut()) };
    }

    #[test]
    fn a_round_trip_through_give_and_free_keeps_the_bytes() {
        let p = give("hello".to_string());
        assert!(!p.is_null());
        assert_eq!(unsafe { CStr::from_ptr(p) }.to_str().unwrap(), "hello");
        unsafe { free(p) };
    }

    #[test]
    fn last_error_into_truncates_and_reports_the_full_length() {
        set_error(Status::Err, "0123456789");
        let mut buf = [0i8; 4];
        let n = unsafe { last_error_into(buf.as_mut_ptr() as *mut c_char, 4) };
        assert_eq!(n, 10, "the length asked for is the whole message");
        let got = unsafe { CStr::from_ptr(buf.as_ptr() as *const c_char) };
        assert_eq!(got.to_str().unwrap(), "012");
        assert_eq!(
            unsafe { last_error_into(std::ptr::null_mut(), 4) },
            -1,
            "a NULL buffer is refused, not written"
        );
    }

    #[test]
    fn json_stamps_the_payload_version_on_an_object_only() {
        #[derive(serde::Serialize)]
        struct P {
            depth: i32,
        }
        assert_eq!(json(&P { depth: 3 }).unwrap(), r#"{"depth":3,"v":1}"#);
        assert_eq!(json(&[1, 2, 3]).unwrap(), "[1,2,3]");
    }

    #[test]
    fn guard_turns_a_panic_into_the_sentinel_and_a_message() {
        let n = guard(-1i32, || panic!("boom"));
        assert_eq!(n, -1);
        assert_eq!(last_status(), Status::Panic.code());
        let msg = unsafe { CStr::from_ptr(last_error_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(msg.contains("boom"), "{msg}");
    }

    #[test]
    fn magic_is_per_prefix_and_never_zero() {
        assert_ne!(magic("hello"), magic("hell"));
        assert_ne!(magic(""), 0);
    }
}

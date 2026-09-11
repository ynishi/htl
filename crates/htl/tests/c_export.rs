//! The C ABI `#[c_export]` generates, exercised through the generated symbols
//! themselves rather than through the Rust methods behind them.
//!
//! A C compiler is not needed for any of this: an `extern "C"` function is callable from
//! Rust, and calling it here is the same call a `dlopen`ed host makes — the same
//! decoding, the same panic guard, the same thread check. What a C program would add is
//! the linker, and that is covered by the C caller `htl new --target ffi --lib` writes
//! (`crates/htl-cli/templates/ffi/main.c`), compiled and run by `just e2e`.

#![cfg(feature = "ffi")]

use htl::{Htl, c_export, ffi};
use std::ffi::{CStr, CString, c_char, c_int, c_void};

#[derive(serde::Deserialize)]
struct Options {
    greeting: String,
}

/// What `game_state` hands back: a record, so it crosses as JSON with a `"v"`.
#[derive(serde::Serialize)]
pub struct State {
    depth: i32,
    greeting: String,
}

/// A host with one method of each shape.
pub struct Game {
    h: Htl,
    greeting: String,
    depth: i32,
}

#[c_export(prefix = "game")]
impl Game {
    /// The opener. Options as one JSON object, and the flag `game_interrupt` sets: the
    /// hook has to go on the state while it is being built, which is the only moment the
    /// handle and the state exist together.
    pub fn open(options: &str, interrupt: ffi::Interrupt) -> Result<Self, String> {
        let o: Options = ffi::from_json(options, "options_json")?;
        let h = Htl::new().map_err(|e| e.to_string())?;
        interrupt.install(&h).map_err(|e| e.to_string())?;
        Ok(Game {
            h,
            greeting: o.greeting,
            depth: 0,
        })
    }

    /// `String` -> `char *`: the text itself.
    pub fn greet(&self, who: &str) -> String {
        format!("{}, {who}", self.greeting)
    }

    /// A record -> `char *`: JSON.
    pub fn state(&self) -> State {
        State {
            depth: self.depth,
            greeting: self.greeting.clone(),
        }
    }

    /// `Result<(), E>` -> `int`: a status and nothing else.
    pub fn deeper(&mut self, by: i32) -> Result<(), String> {
        if by < 0 {
            return Err("cannot go up".into());
        }
        self.depth += by;
        Ok(())
    }

    /// `i32` -> `int` with an out-parameter: the value never rides in the status.
    pub fn depth(&self) -> i32 {
        self.depth
    }

    /// `()` -> `void`.
    pub fn reset(&mut self) {
        self.depth = 0;
    }

    /// Runs Lua, so a Lua `error()` can reach the boundary.
    pub fn run(&self, src: &str) -> Result<String, htl::mlua::Error> {
        self.h.lua().load(src).eval()
    }

    /// A Rust panic, which since 1.81 aborts the process if it reaches `extern "C"`.
    pub fn boom(&self) -> String {
        panic!("boom from Rust")
    }

    /// A string a C caller cannot be handed: the NUL is in the middle of it.
    pub fn with_nul(&self) -> String {
        "before\0after".to_string()
    }
}

// ---------------------------------------------------------------- helpers

fn options(greeting: &str) -> CString {
    CString::new(format!(r#"{{"greeting":"{greeting}"}}"#)).unwrap()
}

/// Open a handle on this thread, or explain why not.
fn open(greeting: &str) -> *mut c_void {
    let o = options(greeting);
    let h = unsafe { game_open(o.as_ptr()) };
    assert!(!h.is_null(), "open failed: {}", last_error());
    h
}

/// Take a `char *` the library returned, as a `String`, and give it back.
fn take(p: *mut c_char) -> String {
    assert!(!p.is_null(), "expected a string: {}", last_error());
    let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
    unsafe { game_free(p) };
    s
}

fn last_error() -> String {
    let p = game_last_error();
    if p.is_null() {
        return "(no error)".to_string();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

// ---------------------------------------------------------------- the ABI

#[test]
fn the_library_says_what_it_is() {
    assert_eq!(game_abi_version(), ffi::ABI_VERSION);
    assert_eq!(<Game as ffi::CExport>::PREFIX, "game");
    assert_eq!(
        game_threadsafe(),
        0,
        "one handle per thread, the sqlite3_threadsafe convention"
    );
    let v = unsafe { CStr::from_ptr(game_version()) }.to_str().unwrap();
    assert_eq!(v, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_header_declares_what_was_generated() {
    let h = <Game as ffi::CExport>::HEADER;
    for want in [
        "game_handle *game_open(const char *options_json);",
        "char *game_greet(game_handle *h, const char *who);",
        "char *game_state(game_handle *h);",
        "int game_deeper(game_handle *h, int by);",
        "int game_depth(game_handle *h, int *out);",
        "void game_reset(game_handle *h);",
        "int game_interrupt(game_handle *h);",
        "void game_free(char *s);",
        "#define GAME_LUA 4",
    ] {
        assert!(h.contains(want), "the header is missing `{want}`:\n{h}");
    }
}

#[test]
fn a_round_trip_returns_text_json_and_an_out_parameter() {
    let h = open("hello");

    assert_eq!(
        take(unsafe { game_greet(h, cstr("Ada").as_ptr()) }),
        "hello, Ada"
    );

    assert_eq!(unsafe { game_deeper(h, 3) }, 0, "{}", last_error());
    let mut depth: c_int = -1;
    assert_eq!(unsafe { game_depth(h, &mut depth) }, 0);
    assert_eq!(depth, 3, "the value comes back through the out-parameter");

    let state = take(unsafe { game_state(h) });
    let parsed: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(parsed["depth"], 3);
    assert_eq!(parsed["greeting"], "hello");
    assert_eq!(
        parsed["v"], 1,
        "an object payload carries its schema version: {state}"
    );

    unsafe { game_reset(h) };
    assert_eq!(unsafe { game_depth(h, &mut depth) }, 0);
    assert_eq!(depth, 0);

    unsafe { game_close(h) };
}

#[test]
fn a_failing_method_answers_a_status_and_a_message() {
    let h = open("hi");
    assert_eq!(
        unsafe { game_deeper(h, -1) },
        ffi::Status::Err.code(),
        "a Result::Err is a status, not a value"
    );
    assert_eq!(last_error(), "cannot go up");
    assert_eq!(game_last_status(), ffi::Status::Err.code());
    unsafe { game_close(h) };
}

#[test]
fn a_lua_error_crosses_as_a_status_and_a_message() {
    let h = open("hi");
    let p = unsafe { game_run(h, cstr("error('from the mod')").as_ptr()) };
    assert!(p.is_null(), "a failed char * function answers NULL");
    assert_eq!(
        game_last_status(),
        ffi::Status::Lua.code(),
        "an mlua error is LUA, not ERR"
    );
    assert!(
        last_error().contains("from the mod"),
        "the message is Lua's: {}",
        last_error()
    );
    assert!(
        !last_error().contains("stack traceback"),
        "and it is the one a host shows a player, not the log: {}",
        last_error()
    );
    // The handle is not poisoned by a Lua error: the state is still the state.
    assert_eq!(
        take(unsafe { game_run(h, cstr("return 'ok'").as_ptr()) }),
        "ok"
    );
    unsafe { game_close(h) };
}

#[test]
fn a_rust_panic_is_caught_and_poisons_the_handle() {
    let h = open("hi");
    let p = unsafe { game_boom(h) };
    assert!(p.is_null(), "the process must still be here");
    assert_eq!(game_last_status(), ffi::Status::Panic.code());
    assert!(
        last_error().contains("boom from Rust"),
        "the panic message is the error: {}",
        last_error()
    );
    // Everything after it is refused rather than run on a state nobody has reasoned
    // about.
    let mut depth: c_int = -1;
    assert_eq!(
        unsafe { game_depth(h, &mut depth) },
        ffi::Status::Panic.code()
    );
    assert!(
        last_error().contains("poisoned"),
        "and it says so: {}",
        last_error()
    );
    unsafe { game_close(h) };
}

#[test]
fn a_handle_used_from_another_thread_answers_wrong_thread() {
    let h = open("hi");
    let addr = h as usize;
    let (status, message) = std::thread::spawn(move || {
        let h = addr as *mut c_void;
        let p = unsafe { game_greet(h, cstr("Ada").as_ptr()) };
        assert!(p.is_null(), "nothing was run, so nothing came back");
        // The error slot is per thread: this is the one this thread just filled.
        (game_last_status(), last_error())
    })
    .join()
    .unwrap();
    assert_eq!(status, ffi::Status::WrongThread.code());
    assert!(message.contains("thread that opened it"), "{message}");
    unsafe { game_close(h) };
}

#[test]
fn a_string_with_a_nul_is_refused_rather_than_cut_short() {
    let h = open("hi");
    let p = unsafe { game_with_nul(h) };
    assert!(p.is_null(), "half a string must not cross");
    assert_eq!(game_last_status(), ffi::Status::Err.code());
    assert!(
        last_error().contains("NUL byte at index 6"),
        "and it says where: {}",
        last_error()
    );
    unsafe { game_close(h) };
}

#[test]
fn freeing_null_and_closing_null_are_no_ops() {
    unsafe { game_free(std::ptr::null_mut()) };
    unsafe { game_close(std::ptr::null_mut()) };
}

#[test]
fn a_bad_handle_is_answered_rather_than_dereferenced() {
    let mut not_a_handle: u64 = 0;
    let p = (&mut not_a_handle) as *mut u64 as *mut c_void;
    let mut depth: c_int = -1;
    assert_eq!(
        unsafe { game_depth(p, &mut depth) },
        ffi::Status::BadHandle.code()
    );
    assert_eq!(
        unsafe { game_depth(std::ptr::null_mut(), &mut depth) },
        ffi::Status::BadHandle.code()
    );
}

#[test]
fn a_null_out_parameter_is_refused() {
    let h = open("hi");
    assert_eq!(
        unsafe { game_depth(h, std::ptr::null_mut()) },
        ffi::Status::Err.code()
    );
    assert!(last_error().contains("`out` is NULL"), "{}", last_error());
    unsafe { game_close(h) };
}

#[test]
fn an_argument_that_is_not_a_string_is_refused_before_the_handle_is_entered() {
    let h = open("hi");
    let p = unsafe { game_greet(h, std::ptr::null()) };
    assert!(p.is_null());
    assert!(last_error().contains("`who` is NULL"), "{}", last_error());
    // The handle is untouched: the caller's mistake is not the handle's.
    assert_eq!(
        take(unsafe { game_greet(h, cstr("Ada").as_ptr()) }),
        "hi, Ada"
    );
    unsafe { game_close(h) };
}

#[test]
fn bad_options_are_a_failed_open_rather_than_a_handle() {
    let bad = cstr("not json");
    let h = unsafe { game_open(bad.as_ptr()) };
    assert!(h.is_null());
    assert!(last_error().contains("not valid JSON"), "{}", last_error());
    let h = unsafe { game_open(std::ptr::null()) };
    assert!(h.is_null());
    assert!(last_error().contains("is NULL"), "{}", last_error());
}

/// The one function callable from another thread, doing the one thing it is for.
#[test]
fn interrupt_stops_a_runaway_script_from_another_thread() {
    let h = open("hi");
    let addr = h as usize;
    let stopper = std::thread::spawn(move || {
        // Long enough that the loop below is running, short enough not to stall the
        // suite if it is not.
        std::thread::sleep(std::time::Duration::from_millis(150));
        unsafe { game_interrupt(addr as *mut c_void) }
    });

    let started = std::time::Instant::now();
    let p = unsafe { game_run(h, cstr("while true do end").as_ptr()) };
    let took = started.elapsed();

    assert_eq!(stopper.join().unwrap(), ffi::Status::Ok.code());
    assert!(
        p.is_null(),
        "the script did not finish, so there is no value"
    );
    assert_eq!(
        game_last_status(),
        ffi::Status::Interrupted.code(),
        "an interrupt is its own status, not a generic Lua error: {}",
        last_error()
    );
    assert!(
        took < std::time::Duration::from_secs(20),
        "it stopped when it was asked to, after {took:?}"
    );

    // The handle still works: an interrupt stops one run, not the game.
    assert_eq!(
        take(unsafe { game_run(h, cstr("return 'ok'").as_ptr()) }),
        "ok"
    );
    unsafe { game_close(h) };
}

/// The status codes the header states and the ones the runtime answers with are two
/// tables in two modules behind two features. This is what keeps them one set.
#[test]
fn the_generator_and_the_runtime_agree_on_the_status_codes() {
    let runtime: Vec<(&str, i32)> = ffi::Status::ALL
        .iter()
        .map(|s| (s.name(), s.code()))
        .collect();
    assert_eq!(runtime, htl::cexport::STATUSES.to_vec());
    assert_eq!(ffi::ABI_VERSION, htl::cexport::ABI_VERSION);
}

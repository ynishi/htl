#!/usr/bin/env python3
"""A Python caller for lib{{mod}}, and the shape a ctypes host has to keep.

What Python gets wrong here by default: `restype = c_char_p` looks right and is the one
mistake this ABI cannot survive. ctypes copies such a return into a `bytes` and throws
the pointer away, so the string this library allocated is leaked on every single call and
there is nothing left to hand to `{{mod}}_free`. The fix is two lines, and both of them
are below:

    fn.restype = ctypes.c_void_p              # keep the address, not a copy
    text = ctypes.cast(p, ctypes.c_char_p).value.decode()   # read it, then free `p`

Run it (after `cargo build` in the project root):

    python3 examples/python/run.py [path/to/lib{{mod}}.so]
"""

import ctypes
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def library_path() -> Path:
    """The build cargo left behind: an argument if one was given, else release if it is
    there and debug otherwise, under CARGO_TARGET_DIR when that is set."""
    if len(sys.argv) > 1:
        return Path(sys.argv[1])
    target = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target")
    names = ["lib{{mod}}.so", "lib{{mod}}.dylib", "{{mod}}.dll"]
    for profile in ("release", "debug"):
        for name in names:
            path = target / profile / name
            if path.exists():
                return path
    sys.exit(f"no built library under {target}; run `cargo build` first")


def declare(lib: ctypes.CDLL) -> None:
    """Every signature, spelled out. ctypes assumes `int` for anything it is not told
    about, which on a 64-bit host truncates a returned pointer to its low 32 bits."""
    p = ctypes.c_void_p
    cs = ctypes.c_char_p
    i = ctypes.c_int
    lib.{{mod}}_abi_version.restype = i
    lib.{{mod}}_version.restype = cs                 # borrowed, static: not ours to free
    lib.{{mod}}_threadsafe.restype = i
    lib.{{mod}}_last_error.restype = cs              # borrowed until the next call
    lib.{{mod}}_last_status.restype = i
    lib.{{mod}}_free.argtypes = [p]
    lib.{{mod}}_open.argtypes, lib.{{mod}}_open.restype = [cs], p
    lib.{{mod}}_close.argtypes = [p]
    lib.{{mod}}_greet.argtypes, lib.{{mod}}_greet.restype = [p, cs], p
    lib.{{mod}}_state.argtypes, lib.{{mod}}_state.restype = [p], p
    lib.{{mod}}_reset.argtypes, lib.{{mod}}_reset.restype = [p], i
    lib.{{mod}}_greeted.argtypes = [p, ctypes.POINTER(i)]
    lib.{{mod}}_greeted.restype = i


def why(lib: ctypes.CDLL) -> str:
    m = lib.{{mod}}_last_error()
    return m.decode() if m else "(no message)"


def take(lib: ctypes.CDLL, p):
    """Read a `char *` the library returned and give it back. `p` is an address (an int),
    because restype is c_void_p; casting is what reads the bytes without losing it."""
    if not p:
        return None
    try:
        return ctypes.cast(p, ctypes.c_char_p).value.decode()
    finally:
        lib.{{mod}}_free(p)  # in a finally: a decode error must not leak the string


def main() -> int:
    path = library_path()
    lib = ctypes.CDLL(str(path))
    declare(lib)

    shown = os.path.relpath(path, ROOT)
    print(f"library        -> {path if shown.startswith('..') else shown}")
    print(f"abi_version    -> {lib.{{mod}}_abi_version()}")
    print(f"version        -> {lib.{{mod}}_version().decode()}")
    print(f"threadsafe     -> {lib.{{mod}}_threadsafe()} (0 = one handle per thread)")

    # Options are one JSON object: what the library would otherwise read from the
    # environment or the working directory is passed in instead.
    options = json.dumps({"greeter": "the Python host"}).encode()
    handle = lib.{{mod}}_open(options)
    if not handle:
        sys.exit(f"open failed: {why(lib)}")

    try:
        print(f"greet          -> {take(lib, lib.{{mod}}_greet(handle, b'Ada'))}")

        # A record crosses as JSON text, with the schema version the runtime stamps on it.
        state = json.loads(take(lib, lib.{{mod}}_state(handle)))
        print(f"state          -> {state} (schema v{state['v']})")

        # An int is a status, never a value: the count comes through the out-parameter.
        greeted = ctypes.c_int(-1)
        if lib.{{mod}}_greeted(handle, ctypes.byref(greeted)) == 0:
            print(f"greeted        -> {greeted.value}")

        # Error path 1: the Teal module refuses an empty name, so the failure is Lua's and
        # comes back as NULL with the LUA status (4) rather than as an empty string.
        if take(lib, lib.{{mod}}_greet(handle, b"")) is None:
            print(f'greet("")      -> NULL, status {lib.{{mod}}_last_status()}: {why(lib)}')

        # Error path 2: a function whose return *is* the status. The second reset has
        # nothing to do and says so.
        print(f"reset          -> status {lib.{{mod}}_reset(handle)}")
        status = lib.{{mod}}_reset(handle)
        print(f"reset again    -> status {status}: {why(lib)}")
    finally:
        lib.{{mod}}_close(handle)
        print("closed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

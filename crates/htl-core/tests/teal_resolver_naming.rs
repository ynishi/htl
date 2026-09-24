//! Which file a `TealResolver` serves for a name: the naming rule's (`htl_core::naming`),
//! the one `htl check` names files by.
//!
//! The resolver used to try its own list — `<name>.tl`, `<name>/init.tl`,
//! `<name>/<last>.tl`, `<name>.d.tl`, first hit wins — and the checker a `package.path`
//! template list with the same `?/?` in it. Both made a host's `scripts/util/util.tl`
//! answer to `util` as well as `util.util`, and served whichever of `util.tl` and
//! `util/init.tl` came first in the list. The `<name>/<name>.tl` spelling is a flat
//! package's entry; it belongs to a directory of packages and nowhere else.

use htl_core::Htl;
use htl_core::pkg::TealResolver;
use mlua_pkg::Registry;
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-resolver-naming", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn host(resolver: TealResolver) -> Htl {
    let h = Htl::new().unwrap();
    let mut reg = Registry::new();
    reg.add(resolver);
    reg.install(h.lua()).unwrap();
    h
}

/// The value of `require(name).n`, or the error text.
fn n_of(h: &Htl, name: &str) -> Result<i64, String> {
    h.lua()
        .load(format!("return require('{name}').n"))
        .eval::<i64>()
        .map_err(|e| e.to_string())
}

#[test]
fn a_file_named_after_its_directory_is_a_submodule_of_a_script_directory() {
    let dir = scratch("top");
    write(&dir.join("util/util.tl"), "return { n = 1 }\n");
    write(
        &dir.join("main.tl"),
        "local util = require(\"util\")\nreturn { n = util.n as integer }\n",
    );
    write(
        &dir.join("fine.tl"),
        "local util = require(\"util.util\")\nreturn { n = util.n + 1 }\n",
    );
    let h = host(TealResolver::new(&dir).unwrap());

    assert_eq!(n_of(&h, "util.util"), Ok(1));
    assert_eq!(n_of(&h, "fine"), Ok(2));
    let util = n_of(&h, "util").unwrap_err();
    assert!(util.contains("not found"), "{util}");
    // The checker reads the directory the same way: what `main` requires is not there.
    let main = n_of(&h, "main").unwrap_err();
    assert!(main.contains("module not found: 'util'"), "{main}");
}

#[test]
fn a_directory_of_packages_reads_a_flat_package_by_its_entry() {
    let dir = scratch("packages");
    write(
        &dir.join("mathx/mathx.tl"),
        "local sub = require(\"mathx.sub\")\nreturn { n = sub.n * 2 }\n",
    );
    write(&dir.join("mathx/sub.tl"), "return { n = 21 }\n");
    let h = host(TealResolver::new(&dir).unwrap().holding_packages());

    assert_eq!(n_of(&h, "mathx"), Ok(42));
    assert_eq!(n_of(&h, "mathx.sub"), Ok(21));
}

#[test]
fn two_implementations_of_one_name_are_an_error() {
    let dir = scratch("ambiguous");
    write(&dir.join("util.tl"), "return { n = 1 }\n");
    write(&dir.join("util/init.tl"), "return { n = 2 }\n");
    let h = host(TealResolver::new(&dir).unwrap());

    let e = n_of(&h, "util").unwrap_err();
    assert!(
        e.contains("module 'util' is implemented by more than one file")
            && e.contains("util.tl")
            && e.contains("init.tl"),
        "{e}"
    );
}

//! `htl.trait`: `impl(defaults, target)` copies the defaults a record does not hold, the
//! checker types it so that a record that is not `is I` is refused, and a bundle carries it
//! as a module of its own rather than asking the host for it.
//!
//! The program is the example the module's declaration shows. It leaves its result in a
//! global instead of printing it, so that a test reads it back without capturing stdout;
//! the same program through the installed binary is `htl-cli`'s `trait_cli.rs`.

use htl_core::Htl;
use htl_core::bundle::{Bundle, Kind, Module};
use htl_core::link::{LinkOptions, link};
use mlua::{Function, Table, Value};
use std::path::{Path, PathBuf};

mod common;

fn tempdir(name: &str) -> common::TempDir {
    common::tempdir("htl-core-trait", name)
}

fn write(path: &Path, text: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    path.to_path_buf()
}

/// `Shape` with one default (`describe`, over `area`), `Circle` implementing `area`, and
/// whatever `circle_extra` adds to `Circle` before `impl` runs. The result is `OUT`.
fn program(circle_extra: &str) -> String {
    format!(
        r#"local trait = require("htl.trait")

local interface Shape
   area: function(self): number
   describe: function(self): string
end

local Shape_default: Shape = {{
   describe = function(self: Shape): string
      return ("area=%.2f"):format(self:area())
   end,
}}

local record Circle is Shape
   r: number
end
function Circle:area(): number return math.pi * self.r ^ 2 end
{circle_extra}
trait.impl(Shape_default, Circle)

local c: Circle = setmetatable({{ r = 1.0 }}, {{ __index = Circle }})
;(_G as {{string:any}})["OUT"] = c:describe()
"#
    )
}

/// A checker the way a project's is set up as far as this module goes: the declaration on
/// the search path. The runtime half is `Htl::new`'s.
fn checker() -> Htl {
    let h = Htl::new().unwrap();
    h.install_trait_lib().unwrap();
    h
}

fn out(h: &Htl) -> String {
    h.lua().globals().get::<String>("OUT").unwrap()
}

/// Acceptance 1: the default lands on the record, and a value reaches it as it reaches the
/// record's own methods.
#[test]
fn a_default_is_copied_onto_the_record() {
    let root = tempdir("copied");
    let main = write(&root.join("main.tl"), &program(""));
    let h = checker();
    let ci = h.run_file(&main, &[]).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert_eq!(out(&h), "area=3.14");
}

/// Acceptance 2: a method the record defines itself is the record's, whatever the defaults
/// say.
#[test]
fn impl_does_not_overwrite_what_the_record_defines() {
    let root = tempdir("keeps");
    let main = write(
        &root.join("main.tl"),
        &program("function Circle:describe(): string return \"circle\" end"),
    );
    let h = checker();
    let ci = h.run_file(&main, &[]).unwrap();
    assert!(ci.errors.is_empty(), "{:?}", ci.errors);
    assert_eq!(out(&h), "circle");
}

/// Acceptance 3: `impl: function<I>(defaults: I, target: I)` refuses a record that does not
/// implement the interface, without a cast anywhere in the program above.
#[test]
fn a_record_that_is_not_the_interface_is_a_type_error() {
    let root = tempdir("refused");
    let src = program("")
        .replace(
            "local c: Circle",
            "local record Other\n   r: number\nend\ntrait.impl(Shape_default, Other)\n\nlocal c: Circle",
        );
    let main = write(&root.join("main.tl"), &src);
    let h = checker();
    let ci = h.check(&main).unwrap();
    assert!(
        ci.errors
            .iter()
            .any(|e| e.contains("got Other, expected interface Shape")),
        "{:?}",
        ci.errors
    );
}

/// `impl` reads and writes raw: a `__index` on the target does not make a key "held", and a
/// `__newindex` does not see the copy. Neither metatable is touched.
#[test]
fn impl_is_raw_and_leaves_metatables_alone() {
    let h = Htl::new().unwrap();
    let n: i64 = h
        .lua()
        .load(
            r#"
            local trait = require("htl.trait")
            local seen = 0
            local mt = {
               __index = function() seen = seen + 1 return "via __index" end,
               __newindex = function() seen = seen + 1 end,
            }
            local target = setmetatable({ kept = "mine", off = false }, mt)
            trait.impl({ kept = "default", off = true, added = "default" }, target)
            assert(rawget(target, "kept") == "mine", "overwrote a held key")
            assert(rawget(target, "off") == false, "overwrote a false")
            assert(rawget(target, "added") == "default", "did not copy")
            assert(getmetatable(target) == mt, "changed the metatable")
            return seen
            "#,
        )
        .eval()
        .unwrap();
    assert_eq!(n, 0, "a metamethod ran");
}

/// Acceptance 5, the link half: a program that requires `htl.trait` has its source in the
/// bundle as a module, and the bundle asks the host for nothing.
#[test]
fn a_bundle_carries_the_module_and_does_not_list_it_as_the_hosts() {
    let root = tempdir("linked");
    let main = write(&root.join("src/main.tl"), &program(""));
    let h = checker();
    h.add_path(&root.join("src")).unwrap();
    let linked = link(&h, &main, &LinkOptions::default()).unwrap();
    assert!(linked.errors.is_empty(), "{:?}", linked.errors);
    assert!(
        linked.host_modules.is_empty(),
        "htl.trait is not the host's: {:?}",
        linked.host_modules
    );
    let bundle = linked.bundle().unwrap();
    assert!(bundle.host_modules.is_empty(), "{:?}", bundle.host_modules);
    let names: Vec<&str> = bundle.modules.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"htl.trait"), "{names:?}");
}

/// Acceptance 5, the run half: that bundle on a host that installed nothing beyond
/// `Htl::new()` runs, and the copy it runs is the bundle's and not the one `Htl::new` put
/// in `package.preload`.
#[test]
fn a_bundle_runs_on_a_host_that_only_called_new_and_its_copy_is_the_one_that_runs() {
    let root = tempdir("run");
    let main = write(&root.join("src/main.tl"), &program(""));
    let linker = checker();
    linker.add_path(&root.join("src")).unwrap();
    let linked = link(&linker, &main, &LinkOptions::default()).unwrap();
    let bundle = linked.bundle().unwrap();

    let host = Htl::new().unwrap();
    let preload: Table = host
        .lua()
        .globals()
        .get::<Table>("package")
        .unwrap()
        .get("preload")
        .unwrap();
    let builtin: Function = preload.get("htl.trait").unwrap();
    host.run_bundle(bundle, &[]).unwrap();
    assert_eq!(out(&host), "area=3.14");
    let now: Function = preload.get("htl.trait").unwrap();
    assert!(now != builtin, "the host's copy still answers `htl.trait`");
}

/// The bundle's copy wins over `Htl::new`'s because it is the bundle's: a different module
/// under the same name is what `require` returns after the install.
#[test]
fn the_bundles_htl_trait_replaces_the_one_new_preloaded() {
    let bundle = Bundle {
        entry: "main".into(),
        modules: vec![
            Module {
                name: "main".into(),
                kind: Kind::Source,
                payload: b"return require('htl.trait').which\n".to_vec(),
            },
            Module {
                name: "htl.trait".into(),
                kind: Kind::Source,
                payload: b"return { which = 'bundle' }\n".to_vec(),
            },
        ],
        ..Default::default()
    };
    let h = Htl::new().unwrap();
    h.install_bundle(&bundle).unwrap();
    let which: String = h
        .lua()
        .load("return require('htl.trait').which")
        .eval()
        .unwrap();
    assert_eq!(which, "bundle");
    // Installed again, the entry is the bundle's loader and stays: the replacement is
    // idempotent like the rest of `install_bundle`.
    h.install_bundle(&bundle).unwrap();
    let again: String = h
        .lua()
        .load("package.loaded['htl.trait'] = nil; return require('htl.trait').which")
        .eval()
        .unwrap();
    assert_eq!(again, "bundle");
}

/// The bundle's copy wins even when htl's was already required in the state: the cached
/// value goes with the loader it came from.
#[test]
fn the_bundles_htl_trait_wins_after_htls_copy_was_required() {
    let bundle = Bundle {
        entry: "main".into(),
        modules: vec![
            Module {
                name: "main".into(),
                kind: Kind::Source,
                payload: b"return 0\n".to_vec(),
            },
            Module {
                name: "htl.trait".into(),
                kind: Kind::Source,
                payload: b"return { which = 'bundle' }\n".to_vec(),
            },
        ],
        ..Default::default()
    };
    let h = Htl::new().unwrap();
    let before: Value = h
        .lua()
        .load("return require('htl.trait').which")
        .eval()
        .unwrap();
    assert!(matches!(before, Value::Nil), "{before:?}");
    h.install_bundle(&bundle).unwrap();
    let which: String = h
        .lua()
        .load("return require('htl.trait').which")
        .eval()
        .unwrap();
    assert_eq!(which, "bundle");
}

/// A host's own state handed to `with_checker_lua` with its own `htl.trait` keeps it: htl
/// fills an empty name only, and records nothing for one it did not write.
#[test]
fn a_program_state_that_brought_its_own_htl_trait_keeps_it() {
    let checker = Htl::new().unwrap();
    let lua = mlua::Lua::new();
    lua.load("package.preload['htl.trait'] = function() return { which = 'host' } end")
        .exec()
        .unwrap();
    let h = Htl::with_checker_lua(&checker, lua).unwrap();
    let which: String = h
        .lua()
        .load("return require('htl.trait').which")
        .eval()
        .unwrap();
    assert_eq!(which, "host");
}

/// The exception is htl's own copy only: a host that preloaded `htl.trait` itself keeps it,
/// as it keeps any name it preloaded.
#[test]
fn a_host_that_preloaded_the_name_itself_keeps_it() {
    let bundle = Bundle {
        entry: "main".into(),
        modules: vec![
            Module {
                name: "main".into(),
                kind: Kind::Source,
                payload: b"return 0\n".to_vec(),
            },
            Module {
                name: "htl.trait".into(),
                kind: Kind::Source,
                payload: b"return { which = 'bundle' }\n".to_vec(),
            },
        ],
        ..Default::default()
    };
    let h = Htl::new().unwrap();
    h.preload("htl.trait", "return { which = 'host' }").unwrap();
    h.install_bundle(&bundle).unwrap();
    let which: String = h
        .lua()
        .load("return require('htl.trait').which")
        .eval()
        .unwrap();
    assert_eq!(which, "host");
}

/// `replace_bundle` takes a bundle's `htl.trait` back to htl's, not to nothing, when the
/// next bundle does not carry it.
#[test]
fn replacing_a_bundle_that_carried_it_restores_the_one_new_made() {
    let with = Bundle {
        entry: "main".into(),
        modules: vec![
            Module {
                name: "main".into(),
                kind: Kind::Source,
                payload: b"return 0\n".to_vec(),
            },
            Module {
                name: "htl.trait".into(),
                kind: Kind::Source,
                payload: b"return { which = 'bundle' }\n".to_vec(),
            },
        ],
        ..Default::default()
    };
    let without = Bundle {
        entry: "main".into(),
        modules: vec![Module {
            name: "main".into(),
            kind: Kind::Source,
            payload: b"return 0\n".to_vec(),
        }],
        ..Default::default()
    };
    let h = Htl::new().unwrap();
    h.install_bundle(&with).unwrap();
    h.replace_bundle(&without, &[]).unwrap();
    let kind: Value = h
        .lua()
        .load("return require('htl.trait').impl")
        .eval()
        .unwrap();
    assert!(matches!(kind, Value::Function(_)), "{kind:?}");
}

//! `htl.toml`: parsing / discovery, the static `contract` lint, the `contract-unenforced`
//! host scan, and `contract_resolvers` giving the host the same contract at run time.

use htl_core::config::{HtlConfig, join_specs};
use htl_core::{Htl, contract_enforcement_lints, contract_lints};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-config-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const CONTRACT_TOML: &str =
    "[[contract]]\ndir = \"mods\"\ntype = \"defs.Mod\"\nrequire_fields = true\n";

/// Project with `htl.toml`, `src/defs.tl` and three mods: conforming, wrong field
/// type, and one that leaves a declared field out.
fn project(name: &str) -> (PathBuf, HtlConfig) {
    let root = scratch(name);
    write(&root.join("htl.toml"), CONTRACT_TOML);
    write(
        &root.join("src").join("defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n   end\nend\nreturn defs\n",
    );
    write(
        &root.join("mods").join("good.tl"),
        "return { name = \"swarm\", hp = 3 }\n",
    );
    write(
        &root.join("mods").join("bad.tl"),
        "return { name = \"broken\", hp = \"lots\" }\n",
    );
    write(
        &root.join("mods").join("partial.tl"),
        "return { name = \"half\" }\n",
    );
    let (_, cfg) = HtlConfig::find(&root)
        .unwrap()
        .expect("htl.toml just written");
    (root, cfg)
}

#[test]
fn parse_sections_and_lint_spec() {
    let cfg = HtlConfig::parse(
        "[lint]\nenable = [\"class-record\", \"explicit-number\"]\ndisable = [\"shadow-local\"]\nstrict = true\n\
         [fmt]\nindent = 2\n[[contract]]\ndir = \"mods\"\ntype = \"defs.Mod\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.lint_spec(),
        "+class-record,+explicit-number,-shadow-local"
    );
    assert_eq!(cfg.lint.strict, Some(true));
    assert_eq!(cfg.fmt.indent, Some(2));
    assert_eq!(cfg.contract.len(), 1);
    assert_eq!(cfg.contract[0].type_path, "defs.Mod");
    assert!(!cfg.contract[0].require_fields, "defaults to off");
    assert_eq!(
        join_specs([cfg.lint_spec().as_str(), "", "+shadow-local"]),
        "+class-record,+explicit-number,-shadow-local,+shadow-local"
    );
    assert!(
        HtlConfig::parse("[lint]\nstrictness = true\n").is_err(),
        "unknown keys are errors"
    );
}

#[test]
fn find_walks_up_from_a_file() {
    let root = scratch("find");
    write(&root.join("htl.toml"), "[fmt]\nindent = 4\n");
    write(&root.join("src").join("deep").join("x.tl"), "return 1\n");
    let (path, cfg) = HtlConfig::find(&root.join("src/deep/x.tl"))
        .unwrap()
        .unwrap();
    assert!(path.ends_with("htl.toml"));
    assert_eq!(cfg.fmt.indent, Some(4));
    let none = HtlConfig::find(&scratch("find-none")).unwrap();
    assert!(none.is_none());
}

#[test]
fn contract_lint_flags_wrong_type_and_missing_field_only() {
    let (root, cfg) = project("lint");
    let h = Htl::new().unwrap();

    let good = contract_lints(&h, &root, &cfg, &root.join("mods/good.tl")).unwrap();
    assert!(good.is_empty(), "{good:?}");

    let bad = contract_lints(&h, &root, &cfg, &root.join("mods/bad.tl")).unwrap();
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(
        bad[0].contains("does not satisfy contract defs.Mod"),
        "{}",
        bad[0]
    );
    assert!(bad[0].contains("[htl contract]"), "{}", bad[0]);

    let partial = contract_lints(&h, &root, &cfg, &root.join("mods/partial.tl")).unwrap();
    assert_eq!(partial.len(), 1, "{partial:?}");
    assert!(
        partial[0].contains("lacks declared field(s) of defs.Mod: hp"),
        "{}",
        partial[0]
    );
    assert!(
        partial[0].contains("mods/partial.tl:1:"),
        "points at the return: {}",
        partial[0]
    );

    // Outside the contract dir: nothing to say.
    let defs = contract_lints(&h, &root, &cfg, &root.join("src/defs.tl")).unwrap();
    assert!(defs.is_empty(), "{defs:?}");
}

#[test]
fn contract_lint_without_require_fields_accepts_partial() {
    let (root, mut cfg) = project("lenient");
    cfg.contract[0].require_fields = false;
    let h = Htl::new().unwrap();
    let partial = contract_lints(&h, &root, &cfg, &root.join("mods/partial.tl")).unwrap();
    assert!(
        partial.is_empty(),
        "every record field is nilable: {partial:?}"
    );
}

#[test]
fn unenforced_contract_is_reported_against_host_sources() {
    let (root, cfg) = project("host");
    let cfg_path = root.join("htl.toml");
    let host = |body: &str| {
        write(&root.join("src").join("main.rs"), body);
        contract_enforcement_lints(&cfg, &cfg_path, Some(&root))
    };

    let none = host("fn main() {}\n");
    assert_eq!(none.len(), 1, "{none:?}");
    assert!(none[0].contains("contract-unenforced"), "{}", none[0]);
    assert!(
        none[0].contains("TealResolver::new(\"mods\").expect_type(\"defs.Mod\").require_fields()"),
        "tells the host what to write: {}",
        none[0]
    );

    let no_fields = host("let r = TealResolver::new(\"mods\")?.expect_type(\"defs.Mod\");\n");
    assert_eq!(no_fields.len(), 1, "{no_fields:?}");
    assert!(
        no_fields[0].contains("never calls .require_fields()"),
        "{}",
        no_fields[0]
    );

    let by_hand =
        host("let r = TealResolver::new(\"mods\")?.expect_type(\"defs.Mod\").require_fields();\n");
    assert!(by_hand.is_empty(), "{by_hand:?}");

    let by_config = host("for r in htl::pkg::contract_resolvers(&root, &cfg)? { reg.add(r); }\n");
    assert!(by_config.is_empty(), "{by_config:?}");
    // A host that picks one contract dir at a time (a site chosen on the command line).
    let one_dir =
        host("reg.add(TealResolver::for_contract_dir(&root, &site_dir, &cfg.contract[0])?);\n");
    assert!(one_dir.is_empty(), "{one_dir:?}");

    // No host crate at all: a script-only project has nothing to enforce.
    assert!(contract_enforcement_lints(&cfg, &cfg_path, None).is_empty());
}

#[test]
fn contract_lint_reads_annotated_cast_and_field_assignment_forms() {
    let (root, cfg) = project("forms");
    // The forms htl's own hint recommends must not switch the static check off.
    let d = "local defs = require(\"defs\")\n";
    write(
        &root.join("mods/annot.tl"),
        &format!("{d}local m: defs.Mod = {{ name = \"a\" }}\nreturn m\n"),
    );
    write(
        &root.join("mods/cast.tl"),
        &format!("{d}return {{ name = \"c\" }} as defs.Mod\n"),
    );
    write(
        &root.join("mods/late.tl"),
        &format!("{d}local m: defs.Mod = {{ name = \"l\" }}\nm.hp = 2\nreturn m\n"),
    );
    write(
        &root.join("mods/reassign.tl"),
        &format!("{d}local m: defs.Mod\nm = {{ name = \"r\" }}\nreturn m\n"),
    );
    let h = Htl::new().unwrap();
    for f in ["annot", "cast", "reassign"] {
        let l = contract_lints(&h, &root, &cfg, &root.join(format!("mods/{f}.tl"))).unwrap();
        assert_eq!(l.len(), 1, "{f}: {l:?}");
        assert!(
            l[0].contains("lacks declared field(s) of defs.Mod: hp"),
            "{f}: {}",
            l[0]
        );
    }
    let late = contract_lints(&h, &root, &cfg, &root.join("mods/late.tl")).unwrap();
    assert!(late.is_empty(), "`m.hp = 2` counts as present: {late:?}");
}

#[test]
fn contract_exclude_glob_dir_and_module_filter() {
    let root = scratch("glob");
    write(
        &root.join("htl.toml"),
        "[[contract]]\ndir = \"mods\"\ntype = \"defs.Mod\"\nrequire_fields = true\nexclude = [\"modkit\"]\n\n\
         [[contract]]\ndir = \"sites/*\"\ntype = \"defs.Site\"\nmodule = \"Site\"\n",
    );
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n   end\n   record Site\n      title: string\n   end\nend\nreturn defs\n",
    );
    // An SDK the host drops into the contract dir: not a Mod, and must not be held to it.
    write(
        &root.join("mods/modkit.tl"),
        "local record modkit\nend\nfunction modkit.define(t: table): table\n   return t\nend\nreturn modkit\n",
    );
    write(
        &root.join("mods/good.tl"),
        "return { name = \"g\", hp = 1 }\n",
    );
    write(&root.join("sites/blog/Site.tl"), "return { title = 1 }\n");
    write(
        &root.join("sites/blog/helper.tl"),
        "return { anything = true }\n",
    );
    write(
        &root.join("sites/docs/Site.tl"),
        "return { title = \"docs\" }\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();

    let dirs = cfg.contract[1].dirs(&root);
    assert_eq!(dirs, vec![root.join("sites/blog"), root.join("sites/docs")]);
    assert!(!cfg.contract[0].applies_to("modkit") && !cfg.contract[0].applies_to("defs"));
    assert!(cfg.contract[1].applies_to("Site") && !cfg.contract[1].applies_to("helper"));

    let h = Htl::new().unwrap();
    let lint = |rel: &str| contract_lints(&h, &root, &cfg, &root.join(rel)).unwrap();
    assert!(
        lint("mods/modkit.tl").is_empty(),
        "excluded SDK held to contract"
    );
    assert!(lint("mods/good.tl").is_empty());
    let blog = lint("sites/blog/Site.tl");
    assert_eq!(blog.len(), 1, "{blog:?}");
    assert!(
        blog[0].contains("does not satisfy contract defs.Site (sites/*)"),
        "{}",
        blog[0]
    );
    assert!(
        lint("sites/blog/helper.tl").is_empty(),
        "module filter must skip helper"
    );
    assert!(lint("sites/docs/Site.tl").is_empty());

    // Run time: one resolver per matched dir, same exclude / module rules.
    let mut reg = htl_core::pkg::mlua_pkg::Registry::new();
    for r in htl_core::pkg::contract_resolvers(&root, &cfg).unwrap() {
        reg.add(r);
    }
    reg.install(h.lua()).unwrap();
    let kit: mlua::Table = h.lua().load("return require('modkit')").eval().unwrap();
    assert!(kit.contains_key("define").unwrap(), "SDK served untouched");
    // Two dirs serve `Site`; the first registered (blog) is rejected, and a rejected
    // module must fail the require rather than silently fall through to docs.
    let err = h
        .lua()
        .load("return require('Site').title")
        .eval::<String>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not satisfy defs.Site"), "{err}");
    let helper: mlua::Table = h.lua().load("return require('helper')").eval().unwrap();
    assert!(
        helper.contains_key("anything").unwrap(),
        "module filter: helper served untyped"
    );
}

#[test]
fn check_paths_make_host_supplied_modules_visible() {
    let root = scratch("paths");
    write(&root.join("htl.toml"), "[check]\npaths = [\"sdk\"]\n");
    write(
        &root.join("sdk/Tasks.tl"),
        "local record Tasks\n   run: function(string)\nend\nreturn Tasks\n",
    );
    write(
        &root.join("src/use.tl"),
        "local Tasks = require(\"Tasks\")\nTasks.run(1)\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    assert_eq!(
        cfg.search_paths(&root),
        vec![root.clone(), root.join("src"), root.join("sdk")]
    );

    let bare = Htl::new().unwrap();
    bare.add_path(&root.join("src")).unwrap();
    let ci = bare.check(&root.join("src/use.tl")).unwrap();
    assert!(
        ci.errors.iter().any(|e| e.contains("module not found")),
        "{:?}",
        ci.errors
    );

    let h = Htl::new().unwrap();
    h.apply_config(&root, &cfg).unwrap();
    let ci = h.check(&root.join("src/use.tl")).unwrap();
    assert!(
        ci.errors
            .iter()
            .any(|e| e.contains("got integer, expected string")),
        "typed through [check] paths: {:?}",
        ci.errors
    );
}

/// `types/` next to htl.toml is searched without configuration, and a source of the
/// same module name elsewhere on the path still wins over the declaration there.
#[test]
fn types_dir_is_searched_by_default() {
    let root = scratch("types");
    write(&root.join("htl.toml"), "[lint]\n");
    write(
        &root.join("types/xlib.d.tl"),
        "local record xlib\n   connect: function(string): boolean\nend\nreturn xlib\n",
    );
    write(
        &root.join("src/use.tl"),
        "local xlib = require(\"xlib\")\nlocal ok: string = xlib.connect(\"h\")\nprint(ok)\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    assert_eq!(
        cfg.search_paths(&root),
        vec![root.clone(), root.join("src"), root.join("types")]
    );

    let h = Htl::new().unwrap();
    h.apply_config(&root, &cfg).unwrap();
    let ci = h.check(&root.join("src/use.tl")).unwrap();
    assert!(
        ci.errors
            .iter()
            .any(|e| e.contains("got boolean, expected string")),
        "typed through types/: {:?}",
        ci.errors
    );

    // A source with the same name in src/ wins over the declaration in types/.
    write(
        &root.join("src/xlib.tl"),
        "local record xlib\nend\nfunction xlib.connect(_: string): string\n   return \"s\"\nend\nreturn xlib\n",
    );
    let h = Htl::new().unwrap();
    h.apply_config(&root, &cfg).unwrap();
    let ci = h.check(&root.join("src/use.tl")).unwrap();
    assert!(ci.ok(), "source beats the declaration: {:?}", ci.errors);
}

#[test]
fn source_beats_a_stale_declaration_wherever_it_sits_on_the_path() {
    let root = scratch("stale-decl");
    write(&root.join("htl.toml"), "[check]\npaths = [\"mods\"]\n");
    // The source gained `items`; the declaration the host wrote last run has not.
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      items: {string}\n   end\nend\nreturn defs\n",
    );
    write(
        &root.join("mods/defs.d.tl"),
        "local record defs\n   record Mod\n      name: string\n   end\nend\nreturn defs\n",
    );
    write(
        &root.join("mods/x.tl"),
        "local defs = require(\"defs\")\nlocal m: defs.Mod = { name = \"x\", items = {} }\nreturn m\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();

    // Both dirs on the path, in either order: the checker must read src/defs.tl.
    for order in [["src", "mods"], ["mods", "src"]] {
        let h = Htl::new().unwrap();
        for d in order {
            h.add_path(&root.join(d)).unwrap();
        }
        let ci = h.check(&root.join("mods/x.tl")).unwrap();
        assert!(ci.ok(), "path order {order:?}: {:?}", ci.errors);
    }
    let h = Htl::new().unwrap();
    h.apply_config(&root, &cfg).unwrap();
    assert!(h.check(&root.join("mods/x.tl")).unwrap().ok());

    // Only the declaration reachable (an external mod author's checkout): it is used.
    let h = Htl::new().unwrap();
    h.add_path(&root.join("mods")).unwrap();
    let ci = h.check(&root.join("mods/x.tl")).unwrap();
    assert!(
        ci.errors.iter().any(|e| e.contains("items")),
        "stale decl in use: {:?}",
        ci.errors
    );
}

#[test]
fn declaration_steps_aside_for_a_preloaded_host_module() {
    let root = scratch("preload");
    write(
        &root.join("mods/host.d.tl"),
        "local record host\n   twice: function(integer): integer\nend\nreturn host\n",
    );
    write(
        &root.join("mods/use.tl"),
        "local host = require(\"host\")\nreturn host.twice(2)\n",
    );

    let h = Htl::new().unwrap();
    let mut reg = htl_core::pkg::mlua_pkg::Registry::new();
    reg.add(htl_core::pkg::TealResolver::new(root.join("mods")).unwrap());
    reg.install(h.lua()).unwrap();

    // Without an implementation the declaration answers, and says so on first use.
    let err = h
        .lua()
        .load("return require('use')")
        .eval::<i64>()
        .unwrap_err()
        .to_string();
    assert!(err.contains("declaration-only"), "{err}");

    // With the host's implementation in package.preload the declaration steps aside.
    let h = Htl::new().unwrap();
    let t = h.lua().create_table().unwrap();
    t.set(
        "twice",
        h.lua().create_function(|_, n: i64| Ok(n * 2)).unwrap(),
    )
    .unwrap();
    h.preload_value("host", t).unwrap();
    let mut reg = htl_core::pkg::mlua_pkg::Registry::new();
    reg.add(htl_core::pkg::TealResolver::new(root.join("mods")).unwrap());
    reg.install(h.lua()).unwrap();
    let four: i64 = h.lua().load("return require('use')").eval().unwrap();
    assert_eq!(four, 4);
}

#[test]
fn contract_resolvers_enforce_the_same_contract_at_run_time() {
    let (root, cfg) = project("runtime");
    let h = Htl::new().unwrap();
    let mut reg = htl_core::pkg::mlua_pkg::Registry::new();
    for r in htl_core::pkg::contract_resolvers(&root, &cfg).unwrap() {
        reg.add(r);
    }
    reg.install(h.lua()).unwrap();

    let hp: i64 = h.lua().load("return require('good').hp").eval().unwrap();
    assert_eq!(hp, 3);
    let bad = h
        .lua()
        .load("return require('bad')")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(bad.contains("does not satisfy defs.Mod"), "{bad}");
    let partial = h
        .lua()
        .load("return require('partial')")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(partial.contains("hp"), "missing field named: {partial}");
}

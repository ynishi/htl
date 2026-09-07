//! `htl.toml`: parsing / discovery, the static `contract` lint, the `contract-unenforced`
//! host scan, and `contract_resolvers` giving the host the same contract at run time.

use htl_core::config::{HtlConfig, join_specs};
use htl_core::pkg::TealResolver as Resolver;
use htl_core::{Htl, contract_enforcement_lints, contract_lints};
use std::path::{Path, PathBuf};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-core-config", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const CONTRACT_TOML: &str = "[[contract]]\ndir = \"mods\"\n";

/// `defs.Mod` with both fields required, written the way a project declares a contract:
/// the marker on the record, `---@required` on what a module must set.
fn defs_src(marks: [&str; 2]) -> String {
    format!(
        "local record defs\n   record Mod   ---@contract\n      name: string   {}\n      \
         hp: integer   {}\n   end\nend\nreturn defs\n",
        marks[0], marks[1]
    )
}

const REQUIRED: &str = "---@required";

/// Project with `htl.toml`, `src/defs.tl` and three mods: conforming, wrong field
/// type, and one that leaves a required field out.
fn project(name: &str) -> (PathBuf, HtlConfig) {
    project_declaring_at(name, "src/defs.tl", CONTRACT_TOML)
}

/// The same project, with the contract type wherever the caller puts it: `decl` is the
/// path to write it to (relative to the root), and `toml` the whole `htl.toml`, so a
/// caller can point `[check] paths` at wherever it put the declaration.
fn project_declaring_at(name: &str, decl: &str, toml: &str) -> (PathBuf, HtlConfig) {
    let root = scratch(name);
    write(&root.join("htl.toml"), toml);
    write(&root.join(decl), &defs_src([REQUIRED, REQUIRED]));
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

/// The contracts a project declares, with anything wrong about them raised rather than
/// returned: a fixture that cannot be resolved is a broken fixture.
fn contracts(root: &Path, cfg: &HtlConfig) -> Vec<htl_core::contract::Resolved> {
    let (found, problems) = htl_core::contract::resolve(root, cfg);
    assert!(problems.is_empty(), "{problems:?}");
    found
}

/// `contract_lints` for one file of a project, resolving the markers first.
fn lints_for(h: &Htl, root: &Path, cfg: &HtlConfig, rel: &str) -> Vec<String> {
    contract_lints(h, root, cfg, &contracts(root, cfg), &root.join(rel)).unwrap()
}

/// Load each mod through resolvers the caller built, and say what happened: `Ok` for a
/// mod that loaded, `Err(message)` for one the contract (or the checker) refused.
fn verdicts(h: &Htl, resolvers: Vec<htl_core::pkg::TealResolver>) -> [Result<(), String>; 2] {
    let mut reg = htl_core::pkg::mlua_pkg::Registry::new();
    for r in resolvers {
        reg.add(r);
    }
    reg.install(h.lua()).unwrap();
    ["good", "partial"].map(|m| {
        h.lua()
            .load(format!("return require('{m}')"))
            .eval::<mlua::Value>()
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

#[test]
fn parse_sections_and_lint_spec() {
    let cfg = HtlConfig::parse(
        "[lint]\nenable = [\"class-record\", \"explicit-number\"]\ndisable = [\"shadow-local\"]\nstrict = true\n\
         [fmt]\nindent = 2\n[[contract]]\ndir = \"mods\"\n",
    )
    .unwrap();
    assert_eq!(
        cfg.lint_spec(),
        "+class-record,+explicit-number,-shadow-local"
    );
    assert_eq!(cfg.lint.strict, Some(true));
    assert_eq!(cfg.fmt.indent, Some(2));
    assert_eq!(cfg.contract.len(), 1);
    assert_eq!(cfg.contract[0].dir, "mods");
    assert_eq!(cfg.contract[0].module, None);
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

    let good = lints_for(&h, &root, &cfg, "mods/good.tl");
    assert!(good.is_empty(), "{good:?}");

    let bad = lints_for(&h, &root, &cfg, "mods/bad.tl");
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(
        bad[0].contains("does not satisfy contract defs.Mod"),
        "{}",
        bad[0]
    );
    assert!(bad[0].contains("[htl contract]"), "{}", bad[0]);

    let partial = lints_for(&h, &root, &cfg, "mods/partial.tl");
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
    let defs = lints_for(&h, &root, &cfg, "src/defs.tl");
    assert!(defs.is_empty(), "{defs:?}");
}

/// An unmarked field is optional. Nothing is required until something says so, which is
/// what lets a contract type grow without breaking the modules that predate a new field.
#[test]
fn an_unmarked_field_is_not_required() {
    let (root, cfg) = project("lenient");
    write(&root.join("src/defs.tl"), &defs_src(["", ""]));
    let h = Htl::new().unwrap();
    let partial = lints_for(&h, &root, &cfg, "mods/partial.tl");
    assert!(
        partial.is_empty(),
        "every record field is nilable: {partial:?}"
    );
}

/// `---@required` on one field of two: that one is mandatory, the other is not.
#[test]
fn required_holds_the_marked_fields_only() {
    let (root, cfg) = project("named");
    write(&root.join("src/defs.tl"), &defs_src([REQUIRED, ""]));
    let h = Htl::new().unwrap();
    let partial = lints_for(&h, &root, &cfg, "mods/partial.tl");
    assert!(
        partial.is_empty(),
        "hp is declared but not required: {partial:?}"
    );

    write(&root.join("src/defs.tl"), &defs_src([REQUIRED, REQUIRED]));
    let both = lints_for(&h, &root, &cfg, "mods/partial.tl");
    assert_eq!(both.len(), 1, "{both:?}");
    assert!(
        both[0].contains("returned table lacks declared field(s) of defs.Mod: hp"),
        "the message is the one it always was: {}",
        both[0]
    );
}

/// The marker may sit on the line above the field as well as after it, which is how
/// `---@struct` / `---@optional` are written too.
#[test]
fn required_is_read_from_the_line_above_as_well() {
    let (root, cfg) = project("above");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string\n      \
         ---@required\n      hp: integer\n   end\nend\nreturn defs\n",
    );
    let h = Htl::new().unwrap();
    let partial = lints_for(&h, &root, &cfg, "mods/partial.tl");
    assert_eq!(partial.len(), 1, "{partial:?}");
    assert!(
        partial[0].contains("lacks declared field(s) of defs.Mod: hp"),
        "{}",
        partial[0]
    );
}

/// The static form and the runtime form are a pair, and one set of markers has to keep
/// them one: the same fixture, the same marks, the same verdict from `htl check` and
/// from `require`.
#[test]
fn the_static_and_runtime_forms_agree_on_the_same_fixture() {
    use htl_core::pkg::{TealResolver, mlua_pkg::Registry};

    let (root, cfg) = project("agree");
    let verdicts = || {
        let h = Htl::new().unwrap();
        let statically = !lints_for(&h, &root, &cfg, "mods/partial.tl").is_empty();
        let h2 = Htl::new().unwrap();
        let mut reg = Registry::new();
        for r in TealResolver::for_contract(&root, &cfg, &contracts(&root, &cfg)[0]).unwrap() {
            reg.add(r);
        }
        reg.install(h2.lua()).unwrap();
        let at_runtime = h2
            .lua()
            .load("return require('partial').name")
            .eval::<String>()
            .is_err();
        (statically, at_runtime)
    };

    write(&root.join("src/defs.tl"), &defs_src([REQUIRED, ""]));
    assert_eq!(verdicts(), (false, false), "hp is not marked");

    write(&root.join("src/defs.tl"), &defs_src([REQUIRED, REQUIRED]));
    assert_eq!(verdicts(), (true, true), "hp is marked");
}

/// The keys that moved onto the record are not quietly ignored: the message says where
/// they went, rather than serde's "unknown field".
#[test]
fn a_config_that_still_declares_the_type_says_where_it_moved() {
    for key in ["type = \"defs.Mod\"", "require_fields = [\"name\"]"] {
        let e = HtlConfig::parse(&format!("[[contract]]\ndir = \"mods\"\n{key}\n"))
            .expect_err("the key is gone")
            .to_string();
        let chain = format!(
            "{e}: {}",
            HtlConfig::parse(&format!("[[contract]]\ndir = \"mods\"\n{key}\n"))
                .unwrap_err()
                .root_cause()
        );
        assert!(chain.contains("---@contract"), "{key}: {chain}");
    }
}

#[test]
fn unenforced_contract_is_reported_against_host_sources() {
    let (root, cfg) = project("host");
    let cfg_path = root.join("htl.toml");
    let found = contracts(&root, &cfg);
    let host = |body: &str| {
        write(&root.join("src").join("main.rs"), body);
        contract_enforcement_lints(&cfg_path, &found, Some(&root))
    };

    let none = host("fn main() {}\n");
    assert_eq!(none.len(), 1, "{none:?}");
    assert!(none[0].contains("contract-unenforced"), "{}", none[0]);
    assert!(
        none[0].contains("htl::pkg::contract_resolvers(root, &config)"),
        "tells the host what to write: {}",
        none[0]
    );
    assert!(
        none[0].contains("src/defs.tl:2:"),
        "points at the marker, not at htl.toml: {}",
        none[0]
    );

    // One call to look for. A resolver assembled by hand still works, but it restates
    // what the record says, so it is not what the lint is satisfied by.
    let by_hand = host(
        "let r = TealResolver::new(\"mods\")?.expect_type(\"defs.Mod\").require_all_fields();\n",
    );
    assert_eq!(by_hand.len(), 1, "not the call this looks for: {by_hand:?}");

    let by_config = host("for r in htl::pkg::contract_resolvers(&root, &cfg)? { reg.add(r); }\n");
    assert!(by_config.is_empty(), "{by_config:?}");

    // No host crate at all: a script-only project has nothing to enforce.
    assert!(contract_enforcement_lints(&cfg_path, &found, None).is_empty());
}

/// `enforced_by` names where the enforcement lives when it is somewhere the scan cannot
/// reach — a Lua-side validator, a sibling crate, generated code, a resolver built by
/// hand. The contract that carries it is not held to the scan; the others still are.
#[test]
fn enforced_by_exempts_the_contract_that_carries_it() {
    let (root, _) = project("enforced");
    let cfg_path = root.join("htl.toml");
    write(&root.join("src/main.rs"), "fn main() {}\n");
    write(&root.join("mods/_validate.lua"), "return function() end\n");
    write(
        &cfg_path,
        "[[contract]]\ndir = \"mods\"\nenforced_by = \"mods/_validate.lua\"\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    let found = contracts(&root, &cfg);
    assert_eq!(
        found[0].enforced_by.as_deref(),
        Some("mods/_validate.lua"),
        "carried onto the resolved contract"
    );
    let out = contract_enforcement_lints(&cfg_path, &found, Some(&root));
    assert!(out.is_empty(), "no call in the Rust sources, and none needed: {out:?}");
}

/// The path is what makes the key a claim rather than an off switch: it has to exist, and
/// a name that points at nothing is reported under the same rule — including when the
/// scan did find the call, because the statement is broken either way.
#[test]
fn enforced_by_naming_nothing_is_reported() {
    let (root, _) = project("enforced-missing");
    let cfg_path = root.join("htl.toml");
    write(
        &cfg_path,
        "[[contract]]\ndir = \"mods\"\nenforced_by = \"mods/_gone.lua\"\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    let found = contracts(&root, &cfg);

    for body in [
        "fn main() {}\n",
        "for r in htl::pkg::contract_resolvers(&root, &cfg)? { reg.add(r); }\n",
    ] {
        write(&root.join("src/main.rs"), body);
        let out = contract_enforcement_lints(&cfg_path, &found, Some(&root));
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            out[0].contains("no such file") && out[0].contains("_gone.lua"),
            "says the named file is missing, not that the host does not enforce it: {}",
            out[0]
        );
        assert!(
            out[0].contains("htl.toml:1:1"),
            "points at the config, where the key is: {}",
            out[0]
        );
    }
}

/// An absolute path is taken as it is, the way `[check] paths` takes one — enforcement
/// may sit outside the project (a sibling crate reached by path, a shared validator).
#[test]
fn enforced_by_takes_an_absolute_path() {
    let (root, _) = project("enforced-abs");
    let elsewhere = scratch("enforced-abs-target").join("validate.lua");
    write(&elsewhere, "return function() end\n");
    let cfg_path = root.join("htl.toml");
    write(
        &cfg_path,
        &format!(
            "[[contract]]\ndir = \"mods\"\nenforced_by = {:?}\n",
            elsewhere.to_string_lossy()
        ),
    );
    write(&root.join("src/main.rs"), "fn main() {}\n");
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    let out = contract_enforcement_lints(&cfg_path, &contracts(&root, &cfg), Some(&root));
    assert!(out.is_empty(), "{out:?}");
}

/// One contract's `enforced_by` does not answer for another.
#[test]
fn enforced_by_is_per_contract() {
    let root = scratch("enforced-two");
    write(
        &root.join("htl.toml"),
        "[[contract]]\ndir = \"mods\"\nenforced_by = \"validate.lua\"\n\n\
         [[contract]]\ndir = \"plugins\"\n",
    );
    write(&root.join("validate.lua"), "return function() end\n");
    write(&root.join("src/main.rs"), "fn main() {}\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract(\"mods\")\n      name: string   ---@required\n   \
         end\n   record Plug   ---@contract(\"plugins\")\n      id: string   ---@required\n   end\nend\nreturn defs\n",
    );
    write(&root.join("mods/a.tl"), "return { name = \"a\" }\n");
    write(&root.join("plugins/b.tl"), "return { id = \"b\" }\n");
    let (cfg_path, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    let found = contracts(&root, &cfg);

    let out = contract_enforcement_lints(&cfg_path, &found, Some(&root));
    assert_eq!(out.len(), 1, "only the one without the key: {out:?}");
    assert!(out[0].contains("defs.Plug"), "{}", out[0]);
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
        let l = lints_for(&h, &root, &cfg, &format!("mods/{f}.tl"));
        assert_eq!(l.len(), 1, "{f}: {l:?}");
        assert!(
            l[0].contains("lacks declared field(s) of defs.Mod: hp"),
            "{f}: {}",
            l[0]
        );
    }
    let late = lints_for(&h, &root, &cfg, "mods/late.tl");
    assert!(late.is_empty(), "`m.hp = 2` counts as present: {late:?}");
}

/// Two contracts in one project, so both markers name their own directory: a glob dir
/// with a module filter, and a plain one. The `[[contract]]` block is then only there
/// for a project that has a single contract to inherit.
#[test]
fn contract_glob_dir_and_module_filter() {
    let root = scratch("glob");
    write(&root.join("htl.toml"), "[lint]\n");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract(\"mods\", exclude = \"modkit\")\n      \
         name: string   ---@required\n      hp: integer   ---@required\n   end\n   \
         record Site   ---@contract(\"sites/*\", module = \"Site\")\n      \
         title: string\n   end\nend\nreturn defs\n",
    );
    // An SDK the host drops into the contract dir as a source: it is a `.tl` beside the
    // modules, so without `exclude` it would be held to the contract like one.
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
    let found = contracts(&root, &cfg);
    assert_eq!(found.len(), 2, "{found:?}");
    let site = found.iter().find(|c| c.dir == "sites/*").unwrap();

    assert_eq!(
        site.dirs(&root),
        vec![root.join("sites/blog"), root.join("sites/docs")]
    );
    let mods = found.iter().find(|c| c.dir == "mods").unwrap();
    assert!(!mods.applies_to("defs"), "the declaring module is exempt");
    assert!(!mods.applies_to("modkit"), "excluded on the marker");
    assert!(site.applies_to("Site") && !site.applies_to("helper"));

    let h = Htl::new().unwrap();
    let lint = |rel: &str| contract_lints(&h, &root, &cfg, &found, &root.join(rel)).unwrap();
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

/// `search_paths` reads as a search order, and `apply_config` has to consult the
/// directories in that order rather than in the reverse of it: `add_path` prepends, so
/// adding the list front to back leaves the last entry first. The case it decides is two
/// declarations of one module — a `.tl` beats a `.d.tl` wherever the two sit, so the
/// order is invisible until neither is a source.
#[test]
fn the_search_order_is_the_one_search_paths_states() {
    let root = scratch("search-order");
    write(&root.join("htl.toml"), "[check]\npaths = [\"sdk\"]\n");
    // The same module declared three times, each saying something different about the
    // return type of `connect`, so the error names the one that was read.
    for (dir, ret) in [("src", "string"), ("types", "boolean"), ("sdk", "integer")] {
        write(
            &root.join(dir).join("xlib.d.tl"),
            &format!("local record xlib\n   connect: function(string): {ret}\nend\nreturn xlib\n"),
        );
    }
    write(
        &root.join("src/use.tl"),
        "local xlib = require(\"xlib\")\nlocal n: nil = xlib.connect(\"h\")\nprint(n)\n",
    );
    let (_, cfg) = HtlConfig::find(&root).unwrap().unwrap();
    assert_eq!(
        cfg.search_paths(&root),
        vec![
            root.clone(),
            root.join("src"),
            root.join("types"),
            root.join("sdk")
        ],
        "the list itself is in search order"
    );

    let h = Htl::new().unwrap();
    h.apply_config(&root, &cfg).unwrap();
    let errors = h.check(&root.join("src/use.tl")).unwrap().errors;
    assert!(
        errors.iter().any(|e| e.contains("got string")),
        "src/ is consulted before types/ and the [check] path: {errors:?}"
    );
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

/// A contract type declared in `types/` — where `htl new` puts hand-written declarations
/// and where a host publishes the one its mod authors write against. The `contract` lint
/// resolves it because `contract_lints` goes through `apply_config`; the resolver has to
/// ask for the same paths rather than assume `root` and `root/src`, or a mod fails at
/// `require` with the checker's "module not found" and nothing about the contract.
#[test]
fn a_contract_type_under_types_resolves_at_run_time() {
    let (root, cfg) = project_declaring_at("types-decl", "types/defs.d.tl", CONTRACT_TOML);
    let h = Htl::new().unwrap();
    let [good, partial] =
        verdicts(&h, Resolver::for_contract(&root, &cfg, &contracts(&root, &cfg)[0]).unwrap());
    assert!(good.is_ok(), "conforming mod: {good:?}");
    assert!(
        partial.as_ref().is_err_and(|e| e.contains("hp")),
        "missing field named: {partial:?}"
    );
}

/// The same for a declaration reachable only through `[check] paths` — an SDK cache, or
/// any directory the host supplies at run time from outside the scaffold layout.
#[test]
fn a_contract_type_under_a_check_path_resolves_at_run_time() {
    let (root, cfg) = project_declaring_at(
        "check-path-decl",
        "sdk/defs.tl",
        "[check]\npaths = [\"sdk\"]\n[[contract]]\ndir = \"mods\"\n",
    );
    let h = Htl::new().unwrap();
    let [good, partial] =
        verdicts(&h, Resolver::for_contract(&root, &cfg, &contracts(&root, &cfg)[0]).unwrap());
    assert!(good.is_ok(), "conforming mod: {good:?}");
    assert!(
        partial.as_ref().is_err_and(|e| e.contains("hp")),
        "missing field named: {partial:?}"
    );
}

/// The module a contract type is declared in is what an outside author writes against,
/// so it is published: `types/<module>.d.tl` by default, since `types/` is where a
/// project keeps declarations for other people and is searched with no configuration.
#[test]
fn a_contract_type_is_published_under_types() {
    let (root, cfg) = project("publish");
    let found = contracts(&root, &cfg);
    let target = root.join("types/defs.d.tl");
    assert_eq!(
        htl_core::contract::dts_target(&root, &found[0]),
        Some(target.clone())
    );

    let (written, problems) = htl_core::contract::publish(&root, &found);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(written, vec![(target.clone(), true)]);
    // The declaring module as it is, but for the marker: a bare `---@contract` inherits
    // its directory from an htl.toml the reader of the declaration does not have.
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        std::fs::read_to_string(root.join("src/defs.tl"))
            .unwrap()
            .replace("---@contract", "---@contract(\"mods\")"),
        "the declaring module, with the directory written out"
    );

    // Idempotent: a second run has nothing to write.
    let (again, _) = htl_core::contract::publish(&root, &found);
    assert_eq!(again, vec![(target, false)]);

    // And the published copy carries the marker, so it is found again by the scan. It is
    // the same contract, not a second one claiming the same directory.
    let found_again = contracts(&root, &cfg);
    assert_eq!(found_again.len(), 1, "{found_again:?}");
    assert!(
        found_again[0].declared_in.ends_with("src/defs.tl"),
        "the source is the one kept: {:?}",
        found_again[0].declared_in
    );
}

/// `---@contract(dts = "…")` sends it somewhere else, relative to the project root.
#[test]
fn the_publish_target_can_be_named() {
    let (root, cfg) = project("publish-where");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract(dts = \"sdk/defs.d.tl\")\n      \
         name: string   ---@required\n      hp: integer   ---@required\n   end\nend\nreturn defs\n",
    );
    let found = contracts(&root, &cfg);
    let (written, problems) = htl_core::contract::publish(&root, &found);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(written, vec![(root.join("sdk/defs.d.tl"), true)]);
    assert!(!root.join("types/defs.d.tl").exists(), "not the default too");
}

/// A module with bodies in it is published as a declaration: the bodies go, and each
/// function that was part of the interface becomes a field of its record, which is what
/// a hand-written `.d.tl` says. A `local function` is not part of the interface and
/// leaves nothing behind.
#[test]
fn a_module_with_implementations_is_published_as_a_declaration() {
    let (root, cfg) = project("publish-impl");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\n\
         local function round(n: number): integer\n   return n // 1 as integer\nend\n\n\
         function defs.helper(n: integer): integer\n   return round(n)\nend\n\n\
         function defs.Mod.rename(m: defs.Mod, to: string): defs.Mod\n   m.name = to\n   \
         return m\nend\n\nreturn defs\n",
    );
    let found = contracts(&root, &cfg);
    let (written, problems) = htl_core::contract::publish(&root, &found);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(written.len(), 1, "{written:?}");

    let d = std::fs::read_to_string(root.join("types/defs.d.tl")).unwrap();
    assert_eq!(
        d,
        "local record defs\n   record Mod   ---@contract(\"mods\")\n      name: string   ---@required\n      \
         rename: function(m: defs.Mod, to: string): defs.Mod\n   end\n   \
         helper: function(n: integer): integer\nend\n\nreturn defs\n",
        "got:\n{d}"
    );

    // And what it wrote is a declaration Teal accepts, checked as one.
    write(
        &root.join("mods/use.tl"),
        "local defs = require(\"defs\")\nreturn { name = defs.helper(1) }\n",
    );
    let h = Htl::new().unwrap();
    h.add_path(&root.join("types")).unwrap();
    let ci = h.check(&root.join("types/defs.d.tl")).unwrap();
    assert!(ci.ok(), "the published declaration checks: {:?}", ci.errors);
}

/// A method keeps the receiver its definition left implicit.
#[test]
fn a_method_gains_its_self_parameter() {
    let (root, cfg) = project("publish-method");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nfunction defs.Mod:label(): string\n   return self.name\nend\n\nreturn defs\n",
    );
    let found = contracts(&root, &cfg);
    let (_, problems) = htl_core::contract::publish(&root, &found);
    assert!(problems.is_empty(), "{problems:?}");
    let d = std::fs::read_to_string(root.join("types/defs.d.tl")).unwrap();
    assert!(
        d.contains("label: function(self: Mod): string"),
        "got:\n{d}"
    );
}

/// A signature over more than one line is carried over as written.
#[test]
fn a_multi_line_signature_survives() {
    let (root, cfg) = project("publish-wrapped");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nfunction defs.make(name: string,\n                   hp: integer): defs.Mod\n   \
         return { name = name }\nend\n\nreturn defs\n",
    );
    let found = contracts(&root, &cfg);
    let (_, problems) = htl_core::contract::publish(&root, &found);
    assert!(problems.is_empty(), "{problems:?}");
    let d = std::fs::read_to_string(root.join("types/defs.d.tl")).unwrap();
    assert!(
        d.contains("make: function(name: string,\n                   hp: integer): defs.Mod"),
        "got:\n{d}"
    );
}

/// A function on a record nothing declares cannot be placed, and is reported rather than
/// dropped: a declaration missing a function is worse than one that was not written.
#[test]
fn a_function_with_no_record_to_join_is_reported() {
    let (root, cfg) = project("publish-orphan");
    write(
        &root.join("src/defs.tl"),
        "local record defs\n   record Mod   ---@contract\n      name: string   ---@required\n   \
         end\nend\n\nfunction other.f(n: integer): integer\n   return n\nend\n\nreturn defs\n",
    );
    let found = contracts(&root, &cfg);
    let (written, problems) = htl_core::contract::publish(&root, &found);
    assert!(written.is_empty(), "{written:?}");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("nothing declares a record other"),
        "{}",
        problems[0]
    );
    assert!(
        !root.join("types/defs.d.tl").exists(),
        "nothing half-written"
    );
}

/// A contract already declared in a `.d.tl` is its own publication: writing it to itself
/// would be a copy onto the file it was read from.
#[test]
fn a_contract_declared_in_types_is_not_republished() {
    let (root, cfg) = project_declaring_at("publish-self", "types/defs.d.tl", CONTRACT_TOML);
    let found = contracts(&root, &cfg);
    let (written, problems) = htl_core::contract::publish(&root, &found);
    assert!(written.is_empty() && problems.is_empty(), "{written:?}");
}

/// The three constructors build one thing, so they resolve from one set of paths. This
/// is the property the split hid: `contract_resolvers` used to add `search_paths` itself
/// on top of what `for_contract_dir` hard-coded, so only the outermost one was whole.
#[test]
fn the_three_constructors_resolve_the_same_way() {
    let (root, cfg) = project_declaring_at("same-paths", "types/defs.d.tl", CONTRACT_TOML);
    let found = contracts(&root, &cfg);
    let c = &found[0];
    let dir = root.join("mods");

    let by_config = verdicts(
        &Htl::new().unwrap(),
        htl_core::pkg::contract_resolvers(&root, &cfg).unwrap(),
    );
    let by_contract = verdicts(
        &Htl::new().unwrap(),
        Resolver::for_contract(&root, &cfg, c).unwrap(),
    );
    let by_dir = verdicts(
        &Htl::new().unwrap(),
        vec![Resolver::for_contract_dir(&root, &dir, &cfg, c).unwrap()],
    );

    assert_eq!(by_config, by_contract, "contract_resolvers vs for_contract");
    assert_eq!(by_contract, by_dir, "for_contract vs for_contract_dir");
    assert!(by_config[0].is_ok(), "conforming mod: {:?}", by_config[0]);
}

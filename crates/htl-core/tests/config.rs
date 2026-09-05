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

const CONTRACT_TOML: &str = "[[contract]]\ndir = \"mods\"\ntype = \"defs.Mod\"\nrequire_fields = true\n";

/// Project with `htl.toml`, `src/defs.tl` and three mods: conforming, wrong field
/// type, and one that leaves a declared field out.
fn project(name: &str) -> (PathBuf, HtlConfig) {
    let root = scratch(name);
    write(&root.join("htl.toml"), CONTRACT_TOML);
    write(
        &root.join("src").join("defs.tl"),
        "local record defs\n   record Mod\n      name: string\n      hp: integer\n   end\nend\nreturn defs\n",
    );
    write(&root.join("mods").join("good.tl"), "return { name = \"swarm\", hp = 3 }\n");
    write(&root.join("mods").join("bad.tl"), "return { name = \"broken\", hp = \"lots\" }\n");
    write(&root.join("mods").join("partial.tl"), "return { name = \"half\" }\n");
    let (_, cfg) = HtlConfig::find(&root).unwrap().expect("htl.toml just written");
    (root, cfg)
}

#[test]
fn parse_sections_and_lint_spec() {
    let cfg = HtlConfig::parse(
        "[lint]\nenable = [\"class-record\", \"explicit-number\"]\ndisable = [\"shadow-local\"]\nstrict = true\n\
         [fmt]\nindent = 2\n[[contract]]\ndir = \"mods\"\ntype = \"defs.Mod\"\n",
    )
    .unwrap();
    assert_eq!(cfg.lint_spec(), "+class-record,+explicit-number,-shadow-local");
    assert_eq!(cfg.lint.strict, Some(true));
    assert_eq!(cfg.fmt.indent, Some(2));
    assert_eq!(cfg.contract.len(), 1);
    assert_eq!(cfg.contract[0].type_path, "defs.Mod");
    assert!(!cfg.contract[0].require_fields, "defaults to off");
    assert_eq!(join_specs([cfg.lint_spec().as_str(), "", "+shadow-local"]), "+class-record,+explicit-number,-shadow-local,+shadow-local");
    assert!(HtlConfig::parse("[lint]\nstrictness = true\n").is_err(), "unknown keys are errors");
}

#[test]
fn find_walks_up_from_a_file() {
    let root = scratch("find");
    write(&root.join("htl.toml"), "[fmt]\nindent = 4\n");
    write(&root.join("src").join("deep").join("x.tl"), "return 1\n");
    let (path, cfg) = HtlConfig::find(&root.join("src/deep/x.tl")).unwrap().unwrap();
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
    assert!(bad[0].contains("does not satisfy contract defs.Mod"), "{}", bad[0]);
    assert!(bad[0].contains("[htl contract]"), "{}", bad[0]);

    let partial = contract_lints(&h, &root, &cfg, &root.join("mods/partial.tl")).unwrap();
    assert_eq!(partial.len(), 1, "{partial:?}");
    assert!(partial[0].contains("lacks declared field(s) of defs.Mod: hp"), "{}", partial[0]);
    assert!(partial[0].contains("mods/partial.tl:1:"), "points at the return: {}", partial[0]);

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
    assert!(partial.is_empty(), "every record field is nilable: {partial:?}");
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
    assert!(no_fields[0].contains("never calls .require_fields()"), "{}", no_fields[0]);

    let by_hand = host("let r = TealResolver::new(\"mods\")?.expect_type(\"defs.Mod\").require_fields();\n");
    assert!(by_hand.is_empty(), "{by_hand:?}");

    let by_config = host("for r in htl::pkg::contract_resolvers(&root, &cfg)? { reg.add(r); }\n");
    assert!(by_config.is_empty(), "{by_config:?}");

    // No host crate at all: a script-only project has nothing to enforce.
    assert!(contract_enforcement_lints(&cfg, &cfg_path, None).is_empty());
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
    let bad = h.lua().load("return require('bad')").eval::<mlua::Value>().unwrap_err().to_string();
    assert!(bad.contains("does not satisfy defs.Mod"), "{bad}");
    let partial = h.lua().load("return require('partial')").eval::<mlua::Value>().unwrap_err().to_string();
    assert!(partial.contains("hp"), "missing field named: {partial}");
}

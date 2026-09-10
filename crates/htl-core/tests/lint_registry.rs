//! The registry and the implementations it names.
//!
//! `lint::RULES` is the one list of rule names: what `--lint` and `[lint]` accept, what
//! `--list-lints` prints, and what both halves of htl report under. `lint.lua` still owns
//! the *implementations* of its twelve, and that is a second list of names — the one place
//! left where a rename could go half done, leaving a rule that nothing runs and nothing
//! says so. This holds the two together, so that goes wrong as a failing test rather than
//! as a lint that quietly stops firing.

use htl_core::Htl;
use htl_core::lint::{Level, RULES, Side, Surfaces};

#[test]
fn lint_lua_implements_exactly_the_lua_side_of_the_registry() {
    let h = Htl::new().unwrap();
    let mut implemented = h.lua_lint_rules().unwrap();
    let mut registered: Vec<String> = RULES
        .iter()
        .filter(|r| r.side == Side::Lua)
        .map(|r| r.name.to_string())
        .collect();
    implemented.sort();
    registered.sort();
    assert_eq!(
        implemented, registered,
        "lint.lua's RULES and the Lua side of lint::RULES have to be the same names"
    );
}

/// The other side has no such list to compare against — the project layer's rules are
/// raised by name in the message each one formats — so what is asserted is that the
/// registry knows them. A rule dropped from `RULES` would print a name `--lint` refuses,
/// which is the defect the registry exists to prevent.
#[test]
fn the_registry_knows_the_rules_the_project_layer_reports_under() {
    for rule in [
        "duplicate-declaration",
        "host-module-shadowed",
        "contract",
        "contract-unenforced",
        "require-cycle",
    ] {
        let r = RULES
            .iter()
            .find(|r| r.name == rule)
            .unwrap_or_else(|| panic!("{rule} is not in the registry"));
        assert_eq!(r.side, Side::Rust, "{rule}");
        assert_eq!(
            r.default,
            Level::Warn,
            "{rule} is reported, and advisory, unless a project says otherwise"
        );
    }
}

/// The third producer is the vendored Teal compiler, whose seven warning kinds are
/// registered under a `tl:` prefix. Their names are not htl's to choose — the compiler
/// tags each warning with one — so what is held here is that the registry carries every
/// kind the vendored `tl.warning_kinds` declares, and none that it does not. A Teal
/// upgrade that adds an eighth fails this test rather than shipping a kind that
/// `--list-lints` never mentions and `[lint]` refuses by name.
#[test]
fn the_registry_carries_exactly_the_vendored_compilers_warning_kinds() {
    let vendored = include_str!("../vendor/tl.lua");
    // `local wk = { ["unused"] = true, ... }` — the table `tl.warning_kinds` is set from.
    let table = vendored
        .split_once("local wk = {")
        .expect("the warning-kind table")
        .1
        .split_once('}')
        .expect("the end of it")
        .0;
    let mut kinds: Vec<String> = table
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|k| format!("tl:{k}"))
        .collect();
    assert_eq!(kinds.len(), 7, "{kinds:?}");

    // The `tl` side also holds the two classes `htl fix` files an error under, which are
    // not warning kinds and are not lints: they belong to the fix surface alone, and the
    // test below is what holds them.
    let mut registered: Vec<String> = RULES
        .iter()
        .filter(|r| r.side == Side::Tl && r.is_lint())
        .map(|r| r.name.to_string())
        .collect();
    for r in RULES.iter().filter(|r| r.side == Side::Tl && r.is_lint()) {
        assert_eq!(
            r.default,
            Level::Warn,
            "{} is reported: Teal chose to say it, and as a warning",
            r.name
        );
    }
    kinds.sort();
    registered.sort();
    assert_eq!(
        kinds, registered,
        "the vendored compiler's warning kinds and the tl side of lint::RULES have to be \
         the same names"
    );
}

/// The registry also holds the two names that are not lints: the classes `htl fix` files
/// an error's fix under. A check reports under neither, so what is asserted is the
/// negative — they are on the fix surface only, which is what keeps them out of
/// `--list-lints` and out of `[lint.rules]`.
#[test]
fn the_registry_holds_the_fix_classes_and_marks_them_as_not_lints() {
    let classes: Vec<&str> = RULES
        .iter()
        .filter(|r| !r.is_lint())
        .map(|r| r.name)
        .collect();
    assert_eq!(classes, ["forward-ref", "tl:error"], "{classes:?}");
    for r in RULES.iter().filter(|r| !r.is_lint()) {
        assert_eq!(r.surfaces, Surfaces::FixOnly, "{}", r.name);
        // The compiler is what produced the diagnostic; htl only chose what to call it.
        assert_eq!(r.side, Side::Tl, "{}", r.name);
    }
}

/// A state nobody configured runs the defaults. The defaults live on the Rust side now,
/// so this is the assertion that they still reach Lua.
#[test]
fn a_fresh_checker_runs_the_default_rules() {
    let dir = std::env::temp_dir().join("htl-core-lint-registry");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("defaults.tl");
    std::fs::write(
        &f,
        "local t: {string:{string:string}} = {}\nprint(t[\"a\"].b)\n",
    )
    .unwrap();

    let h = Htl::new().unwrap();
    let c = h.check(&f).unwrap();
    assert!(
        c.lints.iter().any(|l| l.contains("[htl nil-index]")),
        "an on-by-default rule: {:?}",
        c.lints
    );
    assert!(
        !c.lints.iter().any(|l| l.contains("[htl no-any]")),
        "an off-by-default rule: {:?}",
        c.lints
    );
}

/// The kind reaches `CheckInfo.warnings` in the shape every other rule name arrives in,
/// which is what lets `Diagnostic::parse` read it with no case of its own.
#[test]
fn a_teal_warning_carries_its_kind_into_the_check_result() {
    let dir = std::env::temp_dir().join("htl-core-lint-registry-warnings");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("warned.tl");
    std::fs::write(
        &f,
        "local function go(): integer\n   local unread = 1\n   return 2\nend\nreturn go\n",
    )
    .unwrap();

    let h = Htl::new().unwrap();
    let c = h.check(&f).unwrap();
    assert!(
        c.warnings.iter().any(|w| w.contains("[htl tl:unused]")),
        "{:?}",
        c.warnings
    );
    let d = c.warning_diagnostics();
    let named = d.iter().find(|d| d.rule.as_deref() == Some("tl:unused"));
    let named = named.unwrap_or_else(|| panic!("no tl:unused among {d:?}"));
    // The name is split off, so the message still reads as a sentence.
    assert!(!named.message.contains("[htl"), "{}", named.message);
}

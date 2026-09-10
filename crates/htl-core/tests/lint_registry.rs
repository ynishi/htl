//! The registry and the implementations it names.
//!
//! `lint::RULES` is the one list of rule names: what `--lint` and `[lint]` accept, what
//! `--list-lints` prints, and what both halves of htl report under. `lint.lua` still owns
//! the *implementations* of its twelve, and that is a second list of names — the one place
//! left where a rename could go half done, leaving a rule that nothing runs and nothing
//! says so. This holds the two together, so that goes wrong as a failing test rather than
//! as a lint that quietly stops firing.

use htl_core::Htl;
use htl_core::lint::{RULES, Side};

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
        assert!(r.default_on, "{rule} is on unless a project says otherwise");
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

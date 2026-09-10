//! `htl dts` writes the declarations `#[derive(TealRecord)]` asks for from the Rust
//! source alone — an enum and a newtype included — so a fresh checkout checks before
//! anything is built. The crate here is never compiled; the scan is syntactic.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;

fn scratch(name: &str) -> PathBuf {
    common::scratch("htl-cli-dts", name)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn htl(root: &Path, args: &[&str]) -> Output {
    Command::new(common::htl_bin())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
}

fn host_crate(name: &str) -> PathBuf {
    let root = scratch(name);
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    // The project root: `src/` and `types/` are searched from here.
    write(&root.join("htl.toml"), "");
    write(
        &root.join("src/main.rs"),
        "use htl::{TealRecord, host_module};\n\n\
         #[derive(TealRecord)]\n#[teal(dts = \"types/Mode.d.tl\")]\npub enum Mode { Fast, Careful }\n\n\
         #[derive(TealRecord)]\n#[teal(dts = \"types/Sql.d.tl\")]\npub struct Sql(pub String);\n\n\
         #[derive(TealRecord)]\npub enum Shape { Dot, Circle(f64) }\n\n\
         pub struct Host;\n\n\
         #[host_module(name = \"host\", dts = \"types/host.d.tl\", uses = [Mode, Sql], records = [Shape])]\n\
         impl Host {\n    pub fn run(&self, m: Mode, q: Sql, s: Shape) -> Shape { s }\n}\n\nfn main() {}\n",
    );
    root
}

#[test]
fn dts_writes_an_enum_an_alias_and_a_nested_union() {
    let root = host_crate("kinds");
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("htl dts: 3 declaration(s)"), "{err}");
    assert_eq!(
        std::fs::read_to_string(root.join("types/Mode.d.tl")).unwrap(),
        "local enum Mode\n   \"Fast\"\n   \"Careful\"\nend\n\nreturn Mode\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("types/Sql.d.tl")).unwrap(),
        "local type Sql = string\n\nreturn Sql\n"
    );
    let host = std::fs::read_to_string(root.join("types/host.d.tl")).unwrap();
    assert!(
        host.starts_with(
            "local type Mode = require(\"Mode\")\nlocal type Sql = require(\"Sql\")\n\nlocal record host\n"
        ),
        "{host}"
    );
    assert!(host.contains("   record Shape_Circle\n      where self.kind == \"Circle\"\n      kind: string\n      value: number\n   end\n   type Shape = Shape_Dot | Shape_Circle\n"), "{host}");
    assert!(
        host.contains("   run: function(self: host, m: Mode, q: Sql, s: Shape): Shape\n"),
        "{host}"
    );

    // The declarations check as a set: a script narrowing the union against the host
    // module's variant records, passing the enum as a string and the alias as a string.
    write(
        &root.join("src/main.tl"),
        "local type host = require(\"host\")\n\n\
         local h: host = nil\n\
         local s = h:run(\"Fast\", \"select 1\", { kind = \"Dot\" } as host.Shape_Dot)\n\
         if s is host.Shape_Circle then\n   print(s.value)\nelseif s is host.Shape_Dot then\n   print(\"dot\")\nend\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("0 error(s)"), "{err}");

    // A wrong enum value is a check error that names the enum.
    write(
        &root.join("src/bad.tl"),
        "local type host = require(\"host\")\n\nlocal h: host = nil\nh:run(\"fst\", \"q\", { kind = \"Dot\" } as host.Shape_Dot)\n",
    );
    let out = htl(&root, &["check", "src/bad.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("Mode"), "{err}");

    // Nothing to write the second time.
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(", 0 written"), "{err}");
}

/// `#[teal(rename_all)]` reaches the `.d.tl` the CLI writes from the source alone — the
/// same text the macro writes at build — and the checker holds the renamed words: a
/// script passing the Rust spelling is a type error before anything runs.
#[test]
fn dts_writes_the_renamed_words_and_check_holds_them() {
    let root = scratch("renamed");
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(&root.join("htl.toml"), "");
    write(
        &root.join("src/main.rs"),
        "use htl::{TealRecord, host_module};\n\n\
         #[derive(TealRecord)]\n#[teal(dts = \"types/State.d.tl\", rename_all = \"snake_case\")]\n\
         pub enum State { Open, InReview, #[teal(name = \"done\")] Closed }\n\n\
         pub struct Host;\n\n\
         #[host_module(name = \"host\", dts = \"types/host.d.tl\", uses = [State])]\n\
         impl Host {\n    pub fn advance(&self, s: State) -> State { s }\n}\n\nfn main() {}\n",
    );
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert_eq!(
        std::fs::read_to_string(root.join("types/State.d.tl")).unwrap(),
        "local enum State\n   \"open\"\n   \"in_review\"\n   \"done\"\nend\n\nreturn State\n"
    );

    write(
        &root.join("src/main.tl"),
        "local type host = require(\"host\")\n\nlocal h: host = nil\nprint(h:advance(\"open\"))\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("0 error(s)"), "{err}");

    // The Rust spelling is not one of the words, and the checker says so.
    write(
        &root.join("src/bad.tl"),
        "local type host = require(\"host\")\n\nlocal h: host = nil\nprint(h:advance(\"Open\"))\n",
    );
    let out = htl(&root, &["check", "src/bad.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("State"), "{err}");
}

/// An `Option<T>` parameter is declared `name?: T` from the source alone, and the checker
/// then accepts the call that leaves the argument out as well as the one that passes it.
/// A record field of the same shape stays `T` — every Teal record field is nilable.
#[test]
fn dts_marks_an_option_parameter_optional_and_check_takes_both_calls() {
    let root = scratch("optional");
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(&root.join("htl.toml"), "");
    write(
        &root.join("src/main.rs"),
        "use htl::{TealRecord, host_module};\n\n\
         #[derive(TealRecord)]\npub struct Outcome { pub did: String, pub blocked: Option<String> }\n\n\
         pub struct Api;\n\n\
         #[host_module(name = \"api\", dts = \"types/api.d.tl\", records = [Outcome])]\n\
         impl Api {\n    pub fn find(&self, name: &str, scope: Option<String>) -> Option<Outcome> { todo!() }\n}\n\nfn main() {}\n",
    );
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let api = std::fs::read_to_string(root.join("types/api.d.tl")).unwrap();
    assert!(
        api.contains("   find: function(self: api, name: string, scope?: string): Outcome\n"),
        "{api}"
    );
    assert!(
        api.contains("   record Outcome\n      did: string\n      blocked: string\n   end\n"),
        "{api}"
    );

    write(
        &root.join("src/main.tl"),
        "local type api = require(\"api\")\n\n\
         local a: api = nil\nlocal one = a:find(\"x\")\nlocal two = a:find(\"x\", \"y\")\n\
         print(one.did, two.did)\n",
    );
    let out = htl(&root, &["check", "src/main.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("0 error(s)"), "{err}");

    // The mark says "may be left out", not "any number of arguments".
    write(
        &root.join("src/bad.tl"),
        "local type api = require(\"api\")\n\nlocal a: api = nil\nprint(a:find(\"x\", \"y\", \"z\"))\n",
    );
    let out = htl(&root, &["check", "src/bad.tl", "--no-cache"]);
    let err =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("wrong number of arguments"), "{err}");
}

#[test]
fn a_data_enum_asking_for_its_own_dts_is_refused_with_advice() {
    let root = scratch("refused");
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        &root.join("src/lib.rs"),
        "#[derive(TealRecord)]\n#[teal(dts = \"types/Shape.d.tl\")]\npub enum Shape { Dot, Circle(f64) }\n",
    );
    let out = htl(&root, &["dts"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("records = [Shape]"), "{err}");
    assert!(!root.join("types/Shape.d.tl").exists());
}

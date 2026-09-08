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
    Command::new(env!("CARGO_BIN_EXE_htl"))
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

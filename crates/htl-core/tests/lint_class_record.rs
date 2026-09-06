//! `class-record` (opt-in): records that declare metamethods are classes in disguise;
//! their metatable does not survive serialization or the Rust boundary.

use htl_core::Htl;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "htl-core-classrec-{name}-{}-{}",
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

fn lints_of(dir: &Path, file: &str, spec: Option<&str>) -> Vec<String> {
    let h = Htl::new().unwrap();
    if let Some(s) = spec {
        h.configure_lints(s).unwrap();
    }
    h.add_path(dir).unwrap();
    let ci = h.check(&dir.join(file)).unwrap();
    assert!(ci.ok(), "unexpected type errors: {:?}", ci.errors);
    ci.lints
}

const SRC: &str = "local record Actor\n   hp: integer\n   metamethod __index: Actor\nend\n\
function Actor.new(hp: integer): Actor\n   return setmetatable({ hp = hp } as Actor, { __index = Actor } as metatable<Actor>)\nend\n\
function Actor:hit(n: integer)\n   self.hp = self.hp - n\nend\n\n\
local record Pos\n   x: number\n   y: number\nend\n\n\
local record game\n   record Vec\n      x: number\n      metamethod __add: function(a: Vec, b: Vec): Vec\n   end\n   record Plain\n      n: integer\n   end\nend\n\n\
local a = Actor.new(3)\na:hit(1)\nlocal p: Pos = { x = 1, y = 2 }\nprint(a.hp, p.x)\n";

#[test]
fn off_by_default() {
    let dir = scratch("default");
    write(&dir.join("c.tl"), SRC);
    let lints = lints_of(&dir, "c.tl", None);
    assert!(
        !lints.iter().any(|l| l.contains("class-record")),
        "{lints:?}"
    );
}

#[test]
fn flags_records_with_metamethods_including_nested() {
    let dir = scratch("on");
    write(&dir.join("c.tl"), SRC);
    let lints: Vec<String> = lints_of(&dir, "c.tl", Some("+class-record"))
        .into_iter()
        .filter(|l| l.contains("class-record"))
        .collect();
    assert!(
        lints
            .iter()
            .any(|l| l.contains("record Actor declares metamethod(s) __index")),
        "{lints:?}"
    );
    assert!(
        lints
            .iter()
            .any(|l| l.contains("record game.Vec declares metamethod(s) __add")),
        "{lints:?}"
    );
    assert!(
        !lints
            .iter()
            .any(|l| l.contains("Pos") || l.contains("Plain")),
        "plain records flagged: {lints:?}"
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
}

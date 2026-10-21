//! `CheckInfo::types`: what the checker knows of each name of a file, by position. Only
//! `Htl::check_indexed` keeps it; a question about a position is answered from it with no
//! check of its own (`htl lsp` is the caller).

use htl_core::Htl;
use htl_core::typeindex::TypeAt;
use std::path::Path;

mod common;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const MODULE: &str = "local record m
   record Cfg
      a: integer
   end
   name: string
end

function m.hello(who: string): string
   return \"hello \" .. who
end

return m
";

// Line 1 requires the module; 2-3 use its type; 4-5 shadow `x`; 6 is a comment and a string.
const USER: &str = "local m = require(\"m\")
local c: m.Cfg = { a = 1 }
print(m.hello(\"w\"), m.name, c.a)
local x = 1
do local x = \"s\"; print(x) end
-- m is here  \"c\"
print(x, \"c\")
";

/// The answer at the first `needle` of `text`, `nth` characters into it.
fn at(c: &htl_core::CheckInfo, text: &str, needle: &str, nth: usize) -> Option<TypeAt> {
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in the text"))
        + nth;
    let line = text[..offset].matches('\n').count() + 1;
    let col = offset - text[..offset].rfind('\n').map_or(0, |i| i + 1) + 1;
    c.type_at(text, line, col)
}

#[test]
fn a_check_keeps_the_index_only_when_asked() {
    let root = common::tempdir("htl-core-typeindex", "asked");
    write(&root.join("m.tl"), MODULE);
    write(&root.join("user.tl"), USER);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let plain = h.check_written(&root.join("user.tl")).unwrap();
    assert!(plain.types.is_empty());
    assert_eq!(plain.type_at(USER, 1, 7), None);
    let kept = h.check_indexed(&root.join("user.tl")).unwrap();
    assert!(!kept.types.is_empty());
    // The same check otherwise: the index is a copy, not a different question.
    assert_eq!(plain.errors, kept.errors);
    assert_eq!(plain.deps, kept.deps);
}

#[test]
fn a_name_has_its_type_and_its_declaration() {
    let root = common::tempdir("htl-core-typeindex", "names");
    write(&root.join("m.tl"), MODULE);
    write(&root.join("user.tl"), USER);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let c = h.check_indexed(&root.join("user.tl")).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);

    // A local: its type, and the name it was declared by.
    let t = at(&c, USER, "c.a", 0).unwrap();
    assert_eq!(t.ty.as_deref(), Some("m.Cfg"));
    assert_eq!((t.line, t.col, t.end), (3, 29, 30));
    let d = t.declared.unwrap();
    assert_eq!((d.line, d.col), (2, 7));
    assert_eq!(d.file, root.join("user.tl"));

    // A field that is a function: where the function is written, in the other file. From
    // the first letter, the middle and just after.
    for nth in [0, 2, 5] {
        let t = at(&c, USER, "hello", nth).unwrap();
        assert_eq!(t.ty.as_deref(), Some("function(string): string"), "{nth}");
        assert_eq!((t.col, t.end), (9, 14), "{nth}");
        let d = t.declared.unwrap();
        assert_eq!((d.line, d.col), (8, 1), "{nth}");
        assert!(d.file.ends_with("m.tl"), "{d:?}");
    }

    // A field of a primitive type has a type and nowhere to point.
    let t = at(&c, USER, "name", 0).unwrap();
    assert_eq!((t.ty.as_deref(), t.declared), (Some("string"), None));

    // A type's own name: the record it is, in the file that declares it.
    let t = at(&c, USER, "Cfg", 0).unwrap();
    let d = t.declared.unwrap();
    assert_eq!((d.line, d.col), (2, 4));
    assert!(d.file.ends_with("m.tl"), "{d:?}");

    // The module's local, from its declaration, from a use and from the head of a type
    // written through it (whose whole type the report keeps under that `m`).
    for needle in ["m = require", "m.hello", "m.Cfg"] {
        let t = at(&c, USER, needle, 0).unwrap();
        assert_eq!(t.ty.as_deref(), Some("m"), "{needle}");
        assert_eq!(
            t.declared.map(|d| (d.line, d.col)),
            Some((1, 7)),
            "{needle}"
        );
    }
}

#[test]
fn the_innermost_declaration_in_scope_is_the_one_found() {
    let root = common::tempdir("htl-core-typeindex", "scope");
    write(&root.join("m.tl"), MODULE);
    write(&root.join("user.tl"), USER);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let c = h.check_indexed(&root.join("user.tl")).unwrap();
    // `x` inside the `do` is the string declared there; after the block, the integer.
    let inner = at(&c, USER, "print(x) end", 6).unwrap();
    assert_eq!(inner.ty.as_deref(), Some("string"));
    assert_eq!(inner.declared.map(|d| (d.line, d.col)), Some((5, 10)));
    let outer = at(&c, USER, "print(x, ", 6).unwrap();
    assert_eq!(outer.ty.as_deref(), Some("integer"));
    assert_eq!(outer.declared.map(|d| (d.line, d.col)), Some((4, 7)));
}

#[test]
fn what_is_not_a_name_has_no_answer() {
    let root = common::tempdir("htl-core-typeindex", "none");
    write(&root.join("m.tl"), MODULE);
    write(&root.join("user.tl"), USER);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let c = h.check_indexed(&root.join("user.tl")).unwrap();
    // A keyword, a number, a comment (its `m` is a local's name), a string (so is its `c`).
    assert_eq!(at(&c, USER, "local x = 1", 0), None);
    assert_eq!(at(&c, USER, "= 1\n", 2), None);
    assert_eq!(at(&c, USER, "m is here", 0), None);
    assert_eq!(at(&c, USER, "\"c\")\n", 1), None);
    // A position off the text.
    assert_eq!(c.type_at(USER, 99, 1), None);
    assert_eq!(c.type_at(USER, 0, 1), None);
    assert_eq!(c.type_at(USER, 4, 400), None);
    // The standard library is declared in a file that is not one: a position, no more.
    let p = at(&c, USER, "print(m.hello", 0).unwrap();
    assert!(p.ty.unwrap().starts_with("function("));
}

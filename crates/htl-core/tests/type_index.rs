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

/// The tree as one line per declaration, indented by depth: kind, name, where it starts and
/// ends (`end` just past it, `-` for "the end of the line"), where its name is, its type.
fn outline(decls: &[htl_core::typeindex::Decl], depth: usize, out: &mut String) {
    for d in decls {
        let end = d.end_col.map_or("-".to_string(), |c| c.to_string());
        let detail = d.detail.as_deref().unwrap_or("");
        out.push_str(&format!(
            "{}{:?} {} {}:{}..{}:{} @{}:{} {}\n",
            "  ".repeat(depth),
            d.kind,
            d.name,
            d.line,
            d.col,
            d.end_line,
            end,
            d.name_line,
            d.name_col,
            detail
        ));
        outline(&d.children, depth + 1, out);
    }
}

fn outline_of(root: &Path, name: &str, text: &str) -> Option<String> {
    write(&root.join(name), text);
    let h = Htl::new().unwrap();
    h.add_path(root).unwrap();
    let c = h.check_indexed(&root.join(name)).unwrap();
    let mut out = String::new();
    outline(c.types.declarations()?, 0, &mut out);
    Some(out)
}

const SHAPES: &str = "local record Greeting
   who: string
   cb: function(string): string
   record Inner
      a: number
   end
   enum Kind
      \"a\"
   end
end

local enum Color
   \"red\"
end

local type Alias = Greeting

local function helper(x: integer): integer
   return x + 1
end

function Greeting.make(who: string): Greeting
   return { who = who }
end

function Greeting:show(): string
   return self.who
end

local v = 1
local t = {
   k = 1,
}
return Greeting
";

#[test]
fn the_declarations_of_a_file_are_a_tree() {
    let root = common::tempdir("htl-core-typeindex", "decls");
    let got = outline_of(&root, "shapes.tl", SHAPES).unwrap();
    // Positions: lines from 1 and byte columns from 1; a record function is a child of its
    // record though it is written after the record's `end`; an enum's members are not listed.
    let want = "\
Record Greeting 1:1..10:4 @1:14 \n\
\x20 Field who 2:4..2:- @2:4 string\n\
\x20 Function cb 3:4..3:- @3:4 function(string): string\n\
\x20 Record Inner 4:4..6:7 @4:11 \n\
\x20   Field a 5:7..5:- @5:7 number\n\
\x20 Enum Kind 7:4..9:7 @7:9 \n\
\x20 Function make 22:1..24:4 @22:19 function(string): Greeting\n\
\x20 Method show 26:1..28:4 @26:19 function(Greeting): string\n\
Enum Color 12:1..14:4 @12:12 \n\
Alias Alias 16:1..16:- @16:12 Greeting\n\
Function helper 18:1..20:4 @18:16 function(integer): integer\n\
Local v 30:1..30:- @30:7 integer\n\
Local t 31:1..33:2 @31:7 record\n";
    assert_eq!(got, want, "\n{got}");
}

#[test]
fn a_function_of_a_record_declared_elsewhere_stands_at_the_top() {
    let root = common::tempdir("htl-core-typeindex", "orphan");
    write(&root.join("m.tl"), MODULE);
    let user = "local m = require(\"m\")

function m.extra(): string
   return \"x\"
end

local up = m.hello
return m
";
    let got = outline_of(&root, "user.tl", user).unwrap();
    let want = "\
Local m 1:1..1:23 @1:7 m\n\
Function m.extra 3:1..5:4 @3:12 \n\
Function up 7:1..7:- @7:7 function(string): string\n";
    assert_eq!(got, want, "\n{got}");
}

#[test]
fn a_file_that_does_not_parse_has_no_declarations_and_one_that_declares_none_has_an_empty_list() {
    let root = common::tempdir("htl-core-typeindex", "none-decl");
    assert_eq!(outline_of(&root, "bad.tl", "local x = (\n"), None);
    assert_eq!(
        outline_of(&root, "empty.tl", "print(1)\n").as_deref(),
        Some("")
    );
    // The plain check keeps no index, and so no tree.
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let plain = h.check_written(&root.join("empty.tl")).unwrap();
    assert!(plain.types.declarations().is_none());
}

/// What tl puts in a record's `fields` that the record's body does not declare, and the
/// shapes the first walk got wrong: the fields `is` takes from an interface are not listed
/// under the record, a generic function field is a function, a nested `type X = record` has
/// its name where it is written, a `local type` of a `require` is an alias, and a variable
/// whose initialiser a macroexp expanded (a node with a `yend` and no `xend`) still has an
/// outline.
#[test]
fn what_the_body_does_not_declare_is_not_its_child() {
    let root = common::tempdir("htl-core-typeindex", "inherited");
    write(&root.join("m.tl"), MODULE);
    let text = "local interface Shape
   area: function(Shape): number
end

local record Sq is Shape
   side: number
   id: function<T>(T): T
   type Pt = record
      x: number
   end
end

function Sq:area(): number
   return self.side * self.side
end

local type M = require(\"m\")

local macroexp one(a: integer): {integer}
   return { a }
end

local pair = one(3)
return Sq
";
    let got = outline_of(&root, "shapes.tl", text).unwrap();
    let want = "\
Interface Shape 1:1..3:4 @1:17 \n\
\x20 Function area 2:4..2:- @2:4 function(Shape): number\n\
Record Sq 5:1..11:4 @5:14 \n\
\x20 Field side 6:4..6:- @6:4 number\n\
\x20 Function id 7:4..7:- @7:4 function<T>(T): T\n\
\x20 Record Pt 8:14..10:7 @8:9 \n\
\x20   Field x 9:7..9:- @9:7 number\n\
\x20 Method area 13:1..15:4 @13:13 function(Shape): number\n\
Alias M 17:1..17:28 @17:12 m\n\
Function one 19:1..21:4 @19:16 function(integer): {integer}\n\
Local pair 23:1..23:- @23:7 {integer}\n";
    assert_eq!(got, want, "\n{got}");
}

#[test]
fn a_function_declared_in_the_record_and_defined_below_is_one_child() {
    let root = common::tempdir("htl-core-typeindex", "declared-defined");
    let module = "local record M
   f: function(integer): string
   g: function(M, integer): string
   h: function(): integer
   n: integer
end

function M.f(n: integer): string
   return tostring(n)
end

function M:g(n: integer): string
   return tostring(n)
end

function M.extra(): integer
   return 1
end

return M
";
    let got = outline_of(&root, "m.tl", module).unwrap();
    // `f` and `g` are declared in the body and defined below: one entry each, at the
    // definition (`g` a method, as `M:g` defines it). `h` is declared and never defined, so
    // it stays at the body's line; `extra` is defined and never declared.
    let want = "\
Record M 1:1..6:4 @1:14 \n\
\x20 Function h 4:4..4:- @4:4 function(): integer\n\
\x20 Field n 5:4..5:- @5:4 integer\n\
\x20 Function f 8:1..10:4 @8:12 function(integer): string\n\
\x20 Method g 12:1..14:4 @12:12 function(M, integer): string\n\
\x20 Function extra 16:1..18:4 @16:12 function(): integer\n";
    assert_eq!(got, want, "\n{got}");
}

/// [`outline`] without the type column: what a parse alone can say.
fn shape(decls: &[htl_core::typeindex::Decl], depth: usize, out: &mut String) {
    for d in decls {
        let end = d.end_col.map_or("-".to_string(), |c| c.to_string());
        out.push_str(&format!(
            "{}{:?} {} {}:{}..{}:{} @{}:{}\n",
            "  ".repeat(depth),
            d.kind,
            d.name,
            d.line,
            d.col,
            d.end_line,
            end,
            d.name_line,
            d.name_col
        ));
        shape(&d.children, depth + 1, out);
    }
}

/// `Htl::declarations_of` reads the same tree a check keeps, without the types: the same
/// kinds, names and positions, nothing in `detail`.
#[test]
fn the_declarations_of_a_parse_are_the_ones_of_a_check_without_types() {
    let root = common::tempdir("htl-core-typeindex", "parse-only");
    write(&root.join("m.tl"), MODULE);
    let orphan = "local m = require(\"m\")

function m.extra(): string
   return \"x\"
end

local lit = function(): integer return 1 end
return m
";
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    for (name, text) in [
        ("shapes.tl", SHAPES),
        ("m.tl", MODULE),
        ("orphan.tl", orphan),
    ] {
        write(&root.join(name), text);
        let checked = h.check_indexed(&root.join(name)).unwrap();
        let mut want = String::new();
        shape(checked.types.declarations().unwrap(), 0, &mut want);
        let parsed = h.declarations_of(&root.join(name)).unwrap().unwrap();
        let mut got = String::new();
        shape(&parsed, 0, &mut got);
        assert_eq!(got, want, "{name}");
        assert!(!got.is_empty());
        fn no_types(ds: &[htl_core::typeindex::Decl]) -> bool {
            ds.iter()
                .all(|d| d.detail.is_none() && no_types(&d.children))
        }
        assert!(no_types(&parsed), "{name}");
    }
}

/// The one kind a type decides: a variable that holds a function without writing one
/// (`local up = m.hello`) is a variable until the checker says what it holds.
#[test]
fn a_variable_holding_a_function_by_name_is_a_function_only_once_checked() {
    let root = common::tempdir("htl-core-typeindex", "var-fn");
    write(&root.join("m.tl"), MODULE);
    let user = "local m = require(\"m\")\nlocal up = m.hello\nreturn up\n";
    write(&root.join("user.tl"), user);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let parsed = h.declarations_of(&root.join("user.tl")).unwrap().unwrap();
    assert_eq!(parsed[1].kind, htl_core::typeindex::DeclKind::Local);
    let checked = h.check_indexed(&root.join("user.tl")).unwrap();
    let kind = checked.types.declarations().unwrap()[1].kind;
    assert_eq!(kind, htl_core::typeindex::DeclKind::Function);
}

/// A type error is not looked for; a syntax error leaves no tree; a file that declares
/// nothing has an empty one; a file that is not there has none.
#[test]
fn declarations_of_ignore_type_errors_and_stop_at_syntax_errors() {
    let root = common::tempdir("htl-core-typeindex", "parse-errors");
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let typed_wrong = "local function f(x: integer): string
   return x
end
local n: integer = \"not a number\"
";
    write(&root.join("wrong.tl"), typed_wrong);
    assert!(!h.check_written(&root.join("wrong.tl")).unwrap().ok());
    let got = h.declarations_of(&root.join("wrong.tl")).unwrap().unwrap();
    let names: Vec<_> = got.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["f", "n"]);
    write(&root.join("bad.tl"), "local x = (\n");
    assert!(h.declarations_of(&root.join("bad.tl")).unwrap().is_none());
    write(&root.join("empty.tl"), "print(1)\n");
    assert_eq!(
        h.declarations_of(&root.join("empty.tl"))
            .unwrap()
            .map(|d| d.len()),
        Some(0)
    );
    assert!(h.declarations_of(&root.join("gone.tl")).unwrap().is_none());
}

/// `line:col-end name call? -> file:line:col` for each use of `text`, but the standard
/// library's.
fn listed(c: &htl_core::CheckInfo, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    c.types
        .uses(text)
        .into_iter()
        .filter(|u| !u.declared.file.ends_with("stdlib.d.tl"))
        .map(|u| {
            format!(
                "{}:{}-{} {}{} -> {}:{}:{}",
                u.line,
                u.col,
                u.end,
                &lines[u.line - 1][u.col - 1..u.end - 1],
                if u.call { " call" } else { "" },
                u.declared.file.file_name().unwrap().to_str().unwrap(),
                u.declared.line,
                u.declared.col
            )
        })
        .collect()
}

const API: &str = "local record m
   name: string
   hello: function(string): string
end

function m.hello(who: string): string
   return \"hello \" .. who
end

function m:shout(s: string): string
   return s .. self.name
end

return m
";

const CALLER: &str = "local m = require(\"m\")
local function run(n: string): string
   local g = m.hello
   local t = { n = n }
   return m.hello(n) .. g(n) .. m.name .. t.n
end
local obj: m = { name = \"n\" }
print(run(\"a\"), obj:shout(\"s\"), m.shout(obj, \"t\")) -- run(\"c\")
local s = \"run(1)\"
";

/// Every name of a file that resolves, in the order written, with what it is declared by:
/// locals by their names, a function by the `function` that writes it (its definition's own
/// name included), a function the record's body also declares by the type written there, in
/// the other file and in this one's calls alike; a call is told from a read, and a name that
/// is only a table's key, a comment's or a string's is not a use.
#[test]
fn uses_are_every_name_that_resolves_with_where_it_is_declared_and_whether_it_is_called() {
    let root = common::tempdir("htl-core-typeindex", "uses");
    write(&root.join("m.tl"), API);
    write(&root.join("user.tl"), CALLER);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();

    let c = h.check_indexed(&root.join("m.tl")).unwrap();
    assert_eq!(
        listed(&c, API),
        [
            "1:14-15 m -> m.tl:1:14",
            // The name the body declares the function by is declared by the type written
            // after it, where the uses of the function from other files are.
            "3:4-9 hello -> m.tl:3:11",
            "6:10-11 m -> m.tl:1:14",
            // The name a header declares is a use of the function it defines, at `function`,
            // and not a call; `shout` is a method of a record that does not declare it.
            "6:12-17 hello -> m.tl:6:1",
            "6:18-21 who -> m.tl:6:18",
            "7:23-26 who -> m.tl:6:18",
            "10:10-11 m -> m.tl:1:14",
            "10:12-17 shout -> m.tl:10:1",
            "10:18-19 s -> m.tl:10:18",
            "11:11-12 s -> m.tl:10:18",
            "11:16-20 self -> m.tl:10:10",
            "14:8-9 m -> m.tl:1:14",
        ]
    );
    // `hello` is declared in the body (line 3, its type at column 11) and defined below
    // (line 6, column 1): the two places the uses may name are on the declaration.
    let tree = c.types.declarations().unwrap();
    let hello = tree[0].children.iter().find(|d| d.name == "hello").unwrap();
    assert_eq!((hello.line, hello.col, hello.body), (6, 1, Some((3, 11))));
    let shout = tree[0].children.iter().find(|d| d.name == "shout").unwrap();
    assert_eq!(shout.body, None);

    let c = h.check_indexed(&root.join("user.tl")).unwrap();
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert_eq!(
        listed(&c, CALLER),
        [
            "1:7-8 m -> user.tl:1:7",
            "2:16-19 run -> user.tl:2:1",
            "2:20-21 n -> user.tl:2:20",
            "3:10-11 g -> user.tl:3:10",
            "3:14-15 m -> user.tl:1:7",
            // Read, not called; through the body's declaration.
            "3:16-21 hello -> m.tl:3:11",
            // `n = n`: the key names a field, the value is the parameter.
            "4:10-11 t -> user.tl:4:10",
            "4:20-21 n -> user.tl:2:20",
            "5:11-12 m -> user.tl:1:7",
            "5:13-18 hello call -> m.tl:3:11",
            "5:19-20 n -> user.tl:2:20",
            // A call through a local that holds the function is a call of the local.
            "5:25-26 g call -> user.tl:3:10",
            "5:27-28 n -> user.tl:2:20",
            // `name` and `t.n` are fields of primitive types: nothing to point at.
            "5:33-34 m -> user.tl:1:7",
            "5:43-44 t -> user.tl:4:10",
            "7:7-10 obj -> user.tl:7:7",
            "7:12-13 m -> user.tl:1:7",
            // Not the `run("c")` of the comment, nor the `run(1)` of the string.
            "8:7-10 run call -> user.tl:2:1",
            "8:17-20 obj -> user.tl:7:7",
            // A method call `obj:shout(..)` and the same function through the field.
            "8:21-26 shout call -> m.tl:10:1",
            "8:33-34 m -> user.tl:1:7",
            "8:35-40 shout call -> m.tl:10:1",
            "8:41-44 obj -> user.tl:7:7",
            "9:7-8 s -> user.tl:9:7",
        ]
    );
    // Every use is what a question about its position says, and the standard library's
    // `print` and `require` are among the uses with the places of a file that is not one.
    for u in c.types.uses(CALLER) {
        let at = c.type_at(CALLER, u.line, u.col).unwrap();
        assert_eq!(at.declared.as_ref(), Some(&u.declared));
        assert_eq!((at.col, at.end), (u.col, u.end));
    }
    let std: Vec<_> = c
        .types
        .uses(CALLER)
        .into_iter()
        .filter(|u| u.declared.file.ends_with("stdlib.d.tl"))
        .map(|u| (u.line, u.call))
        .collect();
    assert_eq!(std, [(1, true), (8, true)]);

    // The index of a check that was not asked to keep one has none.
    let plain = h.check_written(&root.join("user.tl")).unwrap();
    assert!(plain.types.uses(CALLER).is_empty());
}

/// Asking a position of the name a header declares is answered with the function it
/// defines: the declaration is its own.
#[test]
fn the_name_a_header_declares_is_the_function_it_defines() {
    let root = common::tempdir("htl-core-typeindex", "header-name");
    write(&root.join("m.tl"), API);
    let h = Htl::new().unwrap();
    h.add_path(&root).unwrap();
    let c = h.check_indexed(&root.join("m.tl")).unwrap();
    let t = at(&c, API, "hello(who", 0).unwrap();
    assert_eq!((t.line, t.col, t.end), (6, 12, 17));
    assert_eq!(t.declared.map(|d| (d.line, d.col)), Some((6, 1)));
    // The name the body declares it by is the body's type, and a field with no definition is
    // not given one.
    let t = at(&c, API, "hello:", 0).unwrap();
    assert_eq!(t.declared.map(|d| (d.line, d.col)), Some((3, 11)));
    assert_eq!(at(&c, API, "name:", 0), None);
    // A local function is declared by its own `local`, as before.
    let local = "local function f(): integer\n   return 1\nend\nreturn f()\n";
    write(&root.join("l.tl"), local);
    let c = h.check_indexed(&root.join("l.tl")).unwrap();
    let t = at(&c, local, "f()", 0).unwrap();
    assert_eq!(t.declared.map(|d| (d.line, d.col)), Some((1, 1)));
}

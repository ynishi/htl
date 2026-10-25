//! The client of an exported module (`[rust] exports`): the struct over an
//! `htl::run::Instance`, one async method per function the module's record declares.
//!
//! What a client is, what it is made of and what refuses one is the module doc of
//! [`crate::rsgen`] (Exports); the types of its arguments and results are the mapping's, so
//! this is the other side of the same lowering and not a second one: a client is built on the
//! [`Lowering`] of its module, after every type of the project is lowered.

use super::{
    Diagnostic, Hold, Lowering, Out, RULE, Reached, Refs, Severity, Unit, children_of, element,
    pascal, rust_ident,
};
use crate::shapes::record_fields;
use crate::typeindex::{Decl, DeclKind, TypeExpr, TypeInfo};
use std::collections::{HashMap, HashSet};

/// The most values mlua converts as one tuple (`IntoLuaMulti` / `FromLuaMulti`): the
/// arguments of a call, and its results, are each one tuple.
const MAX_TUPLE: usize = 16;

/// An exported module: the module, and the record it returns.
pub(super) struct Export<'a> {
    pub(super) unit: Unit<'a>,
    record: &'a Decl,
}

/// What lowering a client made. The text is discarded when a problem was said.
pub(super) struct Made {
    pub(super) text: String,
    pub(super) problems: Vec<Diagnostic>,
    pub(super) notes: Vec<Diagnostic>,
}

/// The records that hold an `any`, directly or through other records: the ones a method
/// may neither take nor return, because a Lua value does not leave the VM thread.
pub(super) fn holding_lua_values(holds: &[Hold]) -> HashSet<(String, String)> {
    let mut set: HashSet<(String, String)> = holds
        .iter()
        .filter(|h| h.reached.iter().any(|r| matches!(r, Reached::Any)))
        .map(|h| h.from.clone())
        .collect();
    loop {
        let before = set.len();
        for h in holds {
            let through = h
                .reached
                .iter()
                .any(|r| matches!(r, Reached::Record { key, .. } if set.contains(key)));
            if through {
                set.insert(h.from.clone());
            }
        }
        if set.len() == before {
            return set;
        }
    }
}

impl<'a> Export<'a> {
    /// The export of `unit`, or the unit back when its module returns no record.
    pub(super) fn new(unit: Unit<'a>) -> Result<Self, Unit<'a>> {
        let info = unit.src.info;
        let record = info.types.declarations().and_then(|decls| {
            decls
                .iter()
                .find(|d| matches!(d.info, Some(TypeInfo::Record { returned: true, .. })))
        });
        match record {
            Some(record) => Ok(Export { unit, record }),
            None => Err(unit),
        }
    }

    /// The client, and what is said of it. `tree` is every module that has a `pub mod`, `lua`
    /// the records [`holding_lua_values`] found.
    pub(super) fn lower(
        &self,
        units: &[Unit<'a>],
        refs: &Refs,
        overrides: &HashMap<(String, String), String>,
        tree: &[&[&'a str]],
        lua: &HashSet<(String, String)>,
    ) -> Made {
        let unit = &self.unit;
        let record = self.record;
        let mut l = Lowering {
            unit,
            units,
            refs,
            overrides,
            children: children_of(&unit.segs, tree),
            cur: String::new(),
            problems: Vec::new(),
            edges: Vec::new(),
            reached: Vec::new(),
            holds: Vec::new(),
        };
        // The types the module's `pub mod` holds when it is listed too.
        let taken: Vec<&str> = units
            .iter()
            .filter(|u| u.src.module == unit.src.module)
            .flat_map(|u| u.cands.iter().map(|c| c.name.as_str()))
            .collect();
        let (text, functions) = l.client(record, &taken, lua);
        let mut notes = Vec::new();
        if functions == 0 {
            notes.push(Diagnostic::new(
                Severity::Warning,
                unit.rel.clone(),
                record.name_line,
                record.name_col,
                format!(
                    "`{}` is listed in [rust] exports and its record declares no function in its body",
                    unit.src.module
                ),
                Some(RULE),
            ));
        }
        for f in &record.children {
            // A function written below the body, with no `f: function(..)` in it: the index
            // has the definition and no type for it.
            if f.kind == DeclKind::Function && !matches!(f.info, Some(TypeInfo::Field { .. })) {
                let short = f.name.rsplit('.').next().unwrap_or(&f.name);
                notes.push(Diagnostic::new(
                    Severity::Warning,
                    unit.rel.clone(),
                    f.line,
                    f.col,
                    format!(
                        "`{}.{short}` is defined outside the body of the record, so it is not in the client; declare `{short}: function(..)` in the body to add it",
                        record.name
                    ),
                    Some(RULE),
                ));
            }
        }
        Made {
            text,
            problems: l.problems,
            notes,
        }
    }
}

impl Lowering<'_, '_> {
    /// The client of an exported module whose returned record is `record`, and how many
    /// functions the record's body declares (a refused one counted: the client has no method
    /// for it, but the body is not empty). `taken` is the names of the types the module's
    /// `pub mod` holds besides.
    fn client(
        &mut self,
        record: &Decl,
        taken: &[&str],
        lua: &HashSet<(String, String)>,
    ) -> (String, usize) {
        let module = self.unit.src.module;
        let name = pascal(&record.name);
        let (line, col) = (record.name_line, record.name_col);
        if rust_ident(&name).is_none_or(|n| n != name) {
            let message = format!(
                "`{}` cannot be PascalCased into the name of a Rust type for its client",
                record.name
            );
            self.problem(line, col, message);
        } else if taken.contains(&name.as_str()) || self.children.contains(name.as_str()) {
            let message = format!(
                "`{}` makes the client `{name}`, which is the name of a type or a module of `{module}` too; rename one",
                record.name
            );
            self.problem(line, col, message);
        }
        let mut methods = Vec::new();
        let mut declared = 0;
        for (f, ty) in record_fields(record) {
            // An overload (two function types for one field) is a field whose type is the
            // text Teal prints for it.
            let function = matches!(f.kind, DeclKind::Function | DeclKind::Method)
                || matches!(ty, TypeExpr::Fn(_))
                || matches!(ty, TypeExpr::Other(t) if t.starts_with("polymorphic function"));
            if function {
                declared += 1;
                let site = format!("{}.{}", record.name, f.name);
                methods.extend(self.method(f, ty, &site, lua));
            }
        }
        let mut s = format!(
            "/// The functions of the Teal module `{module}`, called on a runner instance; it uses `htl::run`, so the host builds `htl` with its `async` feature.
#[derive(Debug, Clone)]
pub struct {name} {{
    inst: ::htl::run::Instance,
}}

impl {name} {{
    /// `require(\"{module}\")` in a fresh instance of `runner`.
    pub async fn load(
        runner: &::htl::run::Runner,
        extra_paths: &[::std::path::PathBuf],
    ) -> ::core::result::Result<Self, ::htl::run::CallError> {{
        let inst = runner.instance(\"{module}\", extra_paths).await?;
        ::core::result::Result::Ok(Self {{ inst }})
    }}

    /// The instance the functions are called on.
    pub fn instance(&self) -> &::htl::run::Instance {{
        &self.inst
    }}
"
        );
        for m in &methods {
            s.push('\n');
            s.push_str(&super::indent(m, 1));
            s.push('\n');
        }
        s.push('}');
        (s, declared)
    }

    /// The method of the client that calls the function `f` of type `ty`; `None` once a
    /// problem has been said. Every problem is said at `f`.
    fn method(
        &mut self,
        f: &Decl,
        ty: &TypeExpr,
        site: &str,
        lua: &HashSet<(String, String)>,
    ) -> Option<String> {
        let (line, col) = (f.line, f.col);
        let TypeExpr::Fn(sig) = ty else {
            let written = f.detail.as_deref().unwrap_or("this type");
            let message = format!(
                "`{site}`: `{written}` is not one function type (an overload, or an alias of one); it has no Rust signature"
            );
            self.problem(line, col, message);
            return None;
        };
        let mut said = Vec::new();
        if sig.method {
            said.push(format!(
                "`{site}` is a method, called with `:`; a client calls functions with `.`"
            ));
        }
        if !sig.type_params.is_empty() {
            said.push(format!(
                "`{site}` is generic (`<{}>`); a type variable has no Rust type",
                sig.type_params.join(", ")
            ));
        }
        if sig.params.rest.is_some() {
            said.push(format!(
                "`{site}` takes a variadic `...`, which has no Rust type"
            ));
        }
        if sig.results.rest.is_some() {
            said.push(format!(
                "`{site}` returns a variadic `...`, which has no Rust type"
            ));
        }
        // The arguments cross as one tuple and the results come back as one, and mlua
        // converts a tuple of at most `MAX_TUPLE` values.
        for (what, n) in [
            ("takes", sig.params.items.len()),
            ("returns", sig.results.items.len()),
        ] {
            if n > MAX_TUPLE {
                said.push(format!(
                    "`{site}` {what} {n} values; a call crosses with at most {MAX_TUPLE} each way, the most a Rust tuple mlua converts holds"
                ));
            }
        }
        let name = rust_ident(&f.name);
        match name.as_deref() {
            None => said.push(format!(
                "`{site}`: `{}` cannot be the name of a Rust method",
                f.name
            )),
            Some(n @ ("load" | "instance")) => said.push(format!(
                "`{site}`: `{n}` is the name of a method the client writes itself (`load`, `instance`); rename the function"
            )),
            // The client derives `Clone`: an inherent method of either name would win over
            // the trait's in method-call syntax, and `app.clone()` would call Teal.
            Some(n @ ("clone" | "clone_from")) => said.push(format!(
                "`{site}`: `{n}` would hide the client's `Clone::{n}`; rename the function"
            )),
            Some(_) => {}
        }
        let failed = !said.is_empty();
        for message in said {
            self.problem(line, col, message);
        }
        // A method's `self` and a generic's type variables would only repeat the problem.
        if sig.method || !sig.type_params.is_empty() {
            return None;
        }
        // A parameter is `T`, an `Option<T>` only when Teal types it `T | nil`: the index
        // says how many arguments a call has to pass and not which ones, so an optional
        // parameter (`x?: T`) is not told from a required one, and the host passes a value.
        let mut params = Vec::new();
        for (i, p) in sig.params.items.iter().enumerate() {
            let at = format!("{site}, argument {}", i + 1);
            params.push(self.crossing(p, f, &at, lua).map(element));
        }
        let mut results = Vec::new();
        for (i, r) in sig.results.items.iter().enumerate() {
            let at = format!("{site}, result {}", i + 1);
            results.push(self.crossing(r, f, &at, lua).map(element));
        }
        let (Some(name), false) = (name, failed) else {
            return None;
        };
        let params: Vec<String> = params.into_iter().collect::<Option<_>>()?;
        let mut results: Vec<String> = results.into_iter().collect::<Option<_>>()?;
        let ret = match results.len() {
            0 => "()".to_string(),
            1 => results.remove(0),
            _ => format!("({})", results.join(", ")),
        };
        let names: Vec<String> = (1..=params.len()).map(|i| format!("a{i}")).collect();
        let args = match names.as_slice() {
            [] => "()".to_string(),
            [one] => format!("({one},)"),
            many => format!("({})", many.join(", ")),
        };
        let doc = match &f.detail {
            Some(d) => format!("/// `{site}`, typed `{d}` in Teal.\n"),
            None => format!("/// `{site}`.\n"),
        };
        let head = if params.is_empty() {
            format!("pub async fn {name}(&self)")
        } else {
            let mut h = format!("pub async fn {name}(\n    &self,\n");
            for (n, t) in names.iter().zip(&params) {
                h.push_str(&format!("    {n}: {t},\n"));
            }
            h.push(')');
            h
        };
        Some(format!(
            "{doc}{head} -> ::core::result::Result<{ret}, ::htl::run::CallError> {{\n    self.inst.call(\"{}\", {args}).await\n}}",
            f.name
        ))
    }

    /// The Rust type of an argument or a result of a client's method: the mapping's, with
    /// one rule more. A Lua value does not leave the VM thread, where `htl::run` keeps every
    /// Lua value, so a type that is or holds an `any` (also inside a `{T}`, a `{K : V}`, or
    /// through a record of `lua`) is a problem.
    fn crossing(
        &mut self,
        ty: &TypeExpr,
        at: &Decl,
        site: &str,
        lua: &HashSet<(String, String)>,
    ) -> Option<Out> {
        self.reached.clear();
        let out = self.lower_ty(ty, at, site, &mut Vec::new(), true)?;
        let held = self.reached.iter().find_map(|r| match r {
            Reached::Any => Some(format!(
                "`{site}`: `any` is a Lua value, which does not leave the VM thread; give it a type"
            )),
            Reached::Record { key, written } if lua.contains(key) => Some(format!(
                "`{site}`: `{written}` holds an `any`, directly or through other records, which is a Lua value that does not leave the VM thread; give that field a type, or a Rust one under [rust.types]"
            )),
            Reached::Record { .. } => None,
        });
        if let Some(message) = held {
            self.problem(at.line, at.col, message);
            return None;
        }
        Some(out)
    }
}

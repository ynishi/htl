//! Syntactic Rust type -> Teal type mapping (used at macro expansion time).

use syn::{GenericArgument, PathArguments, Type};

/// Map a Rust type to a Teal type name. `self_name` replaces `Self`.
/// `Option<T>` maps to `T` (every Teal type admits nil); `Result<T, _>` maps to `T`.
pub fn teal_type(ty: &Type, self_name: &str) -> Result<String, String> {
    match ty {
        Type::Reference(r) => teal_type(&r.elem, self_name),
        Type::Paren(p) => teal_type(&p.elem, self_name),
        Type::Tuple(t) if t.elems.is_empty() => Ok(String::new()),
        Type::Tuple(t) => {
            let parts: Result<Vec<_>, _> = t.elems.iter().map(|e| teal_type(e, self_name)).collect();
            Ok(parts?.join(", "))
        }
        Type::Slice(s) => Ok(format!("{{{}}}", teal_type(&s.elem, self_name)?)),
        Type::Array(a) => Ok(format!("{{{}}}", teal_type(&a.elem, self_name)?)),
        Type::Path(p) => {
            let seg = p.path.segments.last().ok_or("empty type path")?;
            let ident = seg.ident.to_string();
            let args: Vec<&Type> = match &seg.arguments {
                PathArguments::AngleBracketed(ab) => ab
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let arg = |i: usize| -> Result<String, String> {
                args.get(i)
                    .ok_or_else(|| format!("{ident}: missing type argument {i}"))
                    .and_then(|t| teal_type(t, self_name))
            };
            Ok(match ident.as_str() {
                "f32" | "f64" => "number".into(),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
                | "u128" | "usize" => "integer".into(),
                "bool" => "boolean".into(),
                "String" | "str" => "string".into(),
                "Self" => self_name.into(),
                "Vec" | "VecDeque" | "HashSet" | "BTreeSet" => format!("{{{}}}", arg(0)?),
                "HashMap" | "BTreeMap" => format!("{{{}:{}}}", arg(0)?, arg(1)?),
                "Option" => arg(0)?,
                "Result" => arg(0)?,
                "Box" | "Rc" | "Arc" => arg(0)?,
                "Value" => "any".into(),
                "Table" => "{any:any}".into(),
                "Function" => "function".into(),
                "LuaString" => "string".into(),
                other => other.to_string(),
            })
        }
        other => Err(format!("unsupported type for Teal mapping: {}", quote::quote!(#other))),
    }
}

/// `true` if the outermost type is `Result<..>` (the wrapper must propagate the error).
pub fn is_result(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident == "Result")
            .unwrap_or(false),
        _ => false,
    }
}

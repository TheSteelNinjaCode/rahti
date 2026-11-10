//! Source contracts for local agent tooling: each `#[rpc]` as written, and
//! the TypeScript shape of the JSON it receives and returns (see `shapes`).

use crate::shapes::{Direction, Scope, Sources, Status};
use crate::{components::Rpc, route::Kind, scanner::Entry};
use quote::ToTokens;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};
use syn::{FnArg, Item, Pat, ReturnType, Type};

/// Every rpc's contract, the named types those contracts refer to, and the
/// files the types were read from.
pub struct Inventory {
    pub contracts: Value,
    /// File to `{ "Name<T>": "body" }`: the declarations a contract in that
    /// file names. A name is unique within its file, not across files.
    pub types: Value,
    /// The shapes as the macros read them, one line each:
    /// `R\tfile\trpc\ttype` for a response and `T\tfile\thead\tbody` for a
    /// declaration it names; `C\tfile\tComponent\tprops` for each component a
    /// file can call, with the props that component reads (see
    /// [`props_read`]); then, under the file `*`, `G\t*\tpath\ttype` for every
    /// struct of the crate a generated module can name, and the `T\t*\t…`
    /// declarations those use.
    pub shapes: String,
    pub read: Vec<String>,
}

pub fn inventory(
    entries: &[Entry],
    components: &[Rpc],
    routes: &str,
    templates: &[String],
) -> Inventory {
    let mut files = BTreeMap::new();
    for entry in entries
        .iter()
        .filter(|e| e.kind == Kind::Page && !e.rpcs.is_empty())
    {
        files
            .entry(entry.file.clone())
            .or_insert_with(Vec::new)
            .extend(
                entry
                    .rpcs
                    .iter()
                    .map(|name| (name.clone(), "page", entry.urls.clone(), None)),
            );
    }
    for rpc in components {
        files
            .entry(rpc.file.clone())
            .or_insert_with(Vec::new)
            .push((
                rpc.name.clone(),
                "component",
                Vec::new(),
                Some(rpc.module.clone()),
            ));
    }
    let mut contracts = Vec::new();
    let mut types = serde_json::Map::new();
    let mut lines = String::new();
    let mut sources = Sources::default();
    for (file, owners) in files {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("rahti: cannot read RPC contracts from {file}: {e}"));
        let parsed = syn::parse_file(&source)
            .unwrap_or_else(|e| panic!("rahti: cannot parse RPC contracts in {file}: {e}"));
        let mut functions = Vec::new();
        functions_in(&parsed.items, &mut functions);
        let mut shapes = Scope::new(&mut sources, &file);
        for (name, scope, urls, module) in owners {
            let function = functions
                .iter()
                .find(|f| f.sig.ident == name)
                .unwrap_or_else(|| panic!("rahti: RPC contract `{name}` is missing from {file}"));
            let mut contract = describe(function, &mut shapes);
            contract["scope"] = json!(scope);
            contract["file"] = json!(file);
            contract["urls"] = json!(urls);
            contract["module"] = json!(module);
            if let Some(response) = contract["response"]["typescript"].as_str() {
                lines.push_str(&format!("R\t{file}\t{name}\t{response}\n"));
            }
            contracts.push(contract);
        }
        if !shapes.types.is_empty() {
            types.insert(file.clone(), json!(shapes.types));
        }
        for (head, body) in &shapes.types {
            lines.push_str(&format!("T\t{file}\t{head}\t{body}\n"));
        }
    }
    // Each component a template's file can call, and the props it reads: a
    // `{…}` prop the component never reads is refused at the tag.
    let mut read_by: BTreeMap<std::path::PathBuf, Option<String>> = BTreeMap::new();
    for file in entries
        .iter()
        .filter(|entry| entry.kind != Kind::Route)
        .map(|entry| &entry.file)
        .chain(templates)
    {
        let Some(parsed) = fs::read_to_string(file)
            .ok()
            .and_then(|source| syn::parse_file(&source).ok())
        else {
            continue;
        };
        let mut scope = Scope::new(&mut sources, file);
        for name in callable(&parsed) {
            let Some(declared) = scope.component_file(&name) else {
                continue;
            };
            let props = read_by.entry(declared.clone()).or_insert_with(|| {
                fs::read_to_string(&declared)
                    .ok()
                    .and_then(|source| props_read(&source))
                    .map(|props| props.into_iter().collect::<Vec<_>>().join(" "))
            });
            // `*`: the component may read any prop.
            let props = props.as_deref().unwrap_or("*");
            lines.push_str(&format!("C\t{file}\t{name}\t{props}\n"));
        }
    }

    // Every struct the module generated beside the routes can name, with its
    // JSON: what `rahti::shapes!` implements `rahti::shape::Shape` for, and
    // what `html!` checks a server value's reads against.
    let mut crate_wide = Scope::new(&mut sources, routes);
    if crate_wide.has_routes() {
        for (path, shape) in crate_wide.reachable() {
            lines.push_str(&format!("G\t*\t{path}\t{shape}\n"));
        }
        for (head, body) in &crate_wide.types {
            lines.push_str(&format!("T\t*\t{head}\t{body}\n"));
        }
    }
    Inventory {
        contracts: Value::Array(contracts),
        types: Value::Object(types),
        shapes: lines,
        read: sources
            .read
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect(),
    }
}

/// The names a template in this file may call a component by: each
/// `#[component]` function the file declares at its top level, and each
/// capitalised name a top-level `use` brings in.
pub(crate) fn callable(file: &syn::File) -> Vec<String> {
    fn imported(tree: &syn::UseTree, out: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => imported(&path.tree, out),
            syn::UseTree::Name(name) => out.push(name.ident.to_string()),
            syn::UseTree::Rename(rename) => out.push(rename.rename.to_string()),
            syn::UseTree::Group(group) => group.items.iter().for_each(|tree| imported(tree, out)),
            syn::UseTree::Glob(_) => {}
        }
    }
    let mut names = Vec::new();
    for item in &file.items {
        match item {
            Item::Fn(function)
                if function
                    .attrs
                    .iter()
                    .any(|attr| attr.path().is_ident("component")) =>
            {
                names.push(function.sig.ident.to_string());
            }
            Item::Use(item) => imported(&item.tree, &mut names),
            _ => {}
        }
    }
    names.retain(|name| name.starts_with(|c: char| c.is_ascii_uppercase()));
    names.sort();
    names.dedup();
    names
}

/// The props the components of one file read, as `pp.props.name`, or `None`
/// when a prop could be read some way this cannot see.
///
/// The file is read as text, since the reads are in templates and scripts.
/// It errs towards `None`: `pp.props` used whole, the component's `bindings`
/// forwarded to another component or used for anything but the declaration
/// and a spread, all mean a prop may be read somewhere else.
pub(crate) fn props_read(source: &str) -> Option<std::collections::BTreeSet<String>> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let mut names = std::collections::BTreeSet::new();

    let mut rest = source;
    while let Some(at) = rest.find("pp.props") {
        // `app.props` and `pp.propsList` are other names.
        let before = rest[..at].chars().next_back();
        let after = &rest[at + "pp.props".len()..];
        rest = after;
        if before.is_some_and(word) || after.starts_with(word) {
            continue;
        }
        let member = after.strip_prefix("?.").or_else(|| after.strip_prefix('.'));
        let name: String = member
            .map(|member| member.chars().take_while(|c| word(*c)).collect())
            .unwrap_or_default();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        names.insert(name);
    }

    let mut rest = source;
    let mut offset = 0;
    while let Some(at) = rest.find("bindings") {
        let start = offset + at;
        let end = start + "bindings".len();
        offset = end;
        rest = &source[end..];
        let before = source[..start].trim_end();
        let after = source[end..].trim_start();
        if source[..start].chars().next_back().is_some_and(word) || source[end..].starts_with(word)
        {
            continue;
        }
        // The parameter's declaration.
        if after.starts_with(':') && !after.starts_with("::") {
            continue;
        }
        // `@{..bindings}`: a spread, which has to be on a plain element.
        if before.ends_with("..") && after.starts_with('}') {
            let tag = source[..start].rfind('<').map(|open| &source[open + 1..]);
            if tag.is_some_and(|tag| tag.starts_with(|c: char| c.is_ascii_lowercase())) {
                continue;
            }
        }
        return None;
    }
    Some(names)
}

fn functions_in<'a>(items: &'a [Item], out: &mut Vec<&'a syn::ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) if f.attrs.iter().any(|a| a.path().is_ident("rpc")) => out.push(f),
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    functions_in(items, out);
                }
            }
            _ => {}
        }
    }
}

fn type_name(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

fn outer(ty: &Type) -> Option<&syn::PathSegment> {
    if let Type::Path(path) = ty {
        path.path.segments.last()
    } else {
        None
    }
}

fn inner<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let segment = outer(ty)?;
    if segment.ident != wrapper {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|arg| {
        if let syn::GenericArgument::Type(ty) = arg {
            Some(ty)
        } else {
            None
        }
    })
}

fn describe(function: &syn::ItemFn, shapes: &mut Scope) -> Value {
    let signature = &function.sig;
    let mut payload = Vec::new();
    let mut payload_resolved = true;
    let parameters: Vec<_> = signature
        .inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(arg) = arg else {
                return None;
            };
            let Pat::Ident(name) = &*arg.pat else {
                return None;
            };
            let ty = &*arg.ty;
            let optional = inner(ty, "Option");
            let files =
                inner(ty, "Vec").is_some_and(|t| outer(t).is_some_and(|s| s.ident == "RpcFile"));
            let value_type = optional.unwrap_or(ty);
            let transport = if outer(value_type).is_some_and(|s| s.ident == "RpcUpload") {
                "streaming-file"
            } else if files {
                "files"
            } else if outer(value_type).is_some_and(|s| s.ident == "RpcFile") {
                "file"
            } else {
                "json"
            };
            let required = optional.is_none() && !files;
            let shape = shapes.shape(ty, Direction::Receive);
            payload.push(format!(
                "{}{}: {}",
                name.ident,
                if required { "" } else { "?" },
                shape.typescript
            ));
            payload_resolved &= shape.status == Status::Resolved;
            Some(json!({
                "name": name.ident.to_string(), "rustType": type_name(ty),
                "required": required,
                "transport": transport, "schemaStatus": shape.status.label(),
                "typescript": shape.typescript,
            }))
        })
        .collect();
    let output: Type = match &signature.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };
    let fallible = inner(&output, "Result");
    let success = fallible.unwrap_or(&output);
    let streaming = outer(success).is_some_and(|s| s.ident == "RpcStream");
    // A stream's events are whatever the handler sends: nothing declares them.
    let (response_status, response_type) = if streaming {
        ("stream", Value::Null)
    } else {
        let shape = shapes.shape(success, Direction::Send);
        (shape.status.label(), json!(shape.typescript))
    };
    let payload = if payload.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", payload.join("; "))
    };
    json!({
        "name": signature.ident.to_string(),
        "line": signature.ident.span().start().line,
        "column": signature.ident.span().start().column + 1,
        "authRequired": function.attrs.iter().any(|a| a.path().is_ident("rpc") && a.meta.to_token_stream().to_string().replace(' ', "") == "rpc(auth)"),
        "parameters": parameters,
        "payload": { "typescript": payload,
            "schemaStatus": if payload_resolved { "resolved" } else { "partial" } },
        "response": { "rustType": type_name(success), "fallible": fallible.is_some(),
            "transport": if streaming { "stream" } else { "json" },
            "schemaStatus": response_status, "typescript": response_type },
        "checks": { "name": "compile-time", "literalKeys": "compile-time",
            "literalValueShapes": "conservative", "dynamicValues": "server",
            "responseUsage": "compile-time for fields read off `const x = await pp.rpc(…)`" },
        "call": format!("pp.rpc(\"{}\", {{ … }})", signature.ident),
    })
}

#[cfg(test)]
#[path = "tests/contracts.rs"]
mod tests;

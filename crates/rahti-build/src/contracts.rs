//! Source contracts for local agent tooling. Rust spellings are not JSON schemas.

use crate::{components::Rpc, route::Kind, scanner::Entry};
use quote::ToTokens;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};
use syn::{FnArg, Item, Pat, ReturnType, Type};

pub fn inventory(entries: &[Entry], components: &[Rpc]) -> Value {
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
    for (file, owners) in files {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("rahti: cannot read RPC contracts from {file}: {e}"));
        let parsed = syn::parse_file(&source)
            .unwrap_or_else(|e| panic!("rahti: cannot parse RPC contracts in {file}: {e}"));
        let mut functions = Vec::new();
        functions_in(&parsed.items, &mut functions);
        for (name, scope, urls, module) in owners {
            let function = functions
                .iter()
                .find(|f| f.sig.ident == name)
                .unwrap_or_else(|| panic!("rahti: RPC contract `{name}` is missing from {file}"));
            let mut contract = describe(function);
            contract["scope"] = json!(scope);
            contract["file"] = json!(file);
            contract["urls"] = json!(urls);
            contract["module"] = json!(module);
            contracts.push(contract);
        }
    }
    Value::Array(contracts)
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

fn describe(function: &syn::ItemFn) -> Value {
    let signature = &function.sig;
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
            Some(json!({
                "name": name.ident.to_string(), "rustType": type_name(ty),
                "required": optional.is_none() && !files,
                "transport": transport, "schemaStatus": "unresolved",
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
    json!({
        "name": signature.ident.to_string(),
        "line": signature.ident.span().start().line,
        "column": signature.ident.span().start().column + 1,
        "authRequired": function.attrs.iter().any(|a| a.path().is_ident("rpc") && a.meta.to_token_stream().to_string().replace(' ', "") == "rpc(auth)"),
        "parameters": parameters,
        "response": { "rustType": type_name(success), "fallible": fallible.is_some(),
            "transport": if streaming { "stream" } else { "json" }, "schemaStatus": "unresolved" },
        "checks": { "name": "compile-time", "literalKeys": "compile-time",
            "literalValueShapes": "conservative", "dynamicValues": "server", "responseUsage": "unchecked" },
        "call": format!("pp.rpc(\"{}\", {{ … }})", signature.ident),
    })
}

#[cfg(test)]
#[path = "tests/contracts.rs"]
mod tests;

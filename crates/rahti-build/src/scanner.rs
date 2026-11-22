//! Phase 1: walk `app/` and turn it into a validated list of routes.

use std::fs;
use std::path::Path;

use crate::route::{self, Kind, Segment};

#[cfg(test)]
#[path = "tests/scanner.rs"]
mod tests;

/// Methods a `route.rs` may export, in the order they get chained.
pub const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

/// How a `layout.rs` below the app root declares that it owns a document of
/// its own — the second, third, nth root layout of an application.
///
/// Written on a line of its own, which is where rustfmt puts it.
pub const ROOT_MARKER: &str = "pub const ROOT_LAYOUT: bool = true;";

/// One browser-callable function found beneath a Rahti attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callable {
    pub name: String,
    /// `true` for `#[rpc(auth)]` or `#[socket(auth)]`.
    pub auth: bool,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: Kind,
    /// Path from the crate root, forward slashes: `src/app/customers/[id]/page.rs`.
    pub file: String,
    /// Directory chain below `app/`, unparsed: `["customers", "[id]"]`.
    pub dirs: Vec<String>,
    /// URLs this file answers. Empty for layout/error/not-found.
    pub urls: Vec<String>,
    /// Module identifier in the generated file.
    pub module: String,
    /// `pp-component` name the `html!` macro stamps on this file's root
    /// element. `None` for `route.rs`, which renders no markup.
    pub component: Option<String>,
    /// For `route.rs`: the methods actually exported.
    pub methods: Vec<String>,
    /// For `page.rs`: the `#[rpc]` functions the browser may call, in the
    /// order they are written. Each one gets an arm in the dispatcher this
    /// page's URL is given.
    pub rpcs: Vec<String>,
    /// The subset of [`Self::rpcs`] that refuse a call without a session.
    pub auth_rpcs: Vec<String>,
    /// For `page.rs`: the `#[socket]` functions the browser may connect to,
    /// in the order they are written. Unlike an rpc these get no per-page
    /// dispatcher — every connection arrives on the one socket endpoint and
    /// is dispatched by name from the link-time registry — so what the scan
    /// contributes is the merge of that endpoint, and the collision check.
    pub sockets: Vec<String>,
    /// The subset of [`Self::sockets`] that refuse the handshake without a
    /// session.
    pub auth_sockets: Vec<String>,
    /// This page, layout, or root not-found file exports
    /// `pub fn metadata() -> Metadata`.
    pub metadata: bool,
    /// This dynamic page exports `pub async fn static_paths()`.
    pub static_paths: bool,
    /// This page exports `pub fn cache() -> PageCache`.
    pub page_cache: bool,
    /// For `layout.rs`: this layout owns a document of its own, so no layout
    /// above it wraps the pages below it. Declared with [`ROOT_MARKER`], and
    /// implicit for `app/layout.rs`. Always `false` for every other kind.
    pub root: bool,
}

/// The result of a scan, plus every path cargo must watch to know the scan
/// is stale.
pub struct Scan {
    pub entries: Vec<Entry>,
    /// Every directory walked and every convention file seen.
    ///
    /// Watching only the app root is not enough: cargo compares a directory's
    /// own mtime, and deleting `app/(marketing)/x/` bumps `(marketing)` while
    /// leaving `app` untouched. The build would not rerun and the generated
    /// router would keep a module pointing at a deleted file.
    pub watched: Vec<String>,
}

pub fn scan(app_dir: &Path) -> Scan {
    if !app_dir.is_dir() {
        panic!("rahti: app directory not found at {}", app_dir.display());
    }

    let mut found = Vec::new();
    let mut watched = vec![normalize(app_dir)];
    walk(app_dir, &mut Vec::new(), &mut found, &mut watched);

    // Deterministic output regardless of how the filesystem enumerates.
    found.sort_by(|a, b| a.0.cmp(&b.0));

    let mut used_modules = Vec::new();
    let mut entries = Vec::new();
    // What the macro hashes is the path below the app directory, so strip
    // the same prefix here.
    let app_prefix = format!("{}/", normalize(app_dir));

    for (file, dirs, kind) in found {
        let urls = if kind.is_routable() {
            route::to_urls(&dirs)
        } else {
            Vec::new()
        };
        let methods = if kind == Kind::Route {
            exported_methods(&file)
        } else {
            Vec::new()
        };
        let rpcs = page_rpc_functions(&file, kind);
        let auth_rpcs = rpcs
            .iter()
            .filter(|rpc| rpc.auth)
            .map(|rpc| rpc.name.clone())
            .collect();
        let rpcs = rpcs.into_iter().map(|rpc| rpc.name).collect();
        let sockets = page_socket_functions(&file, kind);
        let auth_sockets = sockets
            .iter()
            .filter(|socket| socket.auth)
            .map(|socket| socket.name.clone())
            .collect();
        let sockets = sockets.into_iter().map(|socket| socket.name).collect();
        let source = fs::read_to_string(&file).unwrap_or_default();
        let metadata = exports(&file, &source, &METADATA);
        let static_paths = exports(&file, &source, &STATIC_PATHS);
        let page_cache = exports(&file, &source, &PAGE_CACHE);
        if metadata && matches!(kind, Kind::Route | Kind::Error | Kind::Loading) {
            panic!(
                "rahti: {file} exports `metadata()`, but a `{}` has no document head \
                 to put it in.\n  \
                 Move `pub fn metadata() -> Metadata` into the `page.rs` or \
                 `layout.rs` that owns the document.",
                kind.label()
            );
        }
        if static_paths && (kind != Kind::Page || !urls.iter().any(|url| url.contains('{'))) {
            panic!(
                "rahti: {file} exports `static_paths()`, but it is not a dynamic page.\n  \
                 Only a page under `[param]`, `[...param]`, or `[[...param]]` \
                 needs concrete paths for static generation."
            );
        }
        if page_cache && kind != Kind::Page {
            panic!("rahti: {file} exports `cache()`, but page caching belongs only in `page.rs`.");
        }
        match kind {
            Kind::Layout => check_exports(
                &file,
                "layout",
                "pub fn layout(children: Html) -> Html",
                "put `<slot />` where the page should render",
            ),
            Kind::Error => check_exports(
                &file,
                "error",
                "pub fn error(error: ErrorInfo) -> Html",
                "render whatever the reader should see when a page below fails",
            ),
            Kind::Loading => check_exports(
                &file,
                "loading",
                "pub fn loading() -> Html",
                "render whatever the reader should see while the next page is fetched",
            ),
            _ => {}
        }
        let module = route::module_name(kind, &dirs, &mut used_modules);
        let relative = file.strip_prefix(&app_prefix).unwrap_or(&file).to_string();
        let component = (kind != Kind::Route).then(|| {
            declared_component(&file).unwrap_or_else(|| route::component_id(kind, &relative))
        });
        // `app/layout.rs` has nothing above it, so it is a document root
        // whether or not it says so.
        let root = kind == Kind::Layout && (dirs.is_empty() || declares_root(&file));
        if root {
            check_root_slot(&file);
        }

        entries.push(Entry {
            kind,
            file,
            dirs,
            urls,
            module,
            methods,
            component,
            rpcs,
            auth_rpcs,
            sockets,
            auth_sockets,
            metadata,
            static_paths,
            page_cache,
            root,
        });
    }

    validate(&entries);
    Scan { entries, watched }
}

fn walk(
    dir: &Path,
    dirs: &mut Vec<String>,
    out: &mut Vec<(String, Vec<String>, Kind)>,
    watched: &mut Vec<String>,
) {
    let mut children = Vec::new();

    for item in
        fs::read_dir(dir).unwrap_or_else(|e| panic!("rahti: cannot read {}: {e}", dir.display()))
    {
        let path = item.expect("rahti: unreadable directory entry").path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if path.is_dir() {
            // `_folder` is private: colocated code, never routed.
            if !name.starts_with('_') {
                children.push((name, path));
            }
        } else if let Some(kind) = Kind::from_file_name(&name) {
            let file = normalize(&path);
            watched.push(file.clone());
            out.push((file, dirs.clone(), kind));
        }
    }

    children.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path) in children {
        watched.push(normalize(&path));
        dirs.push(name);
        walk(&path, dirs, out, watched);
        dirs.pop();
    }
}

/// Read a `route.rs` and note which HTTP methods it exports.
///
/// A text scan rather than a real parse: enough to wire the router, and a
/// wrong guess surfaces as an ordinary rustc error against the real file.
/// Only a `pub` function is a handler — the generated router calls it from
/// another module — so a private helper that happens to be named `get` is
/// left alone.
fn exported_methods(file: &str) -> Vec<String> {
    let src = fs::read_to_string(file).unwrap_or_default();
    let mut found = Vec::new();
    let mut private = Vec::new();
    for method in METHODS {
        let declared = declarations(&src, method);
        if declared.iter().any(Declaration::is_public) {
            found.push(method.to_string());
        } else if declared.iter().any(|declaration| declaration.top_level) {
            private.push(method);
        }
    }

    if found.is_empty() && !private.is_empty() {
        let named = |prefix: &str, suffix: &str| {
            private
                .iter()
                .map(|method| format!("`{prefix}fn {method}{suffix}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        panic!(
            "rahti: {file} exports no handlers: {} {} not `pub`.\n  \
             The generated router calls a handler from another module. Write \
             {}.",
            named("", ""),
            if private.len() == 1 { "is" } else { "are" },
            named("pub async ", "(")
        );
    }
    if found.is_empty() {
        panic!(
            "rahti: {file} exports no handlers.\n  \
             Add at least one of: {}",
            METHODS
                .iter()
                .map(|m| format!("`pub async fn {m}()`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    found
}

/// Whether a line is the attribute, in either of the two forms it takes.
///
/// `#[rpc]` and `#[rpc(auth)]` register the same way. The scanner also keeps
/// the valid `auth` spelling so the tooling manifest can describe the runtime
/// check the macro places in the shim. The macro remains the authority that
/// validates arguments against real tokens with a real span.
fn is_rpc_attribute(line: &str) -> bool {
    is_attribute(line, "#[rpc")
}

/// The same rule for `#[socket]` and `#[socket(auth)]`.
fn is_socket_attribute(line: &str) -> bool {
    is_attribute(line, "#[socket")
}

fn is_attribute(line: &str, prefix: &str) -> bool {
    let Some(rest) = line.strip_prefix(prefix) else {
        return false;
    };
    rest == "]" || (rest.starts_with('(') && rest.ends_with(")]"))
}

/// The `#[rpc]` functions in a file, in source order.
///
/// Text-scanned, like [`exported_methods`] — enough to write the dispatcher,
/// and a wrong guess surfaces as an ordinary rustc error against the real
/// file. The attribute counts only where it is written as an attribute: on a
/// line of its own, which is where rustfmt puts it and which keeps a mention
/// of `#[rpc]` in a doc comment from being read as one.
///
/// Shared with `components.rs`: an rpc reads the same on either side of the
/// tree, and only what it is reachable from differs.
pub fn rpc_callables(file: &str) -> Vec<Callable> {
    attribute_functions(file, "rpc", is_rpc_attribute)
}

/// The `#[socket]` functions in a file, in source order — the socket half of
/// [`rpc_callables`], shared with `components.rs` for the same reason.
pub fn socket_callables(file: &str) -> Vec<Callable> {
    attribute_functions(file, "socket", is_socket_attribute)
}

fn attribute_functions(file: &str, what: &str, is_attr: fn(&str) -> bool) -> Vec<Callable> {
    let src = fs::read_to_string(file).unwrap_or_default();
    let mut found: Vec<Callable> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();

    for (at, attribute) in lines.iter().enumerate().filter(|(_, l)| is_attr(l.trim())) {
        let Some(name) = lines[at + 1..].iter().find_map(|line| function_in(line)) else {
            panic!(
                "rahti: {file} has an `#[{what}]` with no function under it.\n  \
                 The attribute goes on the `fn` the browser calls."
            );
        };
        if found.iter().any(|callable| callable.name == name) {
            panic!(
                "rahti: {file} declares two {what}s named `{name}`.\n  \
                 The browser picks one by name, so the names in one file have to \
                 differ."
            );
        }
        let compact: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
        found.push(Callable {
            name,
            auth: compact == format!("#[{what}(auth)]"),
        });
    }

    found
}

/// The same, held to where the attribute means something under `app/`.
///
/// A page owns a URL, so its rpcs are reached by POSTing to it. A layout or a
/// `route.rs` owns no page for the client to be calling from, and a shim
/// written there would never be reached — worth stopping rather than leaving
/// to be discovered. A component is the other case that works, and it is
/// scanned by `components.rs`.
fn page_rpc_functions(file: &str, kind: Kind) -> Vec<Callable> {
    let found = rpc_callables(file);

    if !found.is_empty() && kind != Kind::Page {
        panic!(
            "rahti: {file} is a `{}`, and under `app/` an `#[rpc]` belongs in a \
             `page.rs`.\n  \
             The client runtime POSTs to the URL the page is on — a `{}` has no \
             page for it to be called from.\n  \
             Move the function into the `page.rs` that calls it, into a \
             `src/components/` file if a component owns it, or expose it from a \
             `route.rs` as an ordinary `pub async fn post`.",
            kind.label(),
            kind.label()
        );
    }

    found
}

/// The same placement rule for `#[socket]`: under `app/` it belongs in a
/// `page.rs`. Sockets connect to one app-wide endpoint rather than the page's
/// URL, so a socket written in a layout would in fact be *reachable* — but a
/// layout renders on many pages, and behaviour that belongs to all of them
/// belongs in a component, where the rpc rule already sends it.
fn page_socket_functions(file: &str, kind: Kind) -> Vec<Callable> {
    let found = socket_callables(file);

    if !found.is_empty() && kind != Kind::Page {
        panic!(
            "rahti: {file} is a `{}`, and under `app/` a `#[socket]` belongs in \
             a `page.rs`.\n  \
             Put the function beside the page whose script opens it, or into a \
             `src/components/` file if a component owns the conversation.",
            kind.label()
        );
    }

    found
}

/// The name of the function this line declares, if it declares one.
fn function_in(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Does this `layout.rs` declare itself a document root?
///
/// Text-scanned, like [`exported_methods`]: the marker is a plain `const`, so
/// a file that carries it also compiles with it, and a typo surfaces as an
/// ordinary layout that never took effect rather than as broken generated
/// code.
fn declares_root(file: &str) -> bool {
    fs::read_to_string(file)
        .map(|src| src.lines().any(|line| line.trim() == ROOT_MARKER))
        .unwrap_or(false)
}

/// One `fn name` found by the text scan.
struct Declaration {
    /// What stands between the previous item and `fn`, single-spaced:
    /// `pub async`, `pub(crate)`, or nothing.
    qualifiers: String,
    /// The parameter list, whitespace removed.
    params: String,
    /// What stands between the parameter list and the body, single-spaced:
    /// `-> PageCache`.
    returns: String,
    /// The item starts in column 0, which is where rustfmt puts a function
    /// that belongs to the module rather than to an `impl` or a nested `mod`.
    top_level: bool,
}

impl Declaration {
    fn is_public(&self) -> bool {
        self.qualifiers
            .split(' ')
            .next()
            .is_some_and(|first| first.starts_with("pub"))
    }

    /// The declaration as the author wrote it, for a message.
    fn written(&self, name: &str) -> String {
        let mut out = String::new();
        if !self.qualifiers.is_empty() {
            out.push_str(&self.qualifiers);
            out.push(' ');
        }
        out.push_str(&format!("fn {name}({})", self.params));
        if !self.returns.is_empty() {
            out.push(' ');
            out.push_str(&self.returns);
        }
        out
    }
}

/// Every function named `name` a file declares.
///
/// Still a text scan, but over the whole source rather than line by line, so
/// a signature rustfmt broke across lines reads the same as one it did not.
/// Comment lines are left out: a doc comment showing the function is prose.
fn declarations(src: &str, name: &str) -> Vec<Declaration> {
    let code = src
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let single_spaced = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut found = Vec::new();
    for (at, _) in code.match_indices("fn") {
        let before = &code[..at];
        if before
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
        {
            continue;
        }
        let after = &code[at + 2..];
        let unspaced = after.trim_start();
        if unspaced.len() == after.len() {
            continue;
        }
        let Some(rest) = unspaced.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start();
        if !(rest.starts_with('(') || rest.starts_with('<')) {
            continue;
        }

        // The parameter list, by matching its parentheses, and whatever
        // stands between it and the body.
        let mut params = String::new();
        let mut returns = String::new();
        if let Some(open) = rest.find('(') {
            let mut depth = 0usize;
            let mut close = None;
            for (offset, ch) in rest[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(open + offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close {
                params = rest[open + 1..close]
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                let tail = &rest[close + 1..];
                let tail = single_spaced(&tail[..tail.find(['{', ';']).unwrap_or(tail.len())]);
                returns = match tail.find(" where ") {
                    Some(at) => tail[..at].to_string(),
                    None => tail,
                };
            }
        }

        // The qualifiers are the keywords standing directly before `fn`,
        // read backwards one word at a time. Anything else — the end of the
        // previous item, an attribute, a trailing comment — ends them.
        let mut qualifiers: Vec<&str> = Vec::new();
        let mut first = at;
        let mut end = before.trim_end().len();
        loop {
            let head = &before[..end];
            let start = head
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map(|(index, c)| index + c.len_utf8())
                .unwrap_or(0);
            let word = &head[start..];
            let qualifier = word == "pub"
                || word.starts_with("pub(")
                || matches!(word, "async" | "const" | "unsafe" | "extern" | "default");
            if !qualifier {
                break;
            }
            qualifiers.push(word);
            first = start;
            end = head[..start].trim_end().len();
        }
        qualifiers.reverse();
        found.push(Declaration {
            qualifiers: qualifiers.join(" "),
            params,
            returns,
            top_level: first == 0 || code.as_bytes()[first - 1] == b'\n',
        });
    }
    found
}

/// One optional export the generated router calls by name.
struct Export {
    name: &'static str,
    /// The spelling the build reads, as it is named in a refusal.
    signature: &'static str,
    accepts: fn(&Declaration) -> bool,
    /// The return type that marks a private function of this name as an
    /// attempt at the export. `None` when the name alone says so.
    marker: Option<&'static str>,
}

/// `pub fn metadata() -> Metadata`: static metadata for a page, a layout, or
/// the root `not-found.rs`. Its return type is left to rustc, which checks
/// the call the generated router makes.
const METADATA: Export = Export {
    name: "metadata",
    signature: "pub fn metadata() -> Metadata",
    accepts: |declaration| declaration.qualifiers == "pub",
    marker: Some("Metadata"),
};

/// `pub async fn static_paths()`: the concrete URLs static export renders
/// for a dynamic page.
const STATIC_PATHS: Export = Export {
    name: "static_paths",
    signature: "pub async fn static_paths() -> Vec<String>",
    accepts: |declaration| declaration.qualifiers == "pub async",
    marker: None,
};

/// `pub fn cache() -> PageCache`: a page opting its complete anonymous
/// response into the cache.
const PAGE_CACHE: Export = Export {
    name: "cache",
    signature: "pub fn cache() -> PageCache",
    accepts: |declaration| {
        declaration.qualifiers == "pub"
            && declaration.params.is_empty()
            && declaration.returns == "-> PageCache"
    },
    marker: Some("PageCache"),
};

/// Whether a convention file supplies `export`.
///
/// The generated router calls the real function, so rustc still checks its
/// types. What rustc cannot report is a function the scan did not recognise:
/// that one is simply never called, and the page is served without its
/// metadata, its cache policy or its static paths with nothing to say why. So
/// a module-level function of the export's name in any other form is refused
/// here, against the file the author wrote.
///
/// A private function of the same name that returns something else is an
/// ordinary helper and is left alone.
fn exports(file: &str, src: &str, export: &Export) -> bool {
    let declared = declarations(src, export.name);
    if declared.iter().any(export.accepts) {
        return true;
    }
    let attempt = declared.iter().find(|declaration| {
        declaration.top_level
            && export.marker.is_none_or(|marker| {
                declaration.is_public() || declaration.returns.contains(marker)
            })
    });
    if let Some(declaration) = attempt {
        panic!(
            "rahti: {file} declares `{}`, which the build does not read as the \
             `{}` export, so it would never be called.\n  \
             Write it as `{}`.",
            declaration.written(export.name),
            export.name,
            export.signature
        );
    }
    false
}

/// Whether every dynamic segment of these URLs is the root locale segment,
/// which static export expands once per configured locale by itself.
pub fn locale_only(urls: &[String]) -> bool {
    urls.iter().all(|url| {
        let without_locale = url.replace("{locale}", "").replace("{lang}", "");
        !without_locale.contains('{')
    }) && urls.iter().any(|url| {
        url == "/{locale}"
            || url.starts_with("/{locale}/")
            || url == "/{lang}"
            || url.starts_with("/{lang}/")
    })
}

/// A locale-only page is expanded for every configured locale, so the
/// generated router never calls a `static_paths()` it exports. Refused
/// rather than left to return paths nobody reads.
///
/// Apart from [`validate`] because it needs to know whether the project
/// configured localization, which the scan of `app/` does not.
pub fn check_static_paths(entries: &[Entry], localized: bool) {
    if !localized {
        return;
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.kind == Kind::Page && entry.static_paths && locale_only(&entry.urls))
    {
        panic!(
            "rahti: {} exports `static_paths()`, but its only dynamic segment is \
             the locale, which static export expands for every configured \
             locale by itself.\n  \
             Delete `static_paths()` from this page; it would never be called.",
            entry.file
        );
    }
}

/// A root layout renders its children through `<slot />`, and nothing else.
///
/// `@{children}` compiles and serves the same markup — but the document's
/// reactive boundary, the `<template pp-component="…">` the macro injects, is
/// anchored on `<slot />`. With no slot nothing is injected, the document has
/// no region for client navigation to swap, and nothing about the served page
/// looks wrong. The macro cannot catch this — it never sees the function
/// signature — so the build does, here, where a layout is known to be a root.
///
/// Held to files that render markup at all: a pass-through layout
/// (`{ children }`) has no `html!` block, so there is nothing to anchor and
/// nothing to check. Text-scanned with comment lines left out, like
/// [`declared_component`]: a doc comment naming `<slot />` is prose.
fn check_root_slot(file: &str) {
    let src = fs::read_to_string(file).unwrap_or_default();
    let code_contains = |needle: &str| {
        src.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .any(|line| line.contains(needle))
    };
    if code_contains("html!") && !code_contains("<slot") {
        panic!(
            "rahti: {file} is a root layout with no `<slot />`.\n  \
             The `<template pp-component=\"…\">` that owns the document's reactive \
             region is injected around `<slot />`; interpolating `@{{children}}` \
             instead renders the page but ships no boundary, and client \
             navigation breaks silently.\n  \
             Put `<slot />` where the page should render."
        );
    }
}

/// Every `layout.rs` that wraps this page, outermost first.
///
/// The chain is cut at the deepest layout that owns a document: everything
/// above it belongs to a different document shell, and applying it would put
/// a second `<html>` around the first. With no such layout below the app
/// root — the ordinary case — the chain is the whole ancestry, and its
/// outermost entry is `app/layout.rs`.
///
/// Only pages inherit layouts. `route.rs` answers with JSON or a redirect,
/// and Next.js does not wrap those either.
pub fn layout_chain<'a>(entry: &Entry, entries: &'a [Entry]) -> Vec<&'a Entry> {
    if entry.kind != Kind::Page {
        return Vec::new();
    }
    let mut chain: Vec<&Entry> = entries
        .iter()
        .filter(|l| l.kind == Kind::Layout && route::covers(&l.dirs, &entry.dirs))
        .collect();
    chain.sort_by_key(|l| l.dirs.len());

    if let Some(at) = chain.iter().rposition(|l| l.root) {
        chain.drain(..at);
    }
    chain
}

/// The layout that owns this page's document, if it has one.
pub fn root_layout<'a>(entry: &Entry, entries: &'a [Entry]) -> Option<&'a Entry> {
    layout_chain(entry, entries).into_iter().next()
}

/// A `pp-component` the author wrote by hand.
///
/// `html!` skips its own injection when a block declares one, so the
/// manifest has to report the same name — otherwise it describes markup
/// that never ships. Text-scanned, like `exported_methods`, with comment
/// lines left out: a doc comment explaining the attribute is prose, not
/// markup, and the macro does not see it either.
fn declared_component(file: &str) -> Option<String> {
    let src = fs::read_to_string(file).ok()?;
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .find_map(|line| {
            let (_, rest) = line.split_once("pp-component=\"")?;
            let (name, _) = rest.split_once('"')?;
            Some(name.to_string())
        })
}

/// Convention files get wired into the router unconditionally, so an empty
/// or misnamed one would surface as an error inside generated code. Catch it
/// against the file the author actually wrote.
fn check_exports(file: &str, func: &str, signature: &str, hint: &str) {
    let src = fs::read_to_string(file).unwrap_or_default();
    if !src.contains(&format!("fn {func}(")) {
        panic!(
            "rahti: {file} exports no {func}.\n  \
             Add `{signature}`, and {hint}.\n  \
             Delete the file if this segment needs no {func}."
        );
    }
}

fn validate(entries: &[Entry]) {
    // A segment must make its public role explicit: UI or endpoint. Silently
    // choosing one would leave the other valid-looking file unreachable.
    for page in entries.iter().filter(|entry| entry.kind == Kind::Page) {
        if let Some(route) = entries
            .iter()
            .find(|entry| entry.kind == Kind::Route && entry.dirs == page.dirs)
        {
            panic!(
                "rahti: a route segment cannot define both UI and an API handler.\n  \
                 UI:  {}\n  \
                 API: {}\n  \
                 Keep `page.rs` for UI or `route.rs` for an API, or move one \
                 into a different route segment.",
                page.file, route.file
            );
        }
    }

    // A loading region is chosen in the browser, by walking the current path
    // up one literal segment at a time. `/customers/{id}` is not a path any
    // browser is ever on, so a `loading.rs` under a dynamic segment would be
    // built, shipped and never once matched. Refused here rather than left to
    // be noticed, because nothing about it looks wrong at runtime.
    for entry in entries.iter().filter(|e| e.kind == Kind::Loading) {
        if let Some(dir) = entry.dirs.iter().find(|d| {
            !matches!(
                route::parse_segment(d),
                Segment::Group | Segment::Literal(_)
            )
        }) {
            panic!(
                "rahti: {} sits under the dynamic segment `{dir}`, and a loading \
                 region is matched by literal path.\n  \
                 The browser walks the URL it is on up one segment at a time, so \
                 `{dir}` is never what it looks for.\n  \
                 Move the file to a static ancestor — the loading region there \
                 covers every URL below it, dynamic segments included.",
                entry.file
            );
        }
    }

    // Two regions for one path. The browser takes the first `pp-loading-url`
    // that matches, so the second would ship in every document and never
    // show — a route group contributes no segment, which is how two files
    // come to cover the same path.
    let mut regions: Vec<(String, &str)> = Vec::new();
    for entry in entries.iter().filter(|e| e.kind == Kind::Loading) {
        for url in route::to_urls(&entry.dirs) {
            if let Some((_, other)) = regions.iter().find(|(u, _)| *u == url) {
                panic!(
                    "rahti: two `loading.rs` files cover the same path `{url}`.\n  \
                     {other}\n  {}\n  \
                     A route group adds no URL segment, so both answer for \
                     `{url}` and only one could ever show. Keep one of them.",
                    entry.file
                );
            }
            regions.push((url, &entry.file));
        }
    }

    // The root 404 gets its document, and so its `<head>`, from the layouts
    // at the app root. With none there is nowhere to write metadata, and the
    // generated handler would never call the function.
    if let Some(not_found) = entries
        .iter()
        .find(|e| e.kind == Kind::NotFound && e.dirs.is_empty() && e.metadata)
    {
        if !entries
            .iter()
            .any(|e| e.kind == Kind::Layout && e.dirs.is_empty())
        {
            panic!(
                "rahti: {} exports `metadata()`, but there is no `layout.rs` at \
                 the app root to give the 404 a document head.\n  \
                 Add `layout.rs` beside it, or delete `metadata()`.",
                not_found.file
            );
        }
    }

    // Two files stamping the same `pp-component`. The macro derives the
    // name from the source path with no view of its siblings, so this is the
    // only place the clash can be caught — and the client runtime would be
    // unable to tell the two blocks apart.
    let mut names: Vec<(&str, &str)> = Vec::new();
    for entry in entries {
        let Some(component) = &entry.component else {
            continue;
        };
        if let Some((_, other)) = names.iter().find(|(c, _)| *c == component.as_str()) {
            panic!(
                "rahti: two files would render `pp-component=\"{component}\"`.\n  \
                 {other}\n  {}\n  Directory names that differ only in brackets or \
                 punctuation collapse to the same identifier — rename one segment.",
                entry.file
            );
        }
        names.push((component, &entry.file));
    }

    // More than one root layout means more than one document shell, and the
    // client tells them apart by the identity the root layout stamps. A page
    // with no root layout stamps nothing: navigating to it from either shell
    // would swap its markup into a document that was never built for it,
    // instead of loading the page. Held to only where a second root exists,
    // because a layout-free app is a legitimate shape on its own.
    let pages: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Page).collect();
    let mut roots: Vec<&str> = pages
        .iter()
        .filter_map(|page| root_layout(page, entries).map(|l| l.file.as_str()))
        .collect();
    roots.sort_unstable();
    roots.dedup();

    if roots.len() > 1 {
        let orphans: Vec<&str> = pages
            .iter()
            .filter(|page| root_layout(page, entries).is_none())
            .map(|page| page.file.as_str())
            .collect();
        if !orphans.is_empty() {
            panic!(
                "rahti: this app has {} root layouts, so every page needs one.\n  \
                 roots:   {}\n  \
                 without: {}\n  \
                 Move these pages under a group that has a root layout, add \
                 `app/layout.rs`, or give their segment a `layout.rs` that \
                 declares `{ROOT_MARKER}`.",
                roots.len(),
                roots.join(", "),
                orphans.join(", ")
            );
        }
    }

    // Two files answering one URL.
    let mut claimed: Vec<(&str, &str)> = Vec::new();
    for entry in entries.iter().filter(|e| e.kind.is_routable()) {
        for url in &entry.urls {
            if let Some((_, other)) = claimed.iter().find(|(u, _)| *u == url.as_str()) {
                panic!(
                    "rahti: two files resolve to the same URL `{url}`.\n  \
                     {}\n  {}\n  Route groups and optional catch-alls both \
                     claim their parent path.",
                    other, entry.file
                );
            }
            claimed.push((url, &entry.file));
        }
    }
}

/// Crate-root-relative path with forward slashes, so generated code and the
/// manifest read the same on every platform.
fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

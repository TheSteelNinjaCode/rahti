//! Phase 2: turn the scanned routes into Rust source and a manifest.

use crate::Config;
use crate::components::Rpc;
use crate::route::{self, Kind};
use crate::scanner::{Entry, layout_chain, root_layout};

#[cfg(test)]
#[path = "tests/codegen.rs"]
mod tests;

/// `#[path]` on a module in `src/routes.rs` resolves relative to `src/`,
/// so the crate-root-relative path loses its `src/` prefix.
fn module_path(file: &str) -> &str {
    file.strip_prefix("src/").unwrap_or_else(|| {
        panic!(
            "rahti: {file} lives outside `src/`, so the generated router \
             cannot reach it. Move the app directory under `src/`."
        )
    })
}

/// Routable entries, ordered static -> dynamic -> catch-all, then by URL.
/// Cosmetic only: axum ranks matches itself.
fn ordered(entries: &[Entry]) -> Vec<(&Entry, &String)> {
    let mut pairs: Vec<(&Entry, &String)> = entries
        .iter()
        .filter(|e| e.kind.is_routable())
        .flat_map(|e| e.urls.iter().map(move |u| (e, u)))
        .collect();
    pairs.sort_by_key(|(_, url)| (route::rank(url), url.len(), (*url).clone()));
    pairs
}

/// What the client calls this root layout.
///
/// The layout's `pp-component` name: derived from its path, unique across the
/// app — `scanner::validate` refuses two files that would stamp the same one
/// — and already present in the markup, so nothing new has to be invented for
/// the browser to compare two documents by.
fn root_id(layout: &Entry) -> &str {
    layout.component.as_deref().unwrap_or(&layout.module)
}

/// The `error.rs` that catches this page's failures: the deepest one at or
/// above it. Nearest boundary wins, as in Next.js.
fn error_boundary<'a>(entry: &Entry, entries: &'a [Entry]) -> Option<&'a Entry> {
    if entry.kind != Kind::Page {
        return None;
    }
    entries
        .iter()
        .filter(|e| e.kind == Kind::Error && route::covers(&e.dirs, &entry.dirs))
        .max_by_key(|e| e.dirs.len())
}

/// The `loading.rs` files, each with the URL prefix it covers.
///
/// A loading region belongs to no route group: the client picks one at the
/// start of a navigation, from whatever the app installed, by matching the
/// path it is on. So every one of them is emitted, sorted by URL so the
/// generated list is the same for the same tree.
///
/// `to_urls` yields exactly one URL here — `validate` refuses a `loading.rs`
/// under a dynamic segment, which is the only case that produces more.
fn loading_regions(entries: &[Entry]) -> Vec<(String, &Entry)> {
    let mut regions: Vec<(String, &Entry)> = entries
        .iter()
        .filter(|e| e.kind == Kind::Loading)
        .filter_map(|e| route::to_urls(&e.dirs).into_iter().next().map(|u| (u, e)))
        .collect();
    regions.sort_by(|a, b| a.0.cmp(&b.0));
    regions
}

/// Wrap `inner` in a chain of layout calls, outermost layout applied last.
fn nest_calls(chain: &[&Entry], inner: &str) -> String {
    chain.iter().fold(inner.to_string(), |acc, layout| {
        format!("{}::layout({acc})", layout.module)
    })
}

/// `.route(..)` for one URL, method chain included.
fn route_line(entry: &Entry, url: &str) -> String {
    let handler = match entry.kind {
        Kind::Route => {
            let mut chain = String::new();
            for (i, method) in entry.methods.iter().enumerate() {
                if i == 0 {
                    chain.push_str(&format!(
                        "axum::routing::{method}({}::{method})",
                        entry.module
                    ));
                } else {
                    chain.push_str(&format!(".{method}({}::{method})", entry.module));
                }
            }
            chain
        }
        _ => format!("axum::routing::get({}::page)", entry.module),
    };
    format!(".route(\"{url}\", {handler})")
}

/// Pages that declare at least one `#[rpc]`, in the order they are routed.
fn callable(entries: &[Entry]) -> Vec<&Entry> {
    let mut pages: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == Kind::Page && !e.rpcs.is_empty())
        .collect();
    pages.sort_by_key(|e| e.file.clone());
    pages
}

/// What one URL answers.
///
/// A page's own `#[rpc]` puts POST on it, and so does any component rpc: the
/// component may be rendered there, and its call arrives on that page's URL.
fn methods_of(entry: &Entry, components: bool) -> Vec<String> {
    if entry.kind == Kind::Route {
        return entry.methods.iter().map(|m| m.to_uppercase()).collect();
    }

    let mut methods = vec!["GET".to_string()];
    if !entry.rpcs.is_empty() || (components && entry.kind == Kind::Page) {
        methods.push("POST".to_string());
    }
    methods
}

/// Reading who is being called, which every dispatcher starts with.
///
/// The name and the CSRF check are headers, so this touches no part of the
/// body: reading it belongs to the shim, which is the only thing that knows
/// whether its rpc wants the file spooled or handed over as it arrives.
fn read_name() -> String {
    let mut out = String::new();
    out.push_str("    let name = match ::rahti::rpc_name(req.headers()) {\n");
    out.push_str("        Ok(name) => name,\n");
    out.push_str("        Err(res) => return res,\n");
    out.push_str("    };\n");
    out
}

/// The POST handler for one page's rpcs.
///
/// A `match` on the name the client sent, written out at build time — the
/// dispatch a registry would do at runtime, done by the compiler instead.
///
/// A name the page does not have falls through to the components, because the
/// components on this page post to this URL too and the page cannot know which
/// of them it rendered.
fn dispatcher(entry: &Entry, components: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "/// The rpcs of {}, reached by POST on its own URL.\n",
        entry.file
    ));
    out.push_str(&format!(
        "async fn rpc_{}(req: axum::extract::Request) -> axum::response::Response {{\n",
        entry.module
    ));
    out.push_str(&read_name());
    out.push_str("    match name.as_str() {\n");
    for name in &entry.rpcs {
        out.push_str(&format!(
            "        \"{name}\" => {}::__rahti_rpc_{name}(req).await,\n",
            entry.module
        ));
    }
    if components {
        out.push_str("        _ => component_rpc(&name, req).await,\n");
    } else {
        out.push_str("        other => ::rahti::rpc_unknown(other),\n");
    }
    out.push_str("    }\n}\n\n");
    out
}

/// Every rpc name declared by a `page.rs`, sorted so the generated list is
/// the same for the same tree.
fn page_rpc_names(entries: &[Entry]) -> Vec<String> {
    let mut names: Vec<String> = entries
        .iter()
        .filter(|e| e.kind == Kind::Page)
        .flat_map(|e| e.rpcs.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The dispatcher every page falls through to.
///
/// A component is rendered by whoever calls it, so its rpcs belong to no URL
/// of their own: the client posts to the page it is on, and that page hands
/// the name here.
///
/// Unlike a page's rpcs, these are not matched by name in generated code.
/// They are looked up in the link-time registry every `#[rpc]` submits to,
/// so one dispatcher answers for every component in the binary, whatever
/// file it was written in.
///
/// `PAGE_RPCS` is the exclusion that keeps a page's rpc to its own URL: the
/// registry holds those too, and without it any page could answer any other
/// page's rpc by falling through to here.
fn component_dispatcher(rpcs: &[Rpc], page_rpcs: &[String]) -> String {
    let mut out = String::new();

    out.push_str("/// The app's own page rpcs, which the registry fallback must not answer:\n");
    out.push_str("/// a page's rpc belongs to that page's URL.\n");
    out.push_str("const PAGE_RPCS: &[&str] = &[\n");
    for name in page_rpcs {
        out.push_str(&format!("    \"{name}\",\n"));
    }
    out.push_str("];\n\n");

    out.push_str("/// The rpcs written beside a component, reached from whatever page\n");
    out.push_str("/// rendered it.\n");
    out.push_str("///\n");
    for rpc in rpcs {
        out.push_str(&format!("/// - `{}` — {}\n", rpc.name, rpc.file));
    }
    out.push_str(
        "async fn component_rpc(name: &str, req: axum::extract::Request) \
         -> axum::response::Response {\n",
    );
    out.push_str("    ::rahti::dispatch_registered(name, PAGE_RPCS, req).await\n");
    out.push_str("}\n\n");

    // A page with no `#[rpc]` of its own still renders components, so its URL
    // has to answer POST as well — with nothing but the fallback.
    out.push_str("/// A page with no rpcs of its own, answering for the components it\n");
    out.push_str("/// renders.\n");
    out.push_str(
        "async fn rpc_components(req: axum::extract::Request) -> axum::response::Response {\n",
    );
    out.push_str(&read_name());
    out.push_str("    component_rpc(&name, req).await\n}\n\n");
    out
}

/// The auth layer, as one line.
///
/// Always emitted, unlike the CSRF layer: an app that never calls
/// `rahti::auth::configure` gets the default policy, under which no route is
/// private and the layer does nothing but look for a cookie that is not there.
/// Making it conditional would mean a project could not call `auth::sign_in`
/// until it had first configured a policy it did not need.
fn auth_layer() -> String {
    "        .layer(axum::middleware::from_fn(::rahti::auth_guard))\n".to_string()
}

/// Routes that share the same wrapping, and so the same sub-router.
struct Group<'a> {
    chain: Vec<&'a Entry>,
    boundary: Option<&'a Entry>,
    lines: Vec<String>,
}

pub fn routes_rs(
    entries: &[Entry],
    component_rpcs: &[Rpc],
    sockets: &[Rpc],
    config: &Config,
) -> String {
    let mut out = String::new();
    let has_components = !component_rpcs.is_empty();

    out.push_str("// @generated by rahti-build — do not edit.\n");
    out.push_str("// Regenerated on every build from ");
    out.push_str(&config.app_dir);
    out.push_str("/\n\n");
    out.push_str("#![allow(dead_code)]\n\n");
    out.push_str("use axum::Router;\n\n");

    out.push_str("/// Host from rahti.config.json.\n");
    out.push_str(&format!("pub const HOST: &str = \"{}\";\n", config.host));
    out.push_str("/// Port from rahti.config.json.\n");
    out.push_str(&format!("pub const PORT: u16 = {};\n", config.port));
    out.push_str("/// Static asset root from rahti.config.json.\n");
    out.push_str(&format!(
        "pub const PUBLIC_DIR: &str = \"{}\";\n\n",
        config.public_dir
    ));

    let not_found = entries
        .iter()
        .find(|e| e.kind == Kind::NotFound && e.dirs.is_empty());

    // Only files the router actually calls get a module. A layout counts
    // once some page below it inherits it.
    // A layout above another layout's document is inherited by nothing, so
    // membership of a real chain — not mere ancestry — is what counts.
    let used_layouts: Vec<&Entry> = entries
        .iter()
        .filter(|l| {
            l.kind == Kind::Layout
                && entries
                    .iter()
                    .any(|e| layout_chain(e, entries).iter().any(|c| c.file == l.file))
        })
        .collect();

    let used_errors: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            e.kind == Kind::Error
                && entries
                    .iter()
                    .any(|p| error_boundary(p, entries).is_some_and(|b| b.file == e.file))
        })
        .collect();

    let wired: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            e.kind.is_routable()
                || (e.kind == Kind::NotFound && e.dirs.is_empty())
                || e.kind == Kind::Loading
                || used_layouts.iter().any(|l| l.file == e.file)
                || used_errors.iter().any(|b| b.file == e.file)
        })
        .collect();

    for entry in &wired {
        out.push_str(&format!(
            "#[path = \"{}\"]\nmod {};\n",
            module_path(&entry.file),
            entry.module
        ));
    }
    out.push('\n');

    // Routes sharing a layout chain and an error boundary share a
    // sub-router, because both are layers and a layer covers a whole router.
    let mut groups: Vec<Group> = Vec::new();
    for (entry, url) in ordered(entries) {
        let chain = layout_chain(entry, entries);
        let boundary = error_boundary(entry, entries);
        let line = route_line(entry, url);
        match groups.iter_mut().find(|g| {
            g.boundary.map(|b| &b.file) == boundary.map(|b| &b.file)
                && g.chain
                    .iter()
                    .map(|e| &e.file)
                    .eq(chain.iter().map(|e| &e.file))
        }) {
            Some(group) => group.lines.push(line),
            None => groups.push(Group {
                chain,
                boundary,
                lines: vec![line],
            }),
        }
    }

    if let Some(nf) = not_found {
        let as_page = Entry {
            kind: Kind::Page,
            ..nf.clone()
        };
        let chain = layout_chain(&as_page, entries);
        out.push_str("/// The 404 handler, wrapped in the layouts at the app root.\n");
        out.push_str("///\n");
        out.push_str("/// Called below the static-file fallback rather than inside a route\n");
        out.push_str("/// group, so its layouts are applied here instead of as layers.\n");
        out.push_str("async fn not_found() -> impl axum::response::IntoResponse {\n");
        out.push_str("    (\n        axum::http::StatusCode::NOT_FOUND,\n        ");
        let nested = nest_calls(&chain, &format!("{}::not_found().await", nf.module));
        match root_layout(&as_page, entries) {
            // The identity a page carries, so navigating into a 404 is judged
            // the same way as navigating into any other page.
            Some(root) => out.push_str(&format!(
                "::rahti::root_document(\"{}\", {nested})",
                root_id(root)
            )),
            None => out.push_str(&nested),
        }
        out.push_str(",\n    )\n}\n\n");
    }

    let with_rpcs = callable(entries);
    if has_components {
        out.push_str(&component_dispatcher(
            component_rpcs,
            &page_rpc_names(entries),
        ));
    }
    for entry in &with_rpcs {
        out.push_str(&dispatcher(entry, has_components));
    }

    // Every page answers POST once a component may be rendered on it; only the
    // pages that declare one otherwise.
    let posting: Vec<&Entry> = if has_components {
        let mut pages: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Page).collect();
        pages.sort_by_key(|e| e.file.clone());
        pages
    } else {
        with_rpcs.clone()
    };

    out.push_str("/// The application router, built from the file tree.\n");
    out.push_str("///\n");
    for (entry, url) in ordered(entries) {
        let methods = methods_of(entry, has_components).join(", ");
        let chain = layout_chain(entry, entries);
        let mut notes = Vec::new();
        if !chain.is_empty() {
            notes.push(format!(
                "layouts: {}",
                chain
                    .iter()
                    .map(|l| l.file.as_str())
                    .collect::<Vec<_>>()
                    .join(" > ")
            ));
        }
        if let Some(boundary) = error_boundary(entry, entries) {
            notes.push(format!("errors: {}", boundary.file));
        }
        if !entry.rpcs.is_empty() {
            notes.push(format!("rpc: {}", entry.rpcs.join(", ")));
        }
        let via = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join("; "))
        };
        out.push_str(&format!(
            "/// - `{url}` [{methods}] — {}{via}\n",
            entry.file
        ));
    }
    out.push_str("///\n");
    if has_components {
        out.push_str("/// Every page above also answers the component rpcs, because a\n");
        out.push_str("/// component posts to the page it was rendered on: ");
        out.push_str(
            &component_rpcs
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str(".\n///\n");
    }
    if !sockets.is_empty() {
        out.push_str(&format!(
            "/// Every `#[socket]` connects on `{}?name=…`, dispatched from the\n",
            "/__pulsepoint/ws"
        ));
        out.push_str("/// link-time registry by name alone:\n");
        for socket in sockets {
            out.push_str(&format!("/// - `{}` — {}\n", socket.name, socket.file));
        }
        out.push_str("///\n");
    }
    let regions = loading_regions(entries);
    if !regions.is_empty() {
        out.push_str("/// While a navigation is in flight the client shows the nearest\n");
        out.push_str("/// loading region at or above the URL it is on:\n");
        for (url, entry) in &regions {
            out.push_str(&format!("/// - `{url}` and below — {}\n", entry.file));
        }
        out.push_str("///\n");
    }

    out.push_str("/// Anything the routes above do not claim falls through to `");
    out.push_str(&config.public_dir);
    out.push_str("/`.\n");
    out.push_str("pub fn router() -> Router {\n");

    if has_components {
        // The build checks the names it scanned; this checks the names the
        // linker actually assembled. Two `#[rpc]`s reaching one binary under
        // one name would have whichever the linker placed first answer for
        // both, so the whole set is checked once, here, where everything that
        // will ever be linked in is present.
        out.push_str("    ::rahti::check_rpc_names();\n\n");
    }

    // Rendered once and held, then written into every root document: a
    // loading region takes no arguments, so it cannot differ between two
    // requests. Before the router is assembled only for readability — nothing
    // can be navigating yet.
    if !regions.is_empty() {
        out.push_str("    // The `loading.rs` files, matched in the browser by URL prefix.\n");
        out.push_str("    ::rahti::install_loading(&[\n");
        for (url, entry) in &regions {
            out.push_str(&format!(
                "        (\"{url}\", {}::loading as fn() -> ::rahti::Html),\n",
                entry.module
            ));
        }
        out.push_str("    ]);\n\n");
    }

    for (i, group) in groups.iter().enumerate() {
        let mut wrapping: Vec<&str> = group.chain.iter().map(|l| l.file.as_str()).collect();
        if let Some(boundary) = group.boundary {
            wrapping.push(boundary.file.as_str());
        }
        let label = if wrapping.is_empty() {
            "No layout, no error boundary.".to_string()
        } else {
            wrapping.join(" > ")
        };
        out.push_str(&format!(
            "    // {label}\n    let group_{i} = Router::new()\n"
        ));
        for line in &group.lines {
            out.push_str(&format!("        {line}\n"));
        }

        // Layers are emitted innermost first: a later `.layer` sits further
        // out, so this is the order a response climbs on its way back.
        let depth = group.boundary.map(|b| b.dirs.len());
        let layer_layout = |layout: &Entry| {
            format!(
                "        .layer(axum::middleware::map_response(|res| \
                 ::rahti::apply_layout({}::layout, res)))\n",
                layout.module
            )
        };

        // The outermost layout owns the document, so it is applied apart from
        // the rest: last of all, and with the identity the client navigation
        // compares. `nested` is everything below it.
        let (root, nested) = match group.chain.split_first() {
            Some((root, nested)) => (Some(*root), nested),
            None => (None, &[] as &[&Entry]),
        };

        if let Some(boundary) = group.boundary {
            // Closest to the handler, so a panic becomes a response the
            // boundary above can render instead of killing the connection.
            out.push_str(
                "        .layer(tower_http::catch_panic::CatchPanicLayer::custom(\
                 ::rahti::panic_response))\n",
            );
            // Layouts below the boundary belong to the subtree that failed,
            // so the error UI replaces them rather than sitting inside them.
            for layout in nested.iter().rev() {
                if layout.dirs.len() > depth.unwrap() {
                    out.push_str(&layer_layout(layout));
                }
            }
            out.push_str(&format!(
                "        .layer(axum::middleware::map_response(|res| \
                 ::rahti::apply_error({}::error, res)))\n",
                boundary.module
            ));
        }

        for layout in nested.iter().rev() {
            if depth.is_none_or(|d| layout.dirs.len() <= d) {
                out.push_str(&layer_layout(layout));
            }
        }

        // Outside the boundary even when the boundary sits above it in the
        // tree: an error page is still a page, and it needs the document.
        if let Some(root) = root {
            out.push_str(&format!(
                "        .layer(axum::middleware::map_response(|res| \
                 ::rahti::apply_root_layout({}::layout, \"{}\", res)))\n",
                root.module,
                root_id(root)
            ));
        }

        // Outermost of the group's layers, so a route the policy refuses is a
        // redirect rather than a redirect wrapped in a document — and so the
        // session is decoded before the layouts that may want to render the
        // visitor's name.
        //
        // Here rather than over the whole router because `public/` is served
        // by the fallback below it: a stylesheet needs no session, and an
        // all-private app whose stylesheet redirected to the sign-in page
        // would render that page unstyled.
        out.push_str(&auth_layer());
        out.push_str("        ;\n\n");
    }

    // The rpcs of every page, in one layer-free router.
    //
    // A page's own URL answers POST as well as GET, and the two want opposite
    // treatment: the GET climbs out through its layouts, while the POST is
    // read by `fetch` and has to arrive as the JSON it was asked for. Axum
    // merges two routers on the same path as long as the methods differ, so
    // the wrapping stays with the page and the rpcs stay out of it.
    //
    // Once a component declares an rpc, every page is listed here rather than
    // only the ones with an `#[rpc]` of their own: a component renders wherever
    // it is called, the client posts to the page it is on, and which pages call
    // which components is not something the file tree says.
    if !posting.is_empty() {
        out.push_str("    let rpc = Router::new()\n");
        for entry in &posting {
            let handler = if entry.rpcs.is_empty() {
                "rpc_components".to_string()
            } else {
                format!("rpc_{}", entry.module)
            };
            for url in &entry.urls {
                out.push_str(&format!(
                    "        .route(\"{url}\", axum::routing::post({handler}))\n"
                ));
            }
        }
        // A panic under `fetch` still has to be JSON, so this is the rpc
        // counterpart of the boundary layer a page group carries.
        out.push_str(
            "        .layer(tower_http::catch_panic::CatchPanicLayer::custom(\
             ::rahti::rpc_panic_response))\n",
        );
        // An upload is larger than axum's default body limit, and the limit
        // has to be raised where the body arrives rather than where it is
        // read. Called rather than a constant because it is configurable:
        // the deployment, not the framework, knows how large an upload it is
        // willing to take.
        out.push_str(
            "        .layer(axum::extract::DefaultBodyLimit::max(::rahti::max_upload()))\n",
        );
        // The rpcs need the session too, and for two reasons: `#[rpc(auth)]`
        // is a check against it, and `auth::sign_in` is called from the rpc
        // that verifies a password — the cookie it asks for is written by
        // this layer on the way back out.
        out.push_str(&auth_layer());
        out.push_str("        ;\n\n");
    }

    out.push_str("    Router::new()\n");
    for i in 0..groups.len() {
        out.push_str(&format!("        .merge(group_{i})\n"));
    }
    if !posting.is_empty() {
        out.push_str("        .merge(rpc)\n");
    }

    // The socket endpoint, dispatching by name from the link-time registry.
    // Merged only when the app declares a `#[socket]` somewhere, so an app
    // without one never references the module.
    if !sockets.is_empty() {
        out.push_str(
            "        // If this line does not compile — \"could not find `ws` in \
             `rahti`\" —\n        // enable the feature: rahti = { …, features = \
             [\"ws\"] } in Cargo.toml.\n",
        );
        out.push_str("        .merge(::rahti::ws::routes())\n");
    }

    // Empty outside dev mode, so this costs a release build nothing.
    out.push_str("        .merge(::rahti::dev_routes(::rahti::public_dir(PUBLIC_DIR)))\n");

    out.push_str(
        "        .fallback_service(\n            \
         tower_http::services::ServeDir::new(::rahti::public_dir(PUBLIC_DIR))\n",
    );
    if not_found.is_some() {
        out.push_str("                .not_found_service(axum::routing::any(not_found))\n");
    }
    out.push_str("        )\n");

    // Over the static files too, which is the point: without it a rebuilt
    // stylesheet loses to the browser's own heuristic cache. A no-op outside
    // dev mode.
    out.push_str("        .layer(axum::middleware::from_fn(::rahti::revalidate))\n");

    // Outermost, so the token is on its way to the browser before any rpc can
    // need it — and absent entirely from an app that has no rpc to protect.
    if !posting.is_empty() {
        out.push_str("        .layer(axum::middleware::from_fn(::rahti::csrf))\n");
    }

    out.push_str("}\n");
    out
}

pub fn manifest_json(
    entries: &[Entry],
    components: &[String],
    component_rpcs: &[Rpc],
    sockets: &[Rpc],
    database: Option<&crate::db::Scan>,
    config: &Config,
) -> String {
    let mut out = String::new();
    let has_components = !component_rpcs.is_empty();
    out.push_str("{\n");
    out.push_str("  \"generatedBy\": \"rahti-build\",\n");
    out.push_str(&format!("  \"appDir\": \"{}\",\n", config.app_dir));

    out.push_str("  \"routes\": [\n");
    let pairs = ordered(entries);
    for (i, (entry, url)) in pairs.iter().enumerate() {
        // The rpcs — the page's own, or a component's — are what put POST on
        // a page's URL.
        let methods = methods_of(entry, has_components)
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let comma = if i + 1 == pairs.len() { "" } else { "," };
        let component = match &entry.component {
            Some(id) => format!("\"{id}\""),
            None => "null".to_string(),
        };
        let rpcs = entry
            .rpcs
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let auth_rpcs = entry
            .auth_rpcs
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "    {{ \"url\": \"{url}\", \"kind\": \"{}\", \"methods\": [{methods}], \"component\": {component}, \"rpcs\": [{rpcs}], \"authRpcs\": [{auth_rpcs}], \"file\": \"{}\" }}{comma}\n",
            entry.kind.label(),
            entry.file
        ));
    }
    out.push_str("  ],\n");

    out.push_str("  \"layouts\": [\n");
    let layouts: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Layout).collect();
    for (i, layout) in layouts.iter().enumerate() {
        // What it actually wraps, not what it sits above: a layout above
        // another layout's document wraps nothing.
        let covered: Vec<&str> = entries
            .iter()
            .filter(|e| {
                layout_chain(e, entries)
                    .iter()
                    .any(|c| c.file == layout.file)
            })
            .flat_map(|e| e.urls.iter().map(|u| u.as_str()))
            .collect();
        let urls = covered
            .iter()
            .map(|u| format!("\"{u}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let comma = if i + 1 == layouts.len() { "" } else { "," };
        // `root` is the identity the client compares across a navigation: a
        // page whose root layout differs from the one on screen is loaded
        // rather than swapped in.
        let root = entries
            .iter()
            .any(|e| root_layout(e, entries).is_some_and(|r| r.file == layout.file));
        out.push_str(&format!(
            "    {{ \"file\": \"{}\", \"component\": \"{}\", \"root\": {root}, \"wraps\": [{urls}] }}{comma}\n",
            layout.file,
            layout.component.as_deref().unwrap_or("")
        ));
    }
    out.push_str("  ],\n");

    out.push_str("  \"errors\": [\n");
    let boundaries: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Error).collect();
    for (i, boundary) in boundaries.iter().enumerate() {
        let caught: Vec<&str> = entries
            .iter()
            .filter(|e| error_boundary(e, entries).is_some_and(|b| b.file == boundary.file))
            .flat_map(|e| e.urls.iter().map(|u| u.as_str()))
            .collect();
        let urls = caught
            .iter()
            .map(|u| format!("\"{u}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let comma = if i + 1 == boundaries.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{ \"file\": \"{}\", \"component\": \"{}\", \"catches\": [{urls}] }}{comma}\n",
            boundary.file,
            boundary.component.as_deref().unwrap_or("")
        ));
    }
    out.push_str("  ],\n");

    // Every `loading.rs`, with the URL prefix the browser matches it by.
    // Unlike a layout or a boundary, a loading region covers no fixed set of
    // routes: it answers for whatever path is below it, dynamic segments and
    // future pages included.
    out.push_str("  \"loading\": [\n");
    let regions = loading_regions(entries);
    for (i, (url, entry)) in regions.iter().enumerate() {
        let comma = if i + 1 == regions.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{ \"file\": \"{}\", \"component\": \"{}\", \"covers\": \"{url}\" }}{comma}\n",
            entry.file,
            entry.component.as_deref().unwrap_or("")
        ));
    }
    out.push_str("  ],\n");

    // Files only. A component is named after its function, and the build
    // script does not parse Rust — the name lives in the markup the macro
    // stamps, which is where the client runtime reads it from anyway.
    out.push_str("  \"components\": [\n");
    for (i, file) in components.iter().enumerate() {
        let comma = if i + 1 == components.len() { "" } else { "," };
        // Name and module both: the generated router no longer names a
        // component's rpc — it is found in the link-time registry — so the
        // manifest is the only place left that says where one is written.
        let rpcs = component_rpcs
            .iter()
            .filter(|r| &r.file == file)
            .map(|r| {
                format!(
                    "{{ \"name\": \"{}\", \"module\": \"{}\", \"auth\": {} }}",
                    r.name, r.module, r.auth
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "    {{ \"file\": \"{file}\", \"rpcs\": [{rpcs}] }}{comma}\n"
        ));
    }
    out.push_str("  ],\n");

    // Every `#[socket]`, page and component alike: the names are one flat
    // namespace, so the manifest reports them as one flat list. Empty is the
    // ordinary shape — an app that opens no sockets — and an entry here is
    // also the record that the project needs rahti's `ws` feature.
    out.push_str("  \"sockets\": [\n");
    for (i, socket) in sockets.iter().enumerate() {
        let comma = if i + 1 == sockets.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{ \"name\": \"{}\", \"file\": \"{}\", \"auth\": {} }}{comma}\n",
            socket.name, socket.file, socket.auth
        ));
    }
    out.push_str("  ],\n");

    // What the project's database looks like from the outside: which backend,
    // which entities exist, and the migrations in the order they are applied.
    //
    // Here rather than left to be discovered because the manifest is what
    // tooling — and an agent asked to add a table — reads to learn the shape
    // of the project. A directory listing answers "what files are there"; this
    // answers "what is wired in", which is the question that has been wrong
    // before.
    match (database, config.db.as_ref()) {
        (Some(scan), Some(db)) => {
            let list = |files: &[String]| {
                files
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str("  \"db\": {\n");
            out.push_str(&format!("    \"backend\": \"{}\",\n", db.backend.label()));
            out.push_str(&format!("    \"models\": [{}],\n", list(&scan.models)));
            out.push_str(&format!(
                "    \"migrations\": [{}]\n",
                list(&scan.migrations)
            ));
            out.push_str("  }\n");
        }
        _ => out.push_str("  \"db\": null\n"),
    }

    out.push_str("}\n");
    out
}

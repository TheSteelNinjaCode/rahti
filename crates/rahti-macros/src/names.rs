//! The names a block's client code reads that nothing can supply.
//!
//! A binding reads the block's scope: what its script declares, the rows of
//! the loops it is written in, and `pp`. A script reads what it declares
//! itself. Either may read the window. A name that is none of those is
//! `undefined` on the page, or a `ReferenceError` when the code runs:
//!
//! ```ignore
//! <p hidden={domians.length > 0}>"No domains yet."</p>     // no such name
//! <button onclick={laod()}>"Reload"</button>               // no such name
//! <script>r##"
//!     const [domains, setDomains] = pp.state([]);
//!     async function load() {
//!         setDomians(await pp.rpc("list_domains", {}));    // no such name
//!     }
//! "##</script>
//! ```
//!
//! A script's names are certain whatever the block is: it runs as a function
//! handed `pp`, and reads nothing else it does not declare. A binding's are
//! certain only in a block that has one script of its own. A block with no
//! script renders in the scope of the block around it, and fragment-rooted
//! children read the component they are passed to, so their names are not
//! looked at.

use std::collections::HashSet;
use std::ops::Range;

use rahti_js::Code;

use crate::parse::{ClientKind, ClientSource, Error};

/// What a handler may call its event, and the element it fired on.
const EVENT_NAMES: [&str; 6] = ["event", "e", "$event", "target", "currentTarget", "el"];

/// What the Tailwind scaffold's `main.js` publishes on the window before
/// mounting.
const PUBLISHED: [&str; 1] = ["twMerge"];

/// Set by `rahti-build` via `cargo:rustc-env` to the project's `css.engine`.
const CSS_ENGINE_ENV: &str = "RAHTI_CSS_ENGINE";

/// The names `main.js` publishes under this `css.engine`.
///
/// Only the Tailwind scaffold imports the merge helper, so under `plain` or
/// `none` a bare `twMerge` is a name nothing supplies and a `ReferenceError`
/// in the browser. A crate with no Rahti build script — a component library —
/// has no engine to consult, and its code runs in whichever application
/// mounts it, so the name is admitted there.
fn published(engine: Option<&str>) -> &'static [&'static str] {
    match engine {
        None | Some("tailwind") => &PUBLISHED,
        Some(_) => &[],
    }
}

/// A name some piece of the block's code reads and nothing supplies.
#[derive(Debug, PartialEq)]
pub struct Unknown {
    /// Which of the block's sources reads it.
    pub source: usize,
    /// `script`, `binding` or `handler`.
    pub place: &'static str,
    pub name: String,
    /// Where in that source the name is first written.
    pub span: Range<usize>,
    /// The names that code could have read instead, sorted.
    pub known: Vec<String>,
}

/// Refuses the first name the block's code reads that nothing supplies.
pub fn check(sources: &[ClientSource]) -> Result<(), Error> {
    let Some(unknown) = unknown(sources).into_iter().next() else {
        return Ok(());
    };
    let locator = crate::rpc_calls::Locator::new(&sources[unknown.source]);
    Err(Error {
        span: locator.span(&unknown.span),
        msg: crate::authoring::unknown_name(
            unknown.place,
            &unknown.name,
            &unknown.known,
            &locator.line_at(unknown.span.start),
        ),
    })
}

/// Every name the block's code reads that nothing supplies, in source order.
pub fn unknown(sources: &[ClientSource]) -> Vec<Unknown> {
    unknown_under(sources, std::env::var(CSS_ENGINE_ENV).ok().as_deref())
}

/// [`unknown`], for a project on the named `css.engine`.
pub fn unknown_under(sources: &[ClientSource], engine: Option<&str>) -> Vec<Unknown> {
    let published = published(engine);
    // Most blocks have no client code at all.
    if sources.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut scripts = sources
        .iter()
        .filter(|source| source.kind == ClientKind::Script && !source.foreign);
    // A binding reads one script's scope; with none it reads the scope of
    // the block around this one, and with two there is no one scope.
    let scope: Option<Vec<String>> = match (scripts.next(), scripts.next()) {
        (Some(script), None) => rahti_js::component_script(&script.source)
            .ok()
            .map(|script| script.bindings),
        _ => None,
    };

    for (index, source) in sources.iter().enumerate() {
        if source.foreign {
            continue;
        }
        // What a script declares, to say which name was meant.
        let own = match source.kind {
            ClientKind::Script => rahti_js::component_script(&source.source)
                .map(|script| script.bindings)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let (code, place, mut known): (Code, &'static str, HashSet<&str>) = match source.kind {
            ClientKind::Script => (
                Code::Script,
                "script",
                own.iter().map(String::as_str).collect(),
            ),
            ClientKind::Expression | ClientKind::Handler => {
                let Some(scope) = &scope else {
                    continue;
                };
                let mut known: HashSet<&str> = scope.iter().map(String::as_str).collect();
                known.insert("pp");
                // Rows of the loops the code is written in.
                let mut read = true;
                for directive in &source.loops {
                    match loop_names(directive) {
                        Some(names) => known.extend(names),
                        None => read = false,
                    }
                }
                if !read {
                    continue;
                }
                if source.kind == ClientKind::Handler {
                    known.extend(EVENT_NAMES);
                    (Code::Handler, "handler", known)
                } else {
                    (Code::Expression, "binding", known)
                }
            }
        };
        known.extend(published);
        for (name, span) in rahti_js::free_names(code, &source.source) {
            if known.contains(name.as_str()) || rahti_js::is_global(&name) {
                continue;
            }
            let mut known: Vec<String> = known
                .iter()
                .filter(|name| !EVENT_NAMES.contains(name) && !PUBLISHED.contains(name))
                .map(|name| name.to_string())
                .collect();
            known.sort();
            found.push(Unknown {
                source: index,
                place,
                name,
                span,
                known,
            });
        }
    }
    found
}

/// The names a `pp-for="item in items"` or `pp-for="(item, index) in items"`
/// gives its rows; `None` when the directive is written some other way.
fn loop_names(directive: &str) -> Option<Vec<&str>> {
    let (variables, _) = directive.split_once(" in ")?;
    let variables = variables.trim();
    let names: Vec<&str> = match variables
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        Some(pair) => pair.split(',').map(str::trim).collect(),
        None => vec![variables],
    };
    names
        .iter()
        .all(|name| crate::component::is_valid_js_identifier(name))
        .then_some(names)
}

#[cfg(test)]
#[path = "tests/names.rs"]
mod tests;

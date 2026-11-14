//! The names a piece of client code reads without declaring them.
//!
//! A binding, a handler and a script all read names they do not declare: a
//! binding reads the block's state, a handler calls the script's functions,
//! and a script calls `fetch`. The runtime looks each one up when the code
//! runs, so a misspelled one is `undefined` or a `ReferenceError` on the
//! page. This module finds them, with where each is first written; `html!`
//! decides which of them nothing can supply.
//!
//! ```text
//! domains.length > 0            // reads `domains`
//! setOpen(!open)                // reads `setOpen` and `open`
//! typeof Chart !== "undefined"  // reads nothing: `typeof` asks, it does not read
//! window.Chart                  // reads `window`
//! ```

use std::collections::HashSet;
use std::ops::Range;

use oxc_allocator::Allocator;
use oxc_ast::ast::{Expression, IdentifierReference, UnaryExpression};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::{SourceType, Span};
use oxc_syntax::operator::UnaryOperator;

use crate::Code;

/// Each name `source` reads without declaring, once, in source order, with
/// where it is first written. Nothing, for code that does not parse:
/// compiling it reports that.
///
/// A script is read as the runtime runs it, inside a function that is handed
/// `pp`, so `pp` is not among a script's names; it is among a binding's.
pub fn free_names(code: Code, source: &str) -> Vec<(String, Range<usize>)> {
    let (prefix, suffix) = match code {
        Code::Script => ("async function __pp_script(pp, __pp_server) {\n", "\n}"),
        Code::Expression => ("(", "\n);"),
        Code::Handler if crate::is_expression(source) => ("(", "\n);"),
        Code::Handler => ("function __pp_handler() {\n", "\n}"),
    };
    let wrapped = format!("{prefix}{source}{suffix}");
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
    if parsed.diagnostics.errors().next().is_some() {
        return Vec::new();
    }
    let built = SemanticBuilder::new().build(&parsed.program);
    let mut reader = Reader {
        scoping: built.semantic.scoping(),
        offset: prefix.len(),
        limit: source.len(),
        asked: HashSet::new(),
        seen: HashSet::new(),
        found: Vec::new(),
    };
    reader.visit_program(&parsed.program);
    reader.found
}

struct Reader<'s> {
    scoping: &'s Scoping,
    offset: usize,
    limit: usize,
    /// The operand of each `typeof name`.
    asked: HashSet<Span>,
    seen: HashSet<String>,
    found: Vec<(String, Range<usize>)>,
}

impl<'a> Visit<'a> for Reader<'_> {
    fn visit_unary_expression(&mut self, unary: &UnaryExpression<'a>) {
        if unary.operator == UnaryOperator::Typeof
            && let Expression::Identifier(identifier) = unary.argument.without_parentheses()
        {
            self.asked.insert(identifier.span);
        }
        walk::walk_unary_expression(self, unary);
    }

    fn visit_identifier_reference(&mut self, identifier: &IdentifierReference<'a>) {
        let declared = identifier
            .reference_id
            .get()
            .and_then(|reference| self.scoping.get_reference(reference).symbol_id())
            .is_some();
        if declared || self.asked.contains(&identifier.span) {
            return;
        }
        if !self.seen.insert(identifier.name.to_string()) {
            return;
        }
        let at = |position: u32| {
            (position as usize)
                .saturating_sub(self.offset)
                .min(self.limit)
        };
        self.found.push((
            identifier.name.to_string(),
            at(identifier.span.start)..at(identifier.span.end),
        ));
    }
}

/// Is this a name every browser page can read: part of the language, or of
/// the window a page runs in?
///
/// A name that starts with a capital is taken to be one — browsers keep
/// adding interfaces (`GPUBufferUsage`, `OffscreenCanvas`), a vendored
/// library publishes its own (`Chart`), and state, setters and functions are
/// written in camelCase. The rest of the window is a closed list.
pub fn is_global(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_uppercase()) || WINDOW.contains(&name)
}

/// The window's own lower-case names, and the language's.
const WINDOW: &[&str] = &[
    // The language.
    "arguments",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "escape",
    "eval",
    "globalThis",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "undefined",
    "unescape",
    // The window and what hangs off it.
    "addEventListener",
    "alert",
    "atob",
    "blur",
    "btoa",
    "caches",
    "cancelAnimationFrame",
    "cancelIdleCallback",
    "clearInterval",
    "clearTimeout",
    "close",
    "closed",
    "confirm",
    "console",
    "cookieStore",
    "createImageBitmap",
    "crossOriginIsolated",
    "crypto",
    "customElements",
    "devicePixelRatio",
    "dispatchEvent",
    "document",
    "fetch",
    "focus",
    "frames",
    "getComputedStyle",
    "getSelection",
    "history",
    "indexedDB",
    "innerHeight",
    "innerWidth",
    "isSecureContext",
    "localStorage",
    "location",
    "matchMedia",
    "navigation",
    "navigator",
    "open",
    "opener",
    "origin",
    "outerHeight",
    "outerWidth",
    "pageXOffset",
    "pageYOffset",
    "parent",
    "performance",
    "postMessage",
    "print",
    "prompt",
    "queueMicrotask",
    "removeEventListener",
    "reportError",
    "requestAnimationFrame",
    "requestIdleCallback",
    "scheduler",
    "screen",
    "screenLeft",
    "screenTop",
    "screenX",
    "screenY",
    "scroll",
    "scrollBy",
    "scrollTo",
    "scrollX",
    "scrollY",
    "self",
    "sessionStorage",
    "setInterval",
    "setTimeout",
    "speechSynthesis",
    "stop",
    "structuredClone",
    "top",
    "trustedTypes",
    "visualViewport",
    "window",
];

#[cfg(test)]
#[path = "tests/names.rs"]
mod tests;

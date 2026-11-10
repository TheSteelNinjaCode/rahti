//! Checking the props a component is passed and the props it reads.
//!
//! A `{…}` prop on a component tag reaches the component as an attribute of
//! its root element, which its code reads as `pp.props.name`. Nothing ties
//! the two names together at runtime: a misspelled one is an attribute
//! nobody reads on one side and `undefined` on the other. Two checks do, each
//! only where it is certain.
//!
//! **At the tag.** `<MigrationDialog curent={id}>` is refused when the
//! component's file never reads `pp.props.curent` and the name is not an
//! attribute or handler the root element would use for itself. The build
//! lists, for each component a file can call, the props its file reads, and
//! leaves a component out of the check when it may read a prop some way the
//! build cannot see. A component that reads no prop at all is left out too:
//! it is a wrapper, and what it is passed is for its root.
//!
//! **In the component.** A component whose root element takes no spread has
//! exactly the props that root is written with, so `pp.props.userNmae` there
//! is read off a root that has no such attribute.

use proc_macro2::Span;
use rahti_js::Code;

use crate::parse::{ClientKind, ClientSource, Error, Op, is_native_event_name};
use crate::rpc_shapes::Shapes;

/// `user-name` as the client reads it: `userName`.
fn camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        match c {
            '-' => upper = true,
            c if upper => {
                out.extend(c.to_uppercase());
                upper = false;
            }
            c => out.push(c),
        }
    }
    out
}

/// Attribute names an element uses for itself, which a `{…}` prop may carry
/// to a component's root whether or not the component reads it.
const ATTRIBUTES: &[&str] = &[
    "accept",
    "accesskey",
    "action",
    "allow",
    "alt",
    "as",
    "async",
    "autocapitalize",
    "autocomplete",
    "autocorrect",
    "autofocus",
    "autoplay",
    "capture",
    "charset",
    "checked",
    "cite",
    "class",
    "cols",
    "colspan",
    "content",
    "contenteditable",
    "controls",
    "coords",
    "crossorigin",
    "cx",
    "cy",
    "d",
    "data",
    "datetime",
    "decoding",
    "default",
    "defer",
    "dir",
    "dirname",
    "disabled",
    "download",
    "draggable",
    "enctype",
    "enterkeyhint",
    "fill",
    "for",
    "form",
    "formaction",
    "formenctype",
    "formmethod",
    "formnovalidate",
    "formtarget",
    "headers",
    "height",
    "hidden",
    "high",
    "href",
    "hreflang",
    "id",
    "inert",
    "inputmode",
    "integrity",
    "is",
    "ismap",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "key",
    "kind",
    "label",
    "lang",
    "list",
    "loading",
    "loop",
    "low",
    "max",
    "maxlength",
    "media",
    "method",
    "min",
    "minlength",
    "multiple",
    "muted",
    "name",
    "nonce",
    "novalidate",
    "open",
    "optimum",
    "part",
    "pattern",
    "ping",
    "placeholder",
    "playsinline",
    "points",
    "popover",
    "popovertarget",
    "popovertargetaction",
    "poster",
    "preload",
    "r",
    "readonly",
    "referrerpolicy",
    "rel",
    "required",
    "reversed",
    "role",
    "rows",
    "rowspan",
    "rx",
    "ry",
    "sandbox",
    "scope",
    "selected",
    "shape",
    "size",
    "sizes",
    "slot",
    "span",
    "spellcheck",
    "src",
    "srcdoc",
    "srclang",
    "srcset",
    "start",
    "step",
    "stroke",
    "style",
    "tabindex",
    "target",
    "title",
    "transform",
    "translate",
    "type",
    "usemap",
    "value",
    "viewBox",
    "viewbox",
    "width",
    "wrap",
    "x",
    "x1",
    "x2",
    "xmlns",
    "y",
    "y1",
    "y2",
];

/// Is this a name an element takes for itself, read by a component or not?
fn is_attribute(name: &str) -> bool {
    is_native_event_name(name)
        || [
            "data-", "aria-", "pp-", "stroke-", "fill-", "xlink:", "xml:",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || ATTRIBUTES.contains(&name)
}

/// Refuses the first `{…}` prop, on a component tag of this block, that the
/// component does not read and its root element would not use.
pub fn check_passed(ops: &[Op], shapes: &Shapes) -> Result<(), Error> {
    for op in ops {
        let Op::Component(call) = op else { continue };
        if let Some(children) = &call.children {
            check_passed(&children.ops, shapes)?;
        }
        let component = call.name.to_string();
        let Some(read) = shapes.props_read(&component) else {
            continue;
        };
        // A component that reads no prop of its own passes what it is given
        // on to its root, where a helper written elsewhere may read it:
        // nothing certain can be said about a name it is passed.
        if read.is_empty() {
            continue;
        }
        for (name, _) in &call.bindings {
            if is_attribute(name) || read.contains(&camel(name)) {
                continue;
            }
            return Err(Error {
                span: call.name.span(),
                msg: crate::authoring::prop_not_read(
                    &component,
                    name,
                    &read.iter().cloned().collect::<Vec<_>>(),
                ),
            });
        }
    }
    Ok(())
}

/// Refuses the first `pp.props.name`, in the code of a component whose root
/// element takes no spread, that the root is not written with.
///
/// `root` is every attribute name on that root; `None` when the block's root
/// is not an element or a spread can add to it. `locate` gives the span and
/// the line of a byte range in one of the sources.
pub fn check_read(
    sources: &[ClientSource],
    root: Option<&[String]>,
    locate: impl Fn(&ClientSource, &std::ops::Range<usize>) -> (Span, String),
) -> Result<(), Error> {
    let Some(root) = root else {
        return Ok(());
    };
    let props: Vec<String> = root.iter().map(|name| camel(name)).collect();
    let mut found = Vec::new();
    for source in sources.iter().filter(|source| !source.foreign) {
        let code = match source.kind {
            ClientKind::Script => Code::Script,
            ClientKind::Handler => Code::Handler,
            ClientKind::Expression => Code::Expression,
        };
        let reads = rahti_js::prop_reads(code, &source.source);
        // Used whole, any prop may be read: nothing here is certain.
        if reads.whole {
            return Ok(());
        }
        found.extend(reads.names.into_iter().map(|read| (source, read)));
    }
    for (source, (name, span)) in found {
        if props.contains(&name) {
            continue;
        }
        let (at, line) = locate(source, &span);
        return Err(Error {
            span: at,
            msg: crate::authoring::prop_never_given(&name, &props, &line),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/props.rs"]
mod tests;

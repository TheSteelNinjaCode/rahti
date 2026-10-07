//! Turn a template's tokens into a flat list of writes.
//!
//! The template lexer (`template.rs`) reads `html! {r##"…"##}` as text and
//! hands this module tokens of a few fixed shapes. The grammar is deliberately
//! small. Everything that is known at compile time collapses into one
//! `&'static str`; only interpolation survives to runtime.
//!
//! ```text
//! markup  := directive? node*
//! node    := element | component | fragment | text | binding | block
//! element := '<' name attr* ('/>' | '>' (node* | raw) '</' name '>')
//! component := '<' Name prop* ('/>' | '>' node* '</' Name '>')
//!            // Name starts with a capital: the `#[component]` fn it calls
//! fragment := '<' '>' node* '</' '>'  // a block's root only — see
//!            // [`Parser::fragment`]
//! prop    := ident ('=' (literal | block | binding))?  // no '=' means `true`
//! attr    := name ('=' (string | binding | block))?
//! text    := string-literal          // one run of template text
//! binding := '{' marker '}'          // client-side; its source, verbatim
//! block   := '@' '{' expr '}'        // server-side; a Rust expression
//! raw     := raw-string-literal      // a `<script>` / `<style>` body;
//!            // `@{expr}` is lifted in `<script>` — see [`Parser::raw_text`]
//! directive := '@component' string   // written by `#[component]`, not by hand
//! ```
//!
//! `{…}` belongs to the client runtime, which reads it back out of the DOM and
//! evaluates it against the component's own scope. The macro does not look
//! inside one: it copies the source through and escapes it as text, so what
//! reaches the browser is the expression the author wrote. Rust interpolation
//! is `@{…}` — see [`Op::Text`].

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use std::str::FromStr;

#[cfg(test)]
#[path = "tests/parse.rs"]
mod tests;

/// The attribute that names a block for the client runtime.
pub const COMPONENT_ATTR: &str = "pp-component";

/// The attribute that hands a `<template>` of slot content to the block that
/// wrote it. The client runtime renders the template's content in the named
/// owner's scope and re-renders it whenever that owner's state changes —
/// which is what lets `<Card>{count}</Card>` read the page's `count` the way
/// React children read the scope they were written in.
pub const OWNER_ATTR: &str = "pp-owner";
/// Names the block whose scope runs the native handlers on an element. The
/// macro writes it beside `pp-component` on a root that handles its own
/// events — see [`is_native_event_name`].
pub const EVENT_OWNER_ATTR: &str = "pp-event-owner";

/// Does this attribute name a native DOM event handler?
///
/// The client runtime's naming contract: a lowercase, unhyphenated `on…`
/// (`onclick`, `onsubmit`) is a native event, and a kebab-case `on-…` is a
/// component callback prop. Only the naming is checked, never a list of event
/// names — the runtime decides what is native against the live element, and
/// `pp-event-owner` only routes the handlers it binds, so a name that turns
/// out not to be an event leaves the marker inert.
pub fn is_native_event_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.len() > 2 && lower.starts_with("on") && lower[2..].bytes().all(|b| b.is_ascii_lowercase())
}

/// A root attribute the client runtime would evaluate in some scope: a `{…}`
/// that is not an event handler (owned separately), not a runtime-managed
/// `pp-*` attribute, and not JSON data (`data-config="{\"a\":1}"`).
fn is_scope_binding(name: &str, value: &str) -> bool {
    if !value.contains('{') || is_native_event_name(name) || name.starts_with("pp-") {
        return false;
    }
    let trimmed = value.trim_start();
    let after_brace = trimmed.strip_prefix('{').map(str::trim_start);
    !matches!(after_brace, Some(rest) if rest.starts_with('"') || rest.starts_with('}'))
}

/// The markers a `<>…</>` fragment ships between.
///
/// A fragment is a run of siblings framed by a comment pair — the open
/// marker carrying the block's name after `pp:`, or nothing when the
/// fragment is grouping only:
///
/// ```text
/// <!--pp:page_6d4ae73d--><h1>…</h1><p>…</p><!--/pp-->
/// ```
///
/// Comments are legal in every content context — `<tbody>`, `<ul>`,
/// `<select>` — so a fragment of table rows survives the browser's parser
/// where a wrapper element would be foster-parented out. At mount the
/// runtime turns each pair into a live layout-invisible boundary that
/// carries the name; a close marker pairs with the nearest unclosed open
/// among its siblings, so fragments nest.
pub const FRAGMENT_OPEN: &str = "<!--pp:-->";
pub const FRAGMENT_CLOSE: &str = "<!--/pp-->";

/// Byte offset into [`FRAGMENT_OPEN`] where a block's name lands: just
/// after `pp:`.
pub const FRAGMENT_NAME_AT: usize = 7;

/// Tags HTML5 forbids a closing tag on.
pub const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Tags whose body is text, not markup. Nothing inside one is parsed: no
/// elements, no escaping, and no `{…}` — a brace in JavaScript is a block.
const RAW_TEXT: [&str; 2] = ["script", "style"];

/// Attributes HTML reads by presence rather than by value.
///
/// `disabled="false"` disables the control, so a `false` written on one of
/// these has to make the attribute disappear — which is what JSX does, and
/// what [`is_boolean_attribute`] is consulted for.
///
/// The list is the HTML specification's, and what is *not* on it matters as
/// much as what is. `aria-*` attributes are enumerated, not boolean:
/// `aria-hidden="false"` is a different thing from no `aria-hidden` at all,
/// and dropping it would change what a screen reader announces. So are
/// `contenteditable`, `draggable`, `spellcheck`, `translate` and
/// `autocomplete`, which read `"true"`/`"false"` or `"on"`/`"off"` as words.
/// All of them are deliberately absent, and a `bool` written on one renders
/// as the word it is.
const BOOLEAN_ATTRS: [&str; 29] = [
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    // Takes an optional filename, so a `false` on it means "not a download".
    "download",
    "formnovalidate",
    // Enumerated since `hidden="until-found"`, but `hidden="false"` still
    // hides, which is the failure this list exists to prevent.
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
    "shadowrootclonable",
    "shadowrootdelegatesfocus",
    "shadowrootserializable",
];

/// Is this attribute one HTML reads by presence?
///
/// Decided by name alone rather than by name and element. `open` is boolean on
/// `<details>` and meaningless on a `<div>`, and treating it as boolean on
/// both costs nothing: an attribute that means nothing there is better left
/// off than written as the word `false`.
pub fn is_boolean_attribute(name: &str) -> bool {
    BOOLEAN_ATTRS.contains(&name)
}

/// What a node at the top level of an `html!` block turned out to be.
///
/// Rahti follows React's rule: a page, a layout or an error boundary renders
/// one parent element. Enforcing it here means a route always has exactly one
/// node to attach to, hydrate, or swap.
enum Root {
    /// `<!DOCTYPE html>` — a prologue, not an element. Always allowed.
    Doctype,
    Element(String, Span),
    /// `<Card …>` as the block's root: composition. The block renders another
    /// component's element and owns no markup of its own, so it takes no
    /// `pp-component` — the client runtime reads one identity per element,
    /// and that element is the inner component's.
    Component(String, Span),
    /// `<>…</>` as the block's root: several siblings between the fragment
    /// markers, whose open comment takes the block's name.
    Fragment(Span),
    /// Text, `{expr}` or `<slot />` sitting outside any element.
    Loose(Span, &'static str),
}

/// One write into the output buffer.
#[derive(Debug)]
pub enum Op {
    /// Markup fixed at compile time. Adjacent ones are merged before emit.
    ///
    /// A client `{…}` binding is one of these: it is markup as far as the
    /// server is concerned, and only means something once the runtime reads
    /// it back off the page.
    Static(String),
    /// `@{expr}` in element position — escaped as text.
    Text(TokenStream),
    /// `name=@{expr}` — the whole attribute, because the value decides
    /// whether there is one. The name is carried here rather than folded into
    /// the static markup around it: `href=@{None}` writes nothing at all, and
    /// a name already pushed cannot be taken back.
    Attr {
        /// The name with its leading space — ` href` — so the spacing goes
        /// or stays with the attribute.
        name: String,
        expr: TokenStream,
        /// The name is one HTML reads by presence. See [`BOOLEAN_ATTRS`].
        boolean: bool,
    },
    /// The attributes of an element carrying a `@{..spread}`, built at
    /// runtime so that a later one replaces an earlier one instead of being
    /// written beside it. See [`TagAttr`].
    OpenTag(Vec<TagAttr>),
    /// `@{expr}` inside a `<script>` body — rendered as JS source through
    /// `RenderJs`, the script-side counterpart of `Render`. Separate because
    /// JS source has no HTML escaping, and a string that renders bare in
    /// markup needs quoting in a script. Lifted past the raw-text rule that
    /// would otherwise copy the expression through as a JS block.
    ScriptText(TokenStream),
    /// `<slot />` — the child content this layout wraps.
    Slot,
    /// A root layout's `<slot />`, which always sits in a deferred `<template
    /// pp-component>` (injected by `wrap_slot`, or written by the layout). The children are
    /// written through `rahti::render_deferred_children`, which raises each
    /// brace reference one encoding layer so a value's `&#123;` survives the
    /// browser's parse and the runtime never reads it as an expression.
    DeferredSlot,
    /// `<Card …>` — a call to the `#[component]` function of that name,
    /// rendered where the tag stands.
    Component(ComponentCall),
}

/// One attribute of an element whose open tag is built at runtime.
///
/// An element without a spread never produces these: its attributes fold into
/// the static markup, exactly as before, and only an `@{…}` value survives to
/// runtime. A spread is what forces the whole tag to be assembled, because
/// what a spread carries is not known until it is evaluated — and writing an
/// attribute twice and letting the browser take the first would make a spread
/// lose to whatever it was meant to override.
#[derive(Debug)]
pub enum TagAttr {
    /// `class="card"`, or a bare boolean attribute, or a client `{…}`
    /// binding — anything whose text is known when it is written. The value
    /// is unescaped here and escaped when the list is built, and `None` is
    /// the bare form: `<input required>`.
    Literal { name: String, value: Option<String> },
    /// `width=@{size}` — the value decides whether the attribute is written.
    Value {
        name: String,
        expr: TokenStream,
        boolean: bool,
    },
    /// `@{..extra}` — everything the expression carries, folded in at the
    /// point it was written.
    Spread(TokenStream),
    /// Not an attribute: the block's root handles its own events, so a
    /// spread on it must not bring in a handler of the caller's. Put first
    /// in the list of a root that carries both; see
    /// [`Parser::claim_root_events`].
    OwnsEvents,
}

/// A component tag: `<Card title="…">…</Card>`.
///
/// The call goes through the named-props companion `#[component]` generates
/// beside the function — a struct literal sharing its name — so a missing,
/// unknown or doubled prop fails with rustc's ordinary struct-literal
/// errors.
#[derive(Debug)]
pub struct ComponentCall {
    /// The function's name, spanned to the tag, so a component that does not
    /// resolve is reported where it was written.
    pub name: proc_macro2::Ident,
    /// Props in written order. Each value is a Rust expression: the literal
    /// as the author wrote it, or the contents of `@{…}` parenthesised.
    pub props: Vec<(proc_macro2::Ident, TokenStream)>,
    /// Props written as `{…}` — the client runtime's, not Rust's, and the
    /// reason a component tag can carry `onclick={save()}`.
    ///
    /// A component runs at render, so there is nothing of the browser in it
    /// to evaluate these against. What it gets instead is the source: the
    /// name and the expression as written, both as `&'static str`, which it
    /// is free to write back out onto whatever element it renders. That is
    /// what makes a component's event props behave like React's — the
    /// handler ends up on the DOM node, and the runtime evaluates it there,
    /// when it fires.
    ///
    /// They do not become fields of their own. All of them arrive as one
    /// `bindings: rahti::Bindings` — see `lib.rs` — so a component opts into
    /// the whole surface with one parameter instead of naming forty events.
    pub bindings: Vec<(String, String)>,
    /// `None` for `<Card />`: nothing is passed, so a component that takes
    /// `children` fails with "missing field" until the caller writes
    /// `<Card></Card>` — or passes some.
    pub children: Option<ChildBlock>,
}

/// The children between a component tag pair — a block of its own, exactly
/// as if the caller had written them in a separate `html! {r##"…"##}` and passed
/// that. The stamping that keeps the two shapes byte-identical happens in
/// `lib.rs`.
#[derive(Debug)]
pub struct ChildBlock {
    pub ops: Vec<Op>,
    /// Same as [`Parsed::component_slot`], for the child block's single
    /// root. `None` when the children have several roots — or none — in
    /// which case the fragment goes unstamped, the way an `Html::concat` or
    /// escaped-text child does today.
    pub component_slot: Option<(usize, usize, SlotKind)>,
    /// The author wrote a `pp-component` inside the children, so nothing is
    /// injected there.
    pub declares_component: bool,
    /// The block's own markup carries a client `{…}` binding — in text or as
    /// an element attribute value. Bindings inside a deeper component tag's
    /// children belong to that deeper block and are not counted here.
    ///
    /// This is what decides owner-scoped serialization: children that read
    /// the writing block's state ship inside `<template pp-owner="…">`, the
    /// shape the client runtime re-renders whenever the owner's state
    /// changes. See `stamp_component_children` in `lib.rs`.
    pub has_bindings: bool,
    /// The block carries its own `<script>`, so it is a self-contained
    /// reactive block: its bindings resolve against state it declares
    /// itself, and it keeps its own `pp-component` identity instead of an
    /// owner wrap.
    pub has_script: bool,
    /// Same as [`Parsed::root_owns_events`], for the child block's root.
    pub root_owns_events: bool,
}

/// Where a block's name lands, which decides how it is written there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlotKind {
    /// The root element's open tag — the name becomes a `pp-component`
    /// attribute.
    Element,
    /// A fragment's open marker — the name becomes the comment's text,
    /// after `pp:`.
    Fragment,
}

pub struct Error {
    pub span: Span,
    pub msg: String,
}

impl Error {
    pub fn to_compile_error(&self) -> TokenStream {
        let msg = &self.msg;
        let span = self.span;
        quote::quote_spanned! { span => { ::core::compile_error!(#msg) } }
    }
}

/// A parsed block, plus what the caller needs to stamp it.
pub struct Parsed {
    pub ops: Vec<Op>,
    /// Where the block's name goes: the index of the `Op::Static` holding
    /// the root's open tag or fragment marker, the byte offset the name is
    /// inserted at, and how it is written there.
    pub component_slot: Option<(usize, usize, SlotKind)>,
    /// The author wrote a `pp-component` somewhere in this block, so nothing
    /// gets injected — an explicit marker always wins.
    pub declares_component: bool,
    /// The root element carries a native event handler written in this
    /// block (`<form onsubmit={save(event)}>`), so the stamp names the block
    /// as its event owner too.
    ///
    /// The client runtime evaluates a component root's attributes in the
    /// PARENT's scope, because that is where forwarded props come from. A
    /// handler the block wrote on its own root belongs to the block, as
    /// `<form onSubmit={save}>` inside a React component does; without the
    /// owner it resolves against the parent and throws before it can run.
    pub root_owns_events: bool,
    /// The name a `#[component]` fn handed down, which beats the one derived
    /// from the file path. See [`component_directive`](Parser::component_directive).
    pub forced_component: Option<String>,
    /// The single parent element is `<html>`, so this block is a document —
    /// a root layout. Its slot is wrapped rather than its root element
    /// stamped; see `stamp` in `lib.rs`.
    pub renders_document: bool,
    /// Index of the first `Op::Slot`, which is where that wrapper goes.
    pub slot_at: Option<usize>,
}

/// The element a block has to render to count as a document.
const DOCUMENT_ROOT: &str = "html";

pub fn parse(input: TokenStream) -> Result<Parsed, Error> {
    let call_site = Span::call_site();
    let mut parser = Parser {
        toks: input.into_iter().collect(),
        pos: 0,
        last: call_site,
        depth: 0,
        roots: Vec::new(),
        component_slot: None,
        declares_component: false,
        root_owns_events: false,
        slot_at: None,
        client_bindings: 0,
        scripts: 0,
        root_attr_bindings: 0,
        root_binding: None,
        root_close_at: None,
        inner_spread: false,
    };
    let forced_component = parser.component_directive();
    let mut ops = Vec::new();
    parser.nodes(&mut ops)?;

    if let Some(tt) = parser.peek() {
        return Err(Error {
            span: tt.span(),
            msg: "rahti: unexpected `</` — closing tag without a matching opening tag".into(),
        });
    }

    check_single_root(&parser.roots, call_site)?;
    if forced_component.is_some() && !parser.declares_component {
        parser.reject_root_binding()?;
        parser.claim_scriptless_markup(&mut ops);
    }
    let renders_document = parser.roots.iter().any(
        |root| matches!(root, Root::Element(name, _) if name.eq_ignore_ascii_case(DOCUMENT_ROOT)),
    );
    Ok(Parsed {
        ops,
        component_slot: parser.component_slot,
        declares_component: parser.declares_component,
        root_owns_events: parser.root_owns_events,
        forced_component,
        renders_document,
        slot_at: parser.slot_at,
    })
}

/// Exactly one parent, optionally preceded by a doctype.
///
/// The parent is a literal element or a single component tag — composition,
/// the block rendering another component as its whole output. A document is
/// the one shape that must stay literal: the doctype prologue belongs to a
/// block whose parent is `<html>`, and a component root dissolves into
/// markup this block cannot see.
fn check_single_root(roots: &[Root], call_site: Span) -> Result<(), Error> {
    let parents: Vec<(String, Span)> = roots
        .iter()
        .filter_map(|r| match r {
            Root::Element(name, span) | Root::Component(name, span) => {
                Some((format!("<{name}>"), *span))
            }
            Root::Fragment(span) => Some(("<>…</>".to_string(), *span)),
            _ => None,
        })
        .collect();

    if let Some((_, span)) = parents.get(1) {
        let names = parents
            .iter()
            .map(|(n, _)| n.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error {
            span: *span,
            msg: format!(
                "rahti: `html!` renders a single parent element, and this block has {} \
                 ({names}).\n  Wrap them in one element — `<div>`, `<section>`, `<main>` \
                 — the way a React component returns a single parent.",
                parents.len()
            ),
        });
    }

    if let Some(Root::Loose(span, what)) = roots.iter().find(|r| matches!(r, Root::Loose(..))) {
        return Err(Error {
            span: *span,
            msg: format!(
                "rahti: {what} sits outside the parent element.\n  \
                 Everything a block renders goes inside its single parent element."
            ),
        });
    }

    if parents.is_empty() {
        return Err(Error {
            span: call_site,
            msg: "rahti: `html!` is empty — it must render one parent element.".into(),
        });
    }

    if roots.iter().any(|r| matches!(r, Root::Doctype)) {
        if let Some(Root::Component(name, span)) =
            roots.iter().find(|r| matches!(r, Root::Component(..)))
        {
            return Err(Error {
                span: *span,
                msg: format!(
                    "rahti: a document writes its root element literally, and `<{name}>` is \
                     a component.\n  A root layout's parent element is `<html>` — the \
                     doctype and the document element stay in the layout; components go \
                     inside them."
                ),
            });
        }
    }
    Ok(())
}

struct Parser {
    toks: Vec<TokenTree>,
    pos: usize,
    /// Span of the last token consumed, so end-of-input errors can point
    /// somewhere useful instead of at the whole macro.
    last: Span,
    /// How many elements deep we are. Nodes seen at 0 are roots.
    depth: usize,
    roots: Vec<Root>,
    component_slot: Option<(usize, usize, SlotKind)>,
    declares_component: bool,
    /// See [`Parsed::root_owns_events`].
    root_owns_events: bool,
    slot_at: Option<usize>,
    /// Client `{…}` bindings seen in the block being parsed — text mustaches
    /// and element attribute values. Saved and reset around a component
    /// tag's children, so each [`ChildBlock`] reports only its own.
    client_bindings: usize,
    /// `<script>` elements seen in the block being parsed. Same save/reset
    /// discipline as `client_bindings`.
    scripts: usize,
    /// The part of `client_bindings` written on the root's own attributes.
    /// Those are the caller's to evaluate, so they are not the block's own
    /// markup; see [`Parser::claim_scriptless_markup`].
    root_attr_bindings: usize,
    /// A non-event attribute of the root element whose value is a client
    /// `{…}` binding, with the root's span: the first one written.
    root_binding: Option<(String, String, Span)>,
    /// Index of the op closing the block's root — `</tag>` or a fragment's
    /// close marker — where a claimed script goes.
    root_close_at: Option<usize>,
    /// A spread sits on an element below the root. What it brings is only
    /// known at render, and may be the caller's `bindings`.
    inner_spread: bool,
}

impl Parser {
    fn peek(&self) -> Option<&TokenTree> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, n: usize) -> Option<&TokenTree> {
        self.toks.get(self.pos + n)
    }

    fn next(&mut self) -> Option<TokenTree> {
        let tt = self.toks.get(self.pos).cloned();
        if let Some(t) = &tt {
            self.last = t.span();
            self.pos += 1;
        }
        tt
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, Error> {
        Err(Error {
            span: self.peek().map_or(self.last, |t| t.span()),
            msg: msg.into(),
        })
    }

    fn is_punct(&self, n: usize, c: char) -> bool {
        matches!(self.peek_at(n), Some(TokenTree::Punct(p)) if p.as_char() == c)
    }

    /// Read a leading `@component "name"`, if there is one.
    ///
    /// Nobody writes this by hand — `#[component]` injects it in front of the
    /// markup so the block knows the name of the function it was written in,
    /// which is the one thing a proc macro cannot see from the inside. `@`
    /// opens nothing in the grammar, so a block that starts with one is
    /// unambiguously the attribute talking.
    fn component_directive(&mut self) -> Option<String> {
        if !self.is_punct(0, '@') {
            return None;
        }
        match (self.peek_at(1), self.peek_at(2)) {
            (Some(TokenTree::Ident(id)), Some(TokenTree::Literal(lit))) if id == "component" => {
                let name = unquote(&lit.to_string())?;
                self.next();
                self.next();
                self.next();
                Some(name)
            }
            _ => None,
        }
    }

    /// Consume nodes until end of input or a closing tag.
    fn nodes(&mut self, ops: &mut Vec<Op>) -> Result<(), Error> {
        loop {
            match self.peek() {
                None => return Ok(()),
                // `</` belongs to our caller.
                Some(TokenTree::Punct(p)) if p.as_char() == '<' && self.is_punct(1, '/') => {
                    return Ok(());
                }
                Some(TokenTree::Punct(p)) if p.as_char() == '<' => self.element(ops)?,
                // `@{expr}` — a Rust expression, rendered here.
                Some(TokenTree::Punct(p)) if p.as_char() == '@' && self.brace_at(1) => {
                    self.next(); // '@'
                    let Some(TokenTree::Group(g)) = self.next() else {
                        unreachable!("checked by `brace_at`");
                    };
                    self.mark_root(Root::Loose(g.span(), "`@{expr}`"));
                    ops.push(Op::Text(g.stream()));
                }
                // `{expr}` — the client runtime's, copied through untouched.
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    let source = binding_source(g);
                    let span = g.span();
                    self.next();
                    self.mark_root(Root::Loose(span, "`{expr}`"));
                    self.client_bindings += 1;
                    ops.push(Op::Static(escape_text(&source)));
                }
                // A run that is exactly one string literal keeps its contents
                // verbatim: quoting is how you write text the surrounding
                // whitespace rules would otherwise reshape.
                Some(TokenTree::Literal(_)) if self.lone_string() => {
                    let span = self.span_now();
                    let text = self.literal_text()?;
                    self.mark_root(Root::Loose(span, "this text"));
                    ops.push(Op::Static(escape_text(&text)));
                }
                Some(other) => {
                    return Err(Error {
                        span: other.span(),
                        msg: format!("rahti: unexpected `{other}` in a template"),
                    });
                }
            }
        }
    }

    /// Is the node starting here a single string literal, and nothing else?
    fn lone_string(&self) -> bool {
        let quoted = matches!(self.peek(), Some(TokenTree::Literal(l))
            if l.to_string().starts_with(['"', 'r']));

        quoted && self.ends_text_run(1)
    }

    /// Is the token `n` ahead a `{…}` group?
    fn brace_at(&self, n: usize) -> bool {
        matches!(self.peek_at(n), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace)
    }

    /// Does the braced group `n` ahead open with the spread's `..`?
    ///
    /// Only asked where both a spread and a plain `@{expr}` could stand, so
    /// that each gets the message written for it.
    fn spread_at(&self, n: usize) -> bool {
        let Some(TokenTree::Group(g)) = self.peek_at(n) else {
            return false;
        };
        g.stream()
            .into_iter()
            .take(2)
            .filter(|tt| matches!(tt, TokenTree::Punct(p) if p.as_char() == '.'))
            .count()
            == 2
    }

    /// Does the token `n` ahead close off a run of text?
    fn ends_text_run(&self, n: usize) -> bool {
        match self.peek_at(n) {
            None => true,
            // A bare `@` is text — `@media`, an email address. It only ends a
            // run when it is the one that opens `@{expr}`.
            Some(TokenTree::Punct(p)) => {
                p.as_char() == '<' || (p.as_char() == '@' && self.brace_at(n + 1))
            }
            Some(TokenTree::Group(g)) => g.delimiter() == Delimiter::Brace,
            _ => false,
        }
    }

    /// Note a node that turned out to sit at the top level.
    fn mark_root(&mut self, root: Root) {
        if self.depth == 0 {
            self.roots.push(root);
        }
    }

    fn element(&mut self, ops: &mut Vec<Op>) -> Result<(), Error> {
        let open = self.span_now();
        self.next(); // '<'

        if self.is_punct(0, '!') {
            self.mark_root(Root::Doctype);
            return self.doctype(ops);
        }

        if self.is_punct(0, '>') {
            return self.fragment(ops, open);
        }

        let name = self.tag_name()?;

        if name == "slot" {
            self.mark_root(Root::Loose(open, "`<slot />`"));
            return self.slot(ops, open);
        }

        // A capital letter opens a component, exactly as in JSX: no HTML tag
        // starts uppercase, and a custom element must carry a hyphen.
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return self.component(name, open, ops);
        }

        self.mark_root(Root::Element(name.clone(), open));

        // The open tag below lands in `ops` at the current length: the only
        // pushes between here and then come from attribute interpolation,
        // and the first of those flushes exactly this markup.
        let is_root = self.depth == 0 && self.component_slot.is_none();
        if is_root {
            self.component_slot = Some((ops.len(), 1 + name.len(), SlotKind::Element));
        }

        let mut markup = format!("<{name}");
        let bindings_before = self.client_bindings;
        // The template lexer refused JSX names and unknown directives, with
        // their position, before these tokens were made.
        let mut attrs = self.tag_attributes()?;
        if is_root {
            self.root_attr_bindings = self.client_bindings - bindings_before;
            self.root_binding = attrs.iter().find_map(|attr| match attr {
                TagAttr::Literal {
                    name: attr_name,
                    value: Some(value),
                } if is_scope_binding(attr_name, value) => {
                    Some((attr_name.clone(), value.clone(), open))
                }
                _ => None,
            });
            self.claim_root_events(&mut attrs);
        } else if attrs.iter().any(|attr| matches!(attr, TagAttr::Spread(_))) {
            self.inner_spread = true;
        }
        emit_attributes(attrs, &mut markup, ops);

        let void = VOID.contains(&name.as_str());
        let self_closing = self.is_punct(0, '/');

        if self_closing {
            self.next(); // '/'
            if !self.is_punct(0, '>') {
                return self.err("rahti: expected `>` after `/`");
            }
            self.next();
            // HTML5 has no self-closing syntax for non-void tags; the parser
            // would treat `<div />` as an unclosed `<div>`. Emit both tags.
            markup.push('>');
            if !void {
                markup.push_str(&format!("</{name}>"));
            }
            ops.push(Op::Static(markup));
            return Ok(());
        }

        if !self.is_punct(0, '>') {
            return self.err(format!("rahti: expected `>` or `/>` to close `<{name}`"));
        }
        self.next();
        markup.push('>');
        ops.push(Op::Static(markup));

        if void {
            return Ok(());
        }

        if RAW_TEXT.contains(&name.as_str()) {
            self.raw_text(&name, ops)?;
        } else {
            self.depth += 1;
            self.nodes(ops)?;
            self.depth -= 1;
        }

        // `</name>`
        if !(self.is_punct(0, '<') && self.is_punct(1, '/')) {
            return Err(Error {
                span: open,
                msg: format!("rahti: `<{name}>` is never closed"),
            });
        }
        self.next();
        self.next();
        let closing = self.tag_name()?;
        if closing != name {
            return Err(Error {
                span: self.last,
                msg: format!("rahti: `<{name}>` is closed by `</{closing}>`"),
            });
        }
        if !self.is_punct(0, '>') {
            return self.err("rahti: expected `>` to finish the closing tag");
        }
        self.next();
        ops.push(Op::Static(format!("</{name}>")));
        if is_root && !RAW_TEXT.contains(&name.as_str()) {
            self.root_close_at = Some(ops.len() - 1);
        }
        Ok(())
    }

    /// The body of a `<script>` or `<style>`, up to its closing tag.
    ///
    /// HTML calls these raw text elements: nothing inside is markup, and
    /// `&lt;` inside a script is a literal `&lt;`. The template lexer hands the
    /// body over as one raw string literal, so it arrives exactly as written —
    /// comments, quotes and template literals included — with no `{…}`
    /// binding, which in JavaScript is a block and in CSS is a rule.
    ///
    /// One form *is* interpolated: `@{expr}` inside `<script>` renders the
    /// Rust expression as JavaScript source through `::rahti::RenderJs`, the
    /// script-side counterpart of `Render`. Primitives spell themselves via
    /// `Display`, strings quote themselves as JSON literals, and anything
    /// larger (`Json(&v)`) serialises through `serde_json`. That is the door
    /// page-side data walks through to seed `pp.state(…)` with values known
    /// at render. `@@{` is a literal `@{`. See [`raw_script`].
    ///
    /// `<style>` is copied as written: `@media` and the brace of a
    /// `@keyframes` block are CSS, not Rust.
    fn raw_text(&mut self, name: &str, ops: &mut Vec<Op>) -> Result<(), Error> {
        if name == "script" {
            self.scripts += 1;
        }
        let first = self.pos;
        while !self.closes_raw_text(name) {
            if self.next().is_none() {
                return self.err(format!("rahti: `<{name}>` is never closed"));
            }
        }
        let body = &self.toks[first..self.pos];
        let contents = match body {
            [] => return Ok(()),
            [TokenTree::Literal(lit)] => raw_string_contents(&lit.to_string()).map(|c| (c, lit.span())),
            _ => None,
        };
        let Some((contents, span)) = contents else {
            return self.err(format!("rahti: unexpected tokens in the `<{name}>` body"));
        };
        if name == "script" {
            return raw_script(&contents, span, ops);
        }
        ops.push(Op::Static(contents));
        Ok(())
    }

    /// Are we sitting on the `</script>` that ends the body?
    fn closes_raw_text(&self, name: &str) -> bool {
        self.is_punct(0, '<')
            && self.is_punct(1, '/')
            && matches!(self.peek_at(2), Some(TokenTree::Ident(id)) if *id == name)
    }

    /// `<!DOCTYPE html>` and friends. Emitted verbatim, words space-separated.
    fn doctype(&mut self, ops: &mut Vec<Op>) -> Result<(), Error> {
        self.next(); // '!'
        let mut words = Vec::new();
        loop {
            match self.next() {
                Some(TokenTree::Punct(p)) if p.as_char() == '>' => break,
                Some(TokenTree::Ident(id)) => words.push(id.to_string()),
                Some(TokenTree::Literal(l)) => words.push(l.to_string()),
                Some(_) => return self.err("rahti: unexpected token in doctype"),
                None => return self.err("rahti: unterminated doctype — expected `>`"),
            }
        }
        ops.push(Op::Static(format!("<!{}>", words.join(" "))));
        Ok(())
    }

    /// `<slot />` or `<slot></slot>` — where the wrapped content goes.
    fn slot(&mut self, ops: &mut Vec<Op>, open: Span) -> Result<(), Error> {
        if self.is_punct(0, '/') {
            self.next();
            if !self.is_punct(0, '>') {
                return self.err("rahti: expected `>` after `<slot /`");
            }
            self.next();
        } else if self.is_punct(0, '>') {
            self.next();
            // Tolerate the paired form; anything between the tags is ignored,
            // so reject it rather than silently dropping it.
            if !(self.is_punct(0, '<') && self.is_punct(1, '/')) {
                return Err(Error {
                    span: open,
                    msg: "rahti: `<slot>` takes no children — write `<slot />`".into(),
                });
            }
            self.next();
            self.next();
            let closing = self.tag_name()?;
            if closing != "slot" || !self.is_punct(0, '>') {
                return self.err("rahti: expected `</slot>`");
            }
            self.next();
        } else {
            return Err(Error {
                span: open,
                msg: "rahti: `<slot />` takes no attributes".into(),
            });
        }
        self.slot_at.get_or_insert(ops.len());
        ops.push(Op::Slot);
        Ok(())
    }

    /// `<> … </>` — a fragment: several roots for one block.
    ///
    /// JSX's answer to a block that renders siblings. It ships as the
    /// comment pair in [`FRAGMENT_OPEN`]/[`FRAGMENT_CLOSE`] — legal in every
    /// content context, so a fragment of `<tr>`s or `<li>`s survives the
    /// browser's parser. At mount the runtime turns the pair into a live
    /// layout-invisible boundary: that is where the block's name lands, its
    /// scripts live, and its re-renders happen, while the siblings sit in
    /// the parent's layout as if nothing held them.
    ///
    /// A fragment is a *root* — of an `html!` block, or of the children
    /// between a component's tags. Anywhere deeper, siblings need no wrapper
    /// at all, so the form is refused.
    fn fragment(&mut self, ops: &mut Vec<Op>, open: Span) -> Result<(), Error> {
        if self.depth > 0 {
            return Err(Error {
                span: open,
                msg: "rahti: `<>` opens a fragment, and a fragment is a block's root.\n  \
                      Inside an element, siblings need no wrapper — write them directly.\n  \
                      For a literal less-than sign in text, write `&lt;`."
                    .into(),
            });
        }
        self.next(); // '>'
        self.mark_root(Root::Fragment(open));

        let is_root = self.component_slot.is_none();
        if is_root {
            self.component_slot = Some((ops.len(), FRAGMENT_NAME_AT, SlotKind::Fragment));
        }
        ops.push(Op::Static(FRAGMENT_OPEN.to_string()));

        self.depth += 1;
        self.nodes(ops)?;
        self.depth -= 1;

        // `</>`
        if !(self.is_punct(0, '<') && self.is_punct(1, '/') && self.is_punct(2, '>')) {
            return Err(Error {
                span: open,
                msg: "rahti: `<>` is never closed — expected `</>`".into(),
            });
        }
        self.next();
        self.next();
        self.next();
        ops.push(Op::Static(FRAGMENT_CLOSE.to_string()));
        if is_root {
            self.root_close_at = Some(ops.len() - 1);
        }
        Ok(())
    }

    /// `<Card title="…" count=@{n}>…</Card>` — a component call.
    ///
    /// The tag names a `#[component]` function; props name its parameters.
    /// Children, when the tag has any, are parsed as a block of their own —
    /// the same block an inline `html! {r##"…"##}` child argument is — and
    /// become the `children` argument. A self-closing tag passes no
    /// `children` at all, which is the shape for a component that declares
    /// none.
    fn component(&mut self, name: String, open: Span, ops: &mut Vec<Op>) -> Result<(), Error> {
        let name_span = self.last;
        if name.contains('-') {
            return Err(Error {
                span: name_span,
                msg: format!(
                    "rahti: `<{name}>` starts with a capital but carries a `-`.\n  \
                     A component is a PascalCase Rust identifier; a custom element is \
                     lowercase."
                ),
            });
        }
        self.mark_root(Root::Component(name.clone(), open));
        let ident = proc_macro2::Ident::new(&name, name_span);
        let (props, bindings) = self.component_props(&name)?;

        if self.is_punct(0, '/') {
            self.next(); // '/'
            if !self.is_punct(0, '>') {
                return self.err("rahti: expected `>` after `/`");
            }
            self.next();
            ops.push(Op::Component(ComponentCall {
                name: ident,
                props,
                bindings,
                children: None,
            }));
            return Ok(());
        }

        if !self.is_punct(0, '>') {
            return self.err(format!("rahti: expected `>` or `/>` to close `<{name}`"));
        }
        self.next();

        // The children are a fresh block: their own roots, their own name
        // slot. Save this block's bookkeeping and hand the parser a clean
        // slate, the way a nested `html! {r##"…"##}` would have started with one.
        let outer_roots = std::mem::take(&mut self.roots);
        let outer_slot = self.component_slot.take();
        let outer_declares = std::mem::replace(&mut self.declares_component, false);
        let outer_owns_events = std::mem::replace(&mut self.root_owns_events, false);
        let outer_slot_at = self.slot_at.take();
        let outer_depth = std::mem::replace(&mut self.depth, 0);
        let outer_bindings = std::mem::replace(&mut self.client_bindings, 0);
        let outer_scripts = std::mem::replace(&mut self.scripts, 0);
        let outer_root_attr_bindings = std::mem::replace(&mut self.root_attr_bindings, 0);
        let outer_root_binding = self.root_binding.take();
        let outer_root_close_at = self.root_close_at.take();
        let outer_inner_spread = std::mem::replace(&mut self.inner_spread, false);

        let mut child_ops = Vec::new();
        let children = self.nodes(&mut child_ops);

        let child_slot = self.component_slot;
        let child_declares = self.declares_component;
        let child_owns_events = self.root_owns_events;
        // A deeper component tag inside these children saved and reset the
        // counters around its own child block, so what is left here counts
        // this block's markup alone.
        let child_bindings = self.client_bindings;
        let child_scripts = self.scripts;
        let single_root = matches!(
            self.roots.as_slice(),
            [Root::Element(..)] | [Root::Fragment(..)]
        );

        self.roots = outer_roots;
        self.component_slot = outer_slot;
        self.declares_component = outer_declares;
        self.root_owns_events = outer_owns_events;
        self.slot_at = outer_slot_at;
        self.depth = outer_depth;
        self.client_bindings = outer_bindings;
        self.scripts = outer_scripts;
        self.root_attr_bindings = outer_root_attr_bindings;
        self.root_binding = outer_root_binding;
        self.root_close_at = outer_root_close_at;
        self.inner_spread = outer_inner_spread;
        children?;

        if !(self.is_punct(0, '<') && self.is_punct(1, '/')) {
            return Err(Error {
                span: open,
                msg: format!("rahti: `<{name}>` is never closed"),
            });
        }
        self.next();
        self.next();
        let closing = self.tag_name()?;
        if closing != name {
            return Err(Error {
                span: self.last,
                msg: format!("rahti: `<{name}>` is closed by `</{closing}>`"),
            });
        }
        if !self.is_punct(0, '>') {
            return self.err("rahti: expected `>` to finish the closing tag");
        }
        self.next();

        ops.push(Op::Component(ComponentCall {
            name: ident,
            props,
            bindings,
            children: Some(ChildBlock {
                ops: child_ops,
                component_slot: if single_root { child_slot } else { None },
                declares_component: child_declares,
                has_bindings: child_bindings > 0,
                has_script: child_scripts > 0,
                root_owns_events: child_owns_events && single_root,
            }),
        }));
        Ok(())
    }

    /// The props of a component tag, in written order.
    ///
    /// A prop value is a Rust expression: a literal as written (`title="…"`,
    /// `count=3`), or `@{expr}`. A bare prop is JSX's boolean shorthand —
    /// `<Card wide>` passes `wide: true`.
    ///
    /// A `{…}` value is the client runtime's, and is kept as source rather
    /// than evaluated: it comes back in the second return value and reaches
    /// the component as a `bindings` entry. See [`ComponentCall::bindings`].
    #[allow(clippy::type_complexity)]
    fn component_props(
        &mut self,
        name: &str,
    ) -> Result<
        (
            Vec<(proc_macro2::Ident, TokenStream)>,
            Vec<(String, String)>,
        ),
        Error,
    > {
        let mut props = Vec::new();
        let mut bindings = Vec::new();
        loop {
            if self.peek().is_none() {
                return self.err(format!(
                    "rahti: unterminated `<{name}` — expected `>` or `/>`"
                ));
            }
            if self.is_punct(0, '>') || (self.is_punct(0, '/') && self.is_punct(1, '>')) {
                return Ok((props, bindings));
            }

            // A spread is for elements, where a name is just markup. A
            // component's props are typed parameters, and an attribute set
            // has no way to know which of them it is filling.
            if self.is_punct(0, '@') && self.brace_at(1) && self.spread_at(1) {
                return self.err(format!(
                    "rahti: `@{{..attrs}}` spreads attributes onto an element, and `<{name}>` \
                     is a component.\n  A component's props are its parameters, so they are \
                     named: `<{name} class=\"…\" />`. To forward a set of attributes, give \
                     `{name}` an `attrs: ::rahti::Attrs` prop, pass it by name, and spread it \
                     onto the element `{name}` renders."
                ));
            }

            let prop = match self.next() {
                Some(TokenTree::Ident(id)) => id,
                Some(other) => {
                    return Err(Error {
                        span: other.span(),
                        msg: format!(
                            "rahti: expected a prop name on `<{name}`.\n  \
                             A prop names a parameter of `{name}`, so it is a Rust \
                             identifier."
                        ),
                    });
                }
                None => unreachable!("checked by the peek above"),
            };
            if self.is_punct(0, '-') {
                return self.err(format!(
                    "rahti: a prop of `<{name}>` names one of its parameters, and a Rust \
                     identifier has no `-`."
                ));
            }

            if !self.is_punct(0, '=') {
                // `<Card wide>` — JSX's boolean shorthand.
                props.push((prop, quote::quote! { true }));
                continue;
            }
            self.next(); // '='

            match self.peek().cloned() {
                Some(TokenTree::Literal(lit)) => {
                    self.next();
                    props.push((prop, TokenTree::Literal(lit).into()));
                }
                // `@{expr}` — parenthesised, so the expression stays one value.
                Some(TokenTree::Punct(p)) if p.as_char() == '@' && self.brace_at(1) => {
                    self.next(); // '@'
                    let Some(TokenTree::Group(g)) = self.next() else {
                        unreachable!("checked by `brace_at`");
                    };
                    let mut value = proc_macro2::Group::new(Delimiter::Parenthesis, g.stream());
                    value.set_span(g.span());
                    props.push((prop, TokenTree::Group(value).into()));
                }
                // `{expr}` — the client runtime's. Kept as the source the
                // author wrote, for the component to put back on an element.
                //
                // Handed over without the braces, unlike an element's
                // attribute: those go straight into markup, where the braces
                // are what the runtime looks for, while a component receives
                // an expression and chooses where it lands.
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    self.next();
                    // The expression lands on the component's root, which the
                    // runtime evaluates in the scope that wrote the tag — so
                    // it is this block's binding, the same as one written on
                    // an element. Uncounted, `<Card><Input value={q} /></Card>`
                    // passes for static children and loses its owner.
                    self.client_bindings += 1;
                    let source = binding_source(&g);
                    let expr = source
                        .strip_prefix('{')
                        .and_then(|rest| rest.strip_suffix('}'))
                        .unwrap_or(&source)
                        .trim()
                        .to_string();
                    bindings.push((prop.to_string(), expr));
                }
                _ => {
                    return self.err(format!(
                        "rahti: `{prop}` needs a literal or `@{{expr}}` as its value"
                    ));
                }
            }
        }
    }

    /// Note whether the root element handles its own events.
    ///
    /// A handler written literally on the tag — `onsubmit={save(event)}` or
    /// `onsubmit="save(event)"` — is this block's source, so the block owns
    /// it. A caller's handler can only arrive at runtime, through a spread of
    /// its `Bindings` (an `on…=@{expr}` is refused while parsing), and one
    /// element has one event owner: a spread beside an owned handler is
    /// marked, so the open tag built at runtime refuses an event handler the
    /// spread brings (see `rahti::AttrList::own_events`).
    fn claim_root_events(&mut self, attrs: &mut Vec<TagAttr>) {
        let owns = attrs.iter().any(
            |attr| matches!(attr, TagAttr::Literal { name, .. } if is_native_event_name(name)),
        );
        if !owns {
            return;
        }
        self.root_owns_events = true;
        if attrs.iter().any(|attr| matches!(attr, TagAttr::Spread(_))) {
            attrs.insert(0, TagAttr::OwnsEvents);
        }
    }

    /// Refuse a client binding a component wrote on its own root element.
    ///
    /// The client runtime evaluates a component root's attributes in the
    /// CALLER's scope — that is where the caller's forwarded `bindings`
    /// belong — so `<section hidden={!open}>` in a component reads the
    /// caller's `open`, never the component's own state or `pp.props`, and
    /// renders a wrong value with no error. Handlers are the exception, owned
    /// through `pp-event-owner` (see [`Parser::claim_root_events`]).
    fn reject_root_binding(&self) -> Result<(), Error> {
        let Some((name, value, span)) = &self.root_binding else {
            return Ok(());
        };
        Err(Error {
            span: *span,
            msg: format!(
                "rahti: this component binds `{name}={value}` on its own root element.\n  \
                 The client runtime evaluates a component root's attributes in the \
                 caller's scope, so this cannot read the component's own state or \
                 `pp.props`. Bind an element inside the root, or let the caller pass \
                 it: `<Component {name}={{…}}>` arrives in `bindings` for the root."
            ),
        })
    }

    /// Give a script-less component that has bindings of its own a
    /// `<script>`.
    ///
    /// The client runtime compiles a boundary without a script in its
    /// PARENT's scope: that is what lets a hand-written as-child boundary in
    /// a parent's `pp-for` bake the loop variable in. A component's own
    /// markup is its own, though — `<p>{pp.props.label}</p>` must read this
    /// component's props — so an empty script is written before the root's
    /// closing tag, and the runtime compiles the block in its own scope.
    ///
    /// Counted: client bindings in the block's markup below the root. Not
    /// counted: the root's attributes (the caller's, see
    /// [`Parser::reject_root_binding`]), children between component tags (a
    /// block of their own), and anything a spread below the root brings at
    /// render — which may be the caller's `bindings`, so a block with such a
    /// spread keeps the runtime's rule.
    fn claim_scriptless_markup(&self, ops: &mut Vec<Op>) {
        if self.scripts > 0 || self.inner_spread {
            return;
        }
        if self.client_bindings <= self.root_attr_bindings {
            return;
        }
        if let Some(at) = self.root_close_at {
            ops.insert(at, Op::Static("<script></script>".into()));
        }
    }

    /// Everything between the tag name and the `>`, in written order.
    ///
    /// Collected before any of it is emitted, because one attribute decides
    /// how all of them are written: a `@{..spread}` anywhere in the tag means
    /// the open tag is assembled at runtime. See [`emit_attributes`].
    fn tag_attributes(&mut self) -> Result<Vec<TagAttr>, Error> {
        let mut attrs = Vec::new();

        loop {
            if self.peek().is_none() {
                return self.err("rahti: unterminated tag — expected `>` or `/>`");
            }
            if self.is_punct(0, '>') || (self.is_punct(0, '/') && self.is_punct(1, '>')) {
                return Ok(attrs);
            }

            // `@{..expr}` — a spread, which stands where a name would.
            if self.is_punct(0, '@') && self.brace_at(1) {
                let span = self.span_now();
                self.next(); // '@'
                let Some(TokenTree::Group(g)) = self.next() else {
                    unreachable!("checked by `brace_at`");
                };
                attrs.push(TagAttr::Spread(spread_expr(&g, span)?));
                continue;
            }

            let name = self.attr_name()?;
            if name == COMPONENT_ATTR {
                self.declares_component = true;
            }

            if !self.is_punct(0, '=') {
                // Boolean attribute: `<input required>`.
                attrs.push(TagAttr::Literal { name, value: None });
                continue;
            }
            self.next(); // '='

            match self.peek().cloned() {
                Some(TokenTree::Literal(_)) => {
                    let span = self.span_now();
                    let value = self.literal_text()?;
                    if name == COMPONENT_ATTR && !crate::component::is_valid_js_identifier(&value) {
                        return Err(Error {
                            span,
                            msg: format!(
                                "rahti: `{COMPONENT_ATTR}=\"{value}\"` is not a valid JavaScript \
                                 identifier, and the client runtime binds it as a variable \
                                 name.\n  Use letters, digits, `_` and `$`, starting with a \
                                 letter, `_` or `$` — `{}` would work.",
                                suggest_identifier(&value)
                            ),
                        });
                    }
                    attrs.push(TagAttr::Literal {
                        name,
                        value: Some(value),
                    });
                }
                // `@{expr}` — a Rust expression. Whether it produces an
                // attribute at all is the value's to say, so the name travels
                // with it.
                Some(TokenTree::Punct(p)) if p.as_char() == '@' && self.brace_at(1) => {
                    self.next(); // '@'
                    let Some(TokenTree::Group(g)) = self.next() else {
                        unreachable!("checked by `brace_at`");
                    };
                    let boolean = is_boolean_attribute(&name);
                    attrs.push(TagAttr::Value {
                        name,
                        expr: g.stream(),
                        boolean,
                    });
                }
                // `{expr}` — the client runtime binds this attribute.
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    self.next();
                    self.client_bindings += 1;
                    attrs.push(TagAttr::Literal {
                        name,
                        value: Some(binding_source(&g)),
                    });
                }
                _ => {
                    return self.err(format!(
                        "rahti: `{name}` needs a string literal, `{{expr}}` or `@{{expr}}` as \
                         its value"
                    ));
                }
            }
        }
    }

    /// `div`, `my-element` — idents joined by `-`.
    fn tag_name(&mut self) -> Result<String, Error> {
        let mut name = match self.next() {
            Some(TokenTree::Ident(id)) => id.to_string(),
            // A `<` always opens a tag, exactly as in HTML, so text that
            // needs the character has to spell it out.
            _ => {
                return self.err(
                    "rahti: expected a tag name after `<`.\n  \
                     Write `&lt;` for a less-than sign in text.",
                );
            }
        };
        // `-` joins custom-element names; `.` joins a context provider's,
        // `<themecontext.provider value={theme}>`, the tag PulsePoint reads
        // as `ThemeContext`'s provider.
        while self.is_punct(0, '-') || self.is_punct(0, '.') {
            let joiner = if self.is_punct(0, '-') { '-' } else { '.' };
            self.next();
            match self.next() {
                Some(TokenTree::Ident(id)) => {
                    name.push(joiner);
                    name.push_str(&id.to_string());
                }
                _ => return self.err(format!("rahti: expected a name after `{joiner}`")),
            }
        }
        Ok(name)
    }

    /// `class`, `http-equiv`, `xlink:href`, `xml:space` — idents joined by
    /// `-` or `:`.
    ///
    /// Two things a tag name does not need. `data-*` and `aria-*` may end in
    /// digits (`aria-level-2`), which arrive as literals rather than idents.
    /// And a `:` separates a namespace prefix from the attribute under it,
    /// which is how SVG carries `xlink:href` and how an icon exported from a
    /// drawing program carries `xml:space`.
    fn attr_name(&mut self) -> Result<String, Error> {
        let mut name = match self.next() {
            Some(TokenTree::Ident(id)) => id.to_string(),
            Some(other) => {
                return Err(Error {
                    span: other.span(),
                    msg: "rahti: expected an attribute name".into(),
                });
            }
            None => return self.err("rahti: expected an attribute name"),
        };
        while self.is_punct(0, '-') || self.is_punct(0, ':') {
            let separator = if self.is_punct(0, '-') { '-' } else { ':' };
            self.next();
            match self.next() {
                Some(TokenTree::Ident(id)) => {
                    name.push(separator);
                    name.push_str(&id.to_string());
                }
                Some(TokenTree::Literal(l)) => {
                    name.push(separator);
                    name.push_str(&l.to_string());
                }
                _ => {
                    return self.err(format!("rahti: expected a name after `{separator}`"));
                }
            }
        }
        Ok(name)
    }

    /// The value of a literal token: string contents unescaped, anything
    /// else (numbers, `true`) rendered as written.
    fn literal_text(&mut self) -> Result<String, Error> {
        let lit = match self.next() {
            Some(TokenTree::Literal(l)) => l,
            _ => return self.err("rahti: expected a literal"),
        };
        let repr = lit.to_string();
        Ok(unquote(&repr).unwrap_or(repr))
    }

    fn span_now(&self) -> Span {
        self.peek().map_or(self.last, |t| t.span())
    }
}

// --------------------------------------------------------------- attributes

/// Turn a tag's attributes into writes.
///
/// Two ways, and which one is used is decided by whether a spread was written.
///
/// Without one, every attribute goes where it always went: names and literal
/// values fold into `markup`, which becomes a single `push_str`, and only an
/// `@{…}` survives as an op. The one change from writing the whole thing
/// statically is that a dynamic attribute takes its name with it, since the
/// value is what decides whether the attribute exists.
///
/// With one, the tag is built at runtime through an `AttrList`. That is what a
/// spread needs to mean what it means: writing `class` twice and leaving the
/// browser to take the first would make `<svg class="a" @{..attrs}>` ignore
/// the spread, which is the opposite of what the same line does in JSX. Only
/// tags that carry a spread pay for it.
fn emit_attributes(attrs: Vec<TagAttr>, markup: &mut String, ops: &mut Vec<Op>) {
    if !attrs.iter().any(|a| matches!(a, TagAttr::Spread(_))) {
        for attr in attrs {
            match attr {
                TagAttr::Literal { name, value } => {
                    markup.push(' ');
                    markup.push_str(&name);
                    if let Some(value) = value {
                        markup.push_str("=\"");
                        markup.push_str(&escape_attr(&value));
                        markup.push('"');
                    }
                }
                TagAttr::Value {
                    name,
                    expr,
                    boolean,
                } => {
                    ops.push(Op::Static(std::mem::take(markup)));
                    ops.push(Op::Attr {
                        name: format!(" {name}"),
                        expr,
                        boolean,
                    });
                }
                TagAttr::Spread(_) => unreachable!("checked above"),
                // Only put in a list that carries a spread.
                TagAttr::OwnsEvents => {}
            }
        }
        return;
    }

    // The tag name is already in `markup`, and it stays static: a spread
    // cannot rename an element, and the block's `pp-component` is written
    // into this same chunk.
    ops.push(Op::Static(std::mem::take(markup)));
    ops.push(Op::OpenTag(attrs));
}

/// The expression inside `@{..expr}`, or an error explaining what was meant.
///
/// `..` is Rust's own spread — the token that finishes a struct literal — so a
/// Rust author reads `@{..attrs}` the way they read `..Default::default()`.
/// JSX spells it `{...attrs}`, which cannot be borrowed here: `{…}` in a tag
/// is a client binding, and the two dialects never trade places. The
/// three-dot spelling is recognised only to say so.
fn spread_expr(g: &proc_macro2::Group, span: Span) -> Result<TokenStream, Error> {
    let toks: Vec<TokenTree> = g.stream().into_iter().collect();
    let dots = toks
        .iter()
        .take_while(|tt| matches!(tt, TokenTree::Punct(p) if p.as_char() == '.'))
        .count();

    if dots == 3 {
        return Err(Error {
            span: g.span(),
            msg: "rahti: a spread is `@{..attrs}` — two dots, the way a Rust struct \
                  literal spreads.\n  `{...attrs}` is JSX's spelling, and `{…}` in a tag \
                  belongs to the client runtime."
                .into(),
        });
    }
    if dots != 2 {
        return Err(Error {
            span,
            msg: "rahti: expected an attribute name, or a spread.\n  \
                  A Rust value goes in an attribute's value — `class=@{name}` — and \
                  `@{..attrs}` spreads a set of them onto the element."
                .into(),
        });
    }

    let expr: TokenStream = toks[2..].iter().cloned().collect();
    if expr.is_empty() {
        return Err(Error {
            span: g.span(),
            msg: "rahti: `@{..}` spreads nothing. Name what it spreads — `@{..attrs}`.".into(),
        });
    }
    Ok(expr)
}

// ------------------------------------------------------------------ script
// A script body written as a raw string: JavaScript verbatim, `@{…}` lifted.

/// The contents of a raw string literal (`r"…"`, `r#"…"#`, …), or `None` for
/// any other literal. Raw strings only: the body of a plain `"…"` would need
/// unescaping, and a lone string *is* valid JavaScript.
fn raw_string_contents(repr: &str) -> Option<String> {
    if !repr.starts_with('r') {
        return None;
    }
    unquote(repr)
}

/// Emit a raw-string script body: the JavaScript as written, with each
/// `@{expr}` parsed as a Rust expression and lifted into an
/// [`Op::ScriptText`].
///
/// Finding the end of an `@{…}` counts braces, skipping Rust string and char
/// literals so `@{format!("{}", x)}` ends where it should.
fn raw_script(js: &str, span: Span, ops: &mut Vec<Op>) -> Result<(), Error> {
    let mut run = String::new();
    let mut rest = js;
    while let Some(at) = rest.find("@{") {
        // `@@{` is an escaped, literal `@{`.
        if rest[..at].ends_with('@') {
            run.push_str(&rest[..at - 1]);
            run.push_str("@{");
            rest = &rest[at + 2..];
            continue;
        }
        run.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = closing_brace(after) else {
            return Err(Error {
                span,
                msg: "rahti: `@{` in a raw-string `<script>` is never closed — \
                      write `@{expr}`, or `@@{` for a literal `@{`"
                    .into(),
            });
        };
        let source = after[..end].trim();
        let expr = TokenStream::from_str(source).map_err(|_| Error {
            span,
            msg: format!("rahti: `@{{{source}}}` in a `<script>` is not a Rust expression"),
        })?;
        if expr.is_empty() {
            return Err(Error {
                span,
                msg: "rahti: `@{}` in a `<script>` is empty — write `@{expr}`".into(),
            });
        }
        flush_text(&mut run, ops);
        ops.push(Op::ScriptText(expr));
        rest = &after[end + 1..];
    }
    run.push_str(rest);
    flush_text(&mut run, ops);
    Ok(())
}

/// [`closing_brace`], for the template lexer's `@{…}`.
pub fn closing_brace_of(text: &str) -> Option<usize> {
    closing_brace(text)
}

/// The index of the `}` that closes an `@{` whose contents start at `text`.
///
/// Counts braces in Rust, so it skips what Rust skips: string literals, raw
/// strings (a template nested in `@{…}` is one: `html! {r#"<li>…</li>"#}`), and
/// char literals such as `'{'`.
fn closing_brace(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            b'r' if !(i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) => {
                let hashes = bytes[i + 1..].iter().take_while(|&&b| b == b'#').count();
                if bytes.get(i + 1 + hashes) == Some(&b'"') {
                    let close = format!("\"{}", "#".repeat(hashes));
                    let from = i + 2 + hashes;
                    i = text[from..]
                        .find(&close)
                        .map_or(bytes.len(), |at| from + at + close.len());
                    continue;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'\'' => {
                // A char literal, `'{'` or `'\n'`; a lifetime has no closing quote.
                let escaped = bytes.get(i + 1) == Some(&b'\\');
                let width = if escaped { 2 } else { 1 };
                if bytes.get(i + 1 + width) == Some(&b'\'') {
                    i += 2 + width;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Move accumulated literal text into one `Op::Static`. Adjacent `Op::Static`s
/// are folded together downstream, so this keeps a script's writes small
/// between lifts.
fn flush_text(run: &mut String, ops: &mut Vec<Op>) {
    if run.is_empty() {
        return;
    }
    ops.push(Op::Static(std::mem::take(run)));
}

/// [`unquote`], for the template lexer's client-binding marker.
pub fn unquote_literal(repr: &str) -> Option<String> {
    unquote(repr)
}

/// Recover the contents of a Rust string literal, plain or raw.
///
/// `proc_macro2` hands literals back as source text, so `"a\nb"` arrives as
/// six characters including the backslash.
fn unquote(repr: &str) -> Option<String> {
    let bytes = repr.as_bytes();

    if bytes.first() == Some(&b'r') {
        let hashes = repr[1..].chars().take_while(|c| *c == '#').count();
        let open = 1 + hashes;
        if bytes.get(open) != Some(&b'"') {
            return None;
        }
        return Some(repr[open + 1..repr.len() - 1 - hashes].to_string());
    }

    if bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') || repr.len() < 2 {
        return None;
    }

    let mut out = String::with_capacity(repr.len());
    let mut chars = repr[1..repr.len() - 1].chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'x' => {
                let hex: String = chars.by_ref().take(2).collect();
                out.push(u8::from_str_radix(&hex, 16).ok()? as char);
            }
            'u' => {
                // `\u{1F600}`
                let mut hex = String::new();
                for c in chars.by_ref() {
                    match c {
                        '{' => {}
                        '}' => break,
                        c => hex.push(c),
                    }
                }
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            // A backslash before a newline swallows the following indentation.
            '\n' => {
                while matches!(chars.clone().next(), Some(' ' | '\t' | '\n' | '\r')) {
                    chars.next();
                }
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// The nearest legal identifier to what the author wrote, for the error to
/// suggest: `rahti-main-layout` -> `rahti_main_layout`.
fn suggest_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 1);
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    if !out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$') {
        out.insert(0, '_');
    }
    // Sanitising leaves a reserved word intact, so suggesting it back would
    // be useless advice.
    if !crate::component::is_valid_js_identifier(&out) {
        out.push('_');
    }
    out
}

/// A client `{…}` binding, braces included, as the author wrote it.
///
/// The macro has no opinion on what is inside — that is the client runtime's
/// expression, in its language, and it gets copied through. The template
/// lexer carries it in a marked group (see `template::client_marker`).
fn binding_source(g: &proc_macro2::Group) -> String {
    crate::template::client_source(g).unwrap_or_else(|| format!("{{{}}}", g.stream()))
}

/// Escape text the author wrote in the markup.
///
/// An entity written out in full is left alone: `&amp;` and `&nbsp;` are how
/// you write those characters in HTML, and escaping them again would put
/// `&amp;amp;` on the page. This applies only to text fixed at compile time —
/// a `{expr}` goes through `Render`, which escapes everything, because that
/// is where values the author did not write come in.
///
/// A brace reference is the exception, and [`is_brace_entity`] says why.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(i) = rest.find(['&', '<', '>']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail.as_bytes()[0] {
            b'<' => out.push_str("&lt;"),
            b'>' => out.push_str("&gt;"),
            _ => match entity_len(tail) {
                Some(len) => {
                    if is_brace_entity(&tail[..len]) {
                        out.push_str("&amp;");
                        out.push_str(&tail[1..len]);
                    } else {
                        out.push_str(&tail[..len]);
                    }
                    rest = &tail[len..];
                    continue;
                }
                None => out.push_str("&amp;"),
            },
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Whether a character reference stands for `{` or `}`.
///
/// `{…}` belongs to the client runtime, and `&#123;` is how you ask for a
/// brace that is text rather than the start of one of its expressions. Sent as
/// written, the reference never reaches the runtime: the browser's parser
/// decodes it while building the document, so the runtime reads the page back
/// as `{user}` and evaluates it. Doubling the `&` is what puts the reference
/// itself in the document — the parser leaves `&#123;` standing as text, and
/// the runtime sees a reference where a brace would have been.
///
/// The set matches the one the runtime recognises; a form it does not know
/// would be escaped here into text it renders literally.
fn is_brace_entity(entity: &str) -> bool {
    const BRACES: [&str; 6] = [
        "&#123;", "&#125;", "&#x7b;", "&#x7d;", "&lbrace;", "&rbrace;",
    ];
    BRACES
        .iter()
        .any(|brace| brace.eq_ignore_ascii_case(entity))
}

/// The length of the character reference starting at `&`, if there is one:
/// `&amp;`, `&#160;` or `&#x2014;`.
fn entity_len(s: &str) -> Option<usize> {
    let body = s.strip_prefix('&')?;
    let (body, is_valid): (&str, fn(char) -> bool) = match body.strip_prefix('#') {
        Some(number) => match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, |c| c.is_ascii_hexdigit()),
            None => (number, |c| c.is_ascii_digit()),
        },
        // Named references are letters, and a few carry a trailing digit.
        None => (body, |c| c.is_ascii_alphanumeric()),
    };

    let digits = body.chars().take_while(|c| is_valid(*c)).count();
    if digits == 0 || !body[digits..].starts_with(';') {
        return None;
    }
    Some(s.len() - body.len() + digits + 1)
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

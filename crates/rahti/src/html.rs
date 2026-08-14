//! The HTML value type and the escaping rules around it.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

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
}

/// Numbers and `bool` need no escaping, so they go straight in.
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
    bool, char, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
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
///         <script>
///             const [count, setCount] = pp.state(@{42});
///             const [name, setName] = pp.state(@{"Ada"});
///             const user = @{Json(&user)};
///         </script>
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

/// Numbers and flags need no quoting, so they go straight in — same as
/// `Render`, since JS and HTML spell a number the same way. `NaN` and
/// `Infinity` are valid JS literals here; they are not JSON, so a `Json<f64>`
/// holding one would fail — that path goes through the field's own
/// `RenderJs` impl instead.
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
    bool, char, u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

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
///     <script>
///         const user = @{Json(&user)};
///         const rows = @{Json(&rows)};
///     </script>
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

fn escape_text_into(text: &str, out: &mut String) {
    out.reserve(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

fn escape_attr_into(text: &str, out: &mut String) {
    out.reserve(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

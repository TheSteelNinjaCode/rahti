//! The grammar `html!` accepts, and the markup it produces.
//!
//! Every test here writes a template, as an author does, and goes through
//! [`parse`] — everything the macro does before it emits Rust: what comes back
//! is the list of writes, and joining it back together with [`show`] is the
//! markup a page would send.

use super::*;
use proc_macro2::TokenStream;
use std::str::FromStr;

/// The tokens `html! { {src} }` hands `parse`, or the lexer's refusal.
fn template(src: &str) -> Result<TokenStream, String> {
    // A leading `@component <id>` stands for the directive `#[component]`
    // writes in front of the template.
    let (directive, src) = match src.strip_prefix("@component ") {
        Some(rest) => {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            (format!("@component \"{}\" ", &rest[..end]), &rest[end..])
        }
        None => (String::new(), src),
    };
    TokenStream::from_str(&format!("{directive}{src}")).map_err(|e| e.to_string())
}

/// Parse a template, or fail the test with the message the author would see.
fn parse_ok(src: &str) -> Parsed {
    let tokens =
        template(src).unwrap_or_else(|msg| panic!("expected `{src}` to parse, got:\n{msg}"));
    match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    }
}

/// The ops of a block, joined the way `emit` would write them.
///
/// Interpolation has no value until runtime, so it is shown as it was
/// written: `@{expr}` for Rust and `<slot/>` for the children binding.
fn show(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        match op {
            Op::Static(s) => out.push_str(s),
            Op::Text(expr) => out.push_str(&format!("@{{{expr}}}")),
            // The name travels with the value now, so the op writes the whole
            // attribute — quotes included.
            Op::Attr { name, expr, .. } => out.push_str(&format!("{name}=\"@{{{expr}}}\"")),
            Op::OpenTag(attrs) => {
                for attr in attrs {
                    match attr {
                        TagAttr::Literal { name, value } => match value {
                            Some(value) => out.push_str(&format!(" {name}=\"{value}\"")),
                            None => out.push_str(&format!(" {name}")),
                        },
                        TagAttr::Value { name, expr, .. } => {
                            out.push_str(&format!(" {name}=\"@{{{expr}}}\""));
                        }
                        TagAttr::Spread(expr) => out.push_str(&format!(" @{{..{expr}}}")),
                        TagAttr::OwnsEvents => out.push_str(" @owns-events"),
                    }
                }
            }
            Op::ScriptText(expr) => out.push_str(&format!("@{{{expr}}}")),
            Op::ScriptData(values) => {
                let values: Vec<String> = values.iter().map(|v| format!("@{{{v}}}")).collect();
                out.push_str(&format!(" pp-data=\"[{}]\"", values.join(",")));
            }
            Op::Slot | Op::DeferredSlot => out.push_str("<slot/>"),
            Op::Component(call) => {
                out.push('<');
                out.push_str(&call.name.to_string());
                for (prop, value) in &call.props {
                    out.push_str(&format!(" {prop}={value}"));
                }
                for (name, expr) in &call.bindings {
                    out.push_str(&format!(" {name}={{{expr}}}"));
                }
                match &call.children {
                    None => out.push_str("/>"),
                    Some(child) => {
                        out.push('>');
                        out.push_str(&show(&child.ops));
                        out.push_str(&format!("</{}>", call.name));
                    }
                }
            }
        }
    }
    out
}

/// The markup a block renders.
fn render(src: &str) -> String {
    show(&parse_ok(src).ops)
}

/// The code a block's component script registers.
fn script_source(parsed: &Parsed) -> String {
    parsed
        .client_code
        .iter()
        .find(|code| code.kind == ClientKind::Script)
        .map(|code| code.source.clone())
        .expect("a component script")
}

/// The message a template fails with, or a failure if it parsed.
fn error(src: &str) -> String {
    let stream = match template(src) {
        Ok(tokens) => tokens,
        Err(msg) => return msg,
    };
    match parse(stream) {
        Ok(parsed) => panic!(
            "expected `{src}` to fail, it rendered:\n{}",
            show(&parsed.ops)
        ),
        Err(err) => err.msg,
    }
}

// ---------------------------------------------------------------- elements

#[test]
fn element_with_text() {
    assert_eq!(render(r#"<h1>"Rahti"</h1>"#), "<h1>Rahti</h1>");
}

#[test]
fn elements_nest() {
    assert_eq!(
        render(r#"<div><section><p>"hi"</p></section></div>"#),
        "<div><section><p>hi</p></section></div>"
    );
}

#[test]
fn empty_element() {
    assert_eq!(render(r#"<div></div>"#), "<div></div>");
}

#[test]
fn hyphenated_tag_name() {
    assert_eq!(
        render(r#"<my-element>"x"</my-element>"#),
        "<my-element>x</my-element>"
    );
}

#[test]
fn void_element_gets_no_closing_tag() {
    assert_eq!(render(r#"<div><br></div>"#), "<div><br></div>");
    assert_eq!(
        render(r#"<div><img src="a.png" /></div>"#),
        "<div><img src=\"a.png\"></div>"
    );
}

/// HTML5 has no self-closing syntax for a normal tag: a browser reading
/// `<div />` would treat everything after it as a child.
#[test]
fn self_closing_non_void_expands_to_a_pair() {
    assert_eq!(render(r#"<div><span /></div>"#), "<div><span></span></div>");
}

#[test]
fn doctype_is_emitted_verbatim() {
    assert_eq!(
        render(r#"<!DOCTYPE html><html><body>"x"</body></html>"#),
        "<!DOCTYPE html><html><body>x</body></html>"
    );
}

// -------------------------------------------------------------- attributes

#[test]
fn string_attribute() {
    assert_eq!(
        render(r#"<a href="/users">"u"</a>"#),
        "<a href=\"/users\">u</a>"
    );
}

#[test]
fn several_attributes_keep_their_order() {
    assert_eq!(
        render(r#"<input type="text" name="q" />"#),
        "<input type=\"text\" name=\"q\">"
    );
}

/// `<input required>` — no value at all.
#[test]
fn boolean_attribute() {
    assert_eq!(render(r#"<input required />"#), "<input required>");
}

#[test]
fn non_string_literal_attribute_is_written_as_it_stands() {
    assert_eq!(
        render(r#"<div tabindex="0">"x"</div>"#),
        "<div tabindex=\"0\">x</div>"
    );
}

#[test]
fn hyphenated_and_numbered_attribute_names() {
    assert_eq!(
        render(r#"<div data-user-id="7" aria-level-2="x">"y"</div>"#),
        "<div data-user-id=\"7\" aria-level-2=\"x\">y</div>"
    );
}

#[test]
fn attribute_values_are_escaped() {
    assert_eq!(
        render(r#"<div title="a \" b & c < d">"x"</div>"#),
        "<div title=\"a &quot; b &amp; c &lt; d\">x</div>"
    );
}

/// A `:` separates a namespace prefix from the attribute under it, which is
/// how SVG carries `xlink:href` and how an icon exported from a drawing
/// program carries `xml:space`.
#[test]
fn namespaced_attribute_names() {
    assert_eq!(
        render(
            r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="#a" /></svg>"##
        ),
        "<svg xmlns:xlink=\"http://www.w3.org/1999/xlink\"><use xlink:href=\"#a\"></use></svg>"
    );
    assert_eq!(
        render(r#"<svg xml:space="preserve">"x"</svg>"#),
        "<svg xml:space=\"preserve\">x</svg>"
    );
}

#[test]
fn a_namespace_prefix_with_nothing_after_it_is_refused() {
    assert!(error(r#"<svg xmlns:>"x"</svg>"#).contains("expected a name after `:`"));
}

// ------------------------------------------------------------- boolean names

/// The table is consulted by name, and the flag it produces is what tells
/// `rahti::attribute` that `false` means "no attribute" rather than the word.
#[test]
fn a_boolean_attribute_name_is_marked_as_one() {
    let parsed = parse_ok(r#"<input disabled=@{off} />"#);
    let marked = parsed
        .ops
        .iter()
        .any(|op| matches!(op, Op::Attr { name, boolean, .. } if name == " disabled" && *boolean));
    assert!(marked, "{}", show(&parsed.ops));
}

/// The names that are *not* on it matter as much as the ones that are.
/// `aria-hidden="false"` says something no absent attribute says.
#[test]
fn an_aria_attribute_is_not_boolean() {
    for src in [
        "<span aria-hidden=@{flag}>x</span>",
        "<span aria-expanded=@{flag}>x</span>",
        "<div contenteditable=@{flag}>x</div>",
        "<div draggable=@{flag}>x</div>",
        "<div spellcheck=@{flag}>x</div>",
        "<input autocomplete=@{flag} />",
    ] {
        let parsed = parse_ok(src);
        let marked = parsed
            .ops
            .iter()
            .any(|op| matches!(op, Op::Attr { boolean, .. } if *boolean));
        assert!(!marked, "{src} should not be a boolean attribute");
    }
}

#[test]
fn the_boolean_table_covers_the_html_ones() {
    for name in [
        "disabled",
        "checked",
        "readonly",
        "required",
        "hidden",
        "open",
        "multiple",
        "selected",
        "autofocus",
        "async",
        "defer",
        "inert",
        "loop",
        "muted",
        "novalidate",
        "reversed",
    ] {
        assert!(is_boolean_attribute(name), "{name} is a boolean attribute");
    }
}

#[test]
fn the_boolean_table_leaves_the_enumerated_ones_out() {
    for name in [
        "aria-hidden",
        "aria-expanded",
        "aria-pressed",
        "contenteditable",
        "draggable",
        "spellcheck",
        "translate",
        "autocomplete",
        "class",
        "value",
    ] {
        assert!(
            !is_boolean_attribute(name),
            "{name} is read by value, not by presence"
        );
    }
}

// -------------------------------------------------------------------- spread

#[test]
fn a_spread_is_an_attribute_of_its_own() {
    assert_eq!(
        render(r#"<div class="a" @{..extra}>"x"</div>"#),
        "<div class=\"a\" @{..extra}>x</div>"
    );
}

/// A tag with a spread builds every one of its attributes, because they all
/// have to go through the same list for a later one to replace an earlier one.
#[test]
fn a_spread_takes_the_whole_tag_with_it() {
    let parsed = parse_ok(r#"<div class="a" id=@{n} @{..extra}>"x"</div>"#);
    let attrs = parsed
        .ops
        .iter()
        .find_map(|op| match op {
            Op::OpenTag(attrs) => Some(attrs),
            _ => None,
        })
        .expect("an open tag built at runtime");

    assert_eq!(attrs.len(), 3);
    assert!(matches!(&attrs[0], TagAttr::Literal { name, .. } if name == "class"));
    assert!(matches!(&attrs[1], TagAttr::Value { name, .. } if name == "id"));
    assert!(matches!(attrs[2], TagAttr::Spread(_)));
}

/// And a tag without one is untouched: its attributes still fold into the
/// static markup, and only an `@{…}` value survives to runtime.
#[test]
fn a_tag_without_a_spread_builds_nothing() {
    let parsed = parse_ok(r#"<div class="a" id=@{n}>"x"</div>"#);

    assert!(!parsed.ops.iter().any(|op| matches!(op, Op::OpenTag(_))));
}

#[test]
fn a_spread_may_stand_alone_or_beside_a_binding() {
    assert_eq!(
        render(r#"<div @{..extra}>"x"</div>"#),
        "<div @{..extra}>x</div>"
    );
    assert_eq!(
        render(r#"<div><button onclick={save()} @{..extra}>"x"</button></div>"#),
        "<div><button onclick=\"{save()}\" @{..extra}>x</button></div>"
    );
    // As the block's root, the button owns its `onclick`, so the tag built at
    // runtime is marked to refuse a handler the spread would bring.
    assert_eq!(
        render(r#"<button onclick={save()} @{..extra}>"x"</button>"#),
        "<button @owns-events onclick=\"{save()}\" @{..extra}>x</button>"
    );
}

#[test]
fn several_spreads_are_folded_in_written_order() {
    let parsed = parse_ok(r#"<div @{..a} @{..b}>"x"</div>"#);
    let attrs = parsed
        .ops
        .iter()
        .find_map(|op| match op {
            Op::OpenTag(attrs) => Some(attrs),
            _ => None,
        })
        .expect("an open tag built at runtime");

    assert_eq!(attrs.len(), 2);
    assert!(attrs.iter().all(|a| matches!(a, TagAttr::Spread(_))));
}

/// JSX spells a spread `{...props}`, and `{…}` in a tag is the client
/// runtime's. The three-dot form is recognised only to say which dialect it
/// landed in.
#[test]
fn the_jsx_spelling_is_refused_with_the_rahti_one() {
    let message = error(r#"<div @{...extra}>"x"</div>"#);

    assert!(message.contains("@{..attrs}"), "{message}");
    assert!(message.contains("client runtime"), "{message}");
}

#[test]
fn a_spread_of_nothing_is_refused() {
    assert!(error(r#"<div @{..}>"x"</div>"#).contains("spreads nothing"));
}

/// A Rust value in the name position is the other way of writing this wrong,
/// and the message names both shapes.
#[test]
fn a_rust_value_where_a_name_belongs_is_refused() {
    let message = error(r#"<div @{extra}>"x"</div>"#);

    assert!(message.contains("class=@{name}"), "{message}");
    assert!(message.contains("@{..attrs}"), "{message}");
}

/// A component's props are its parameters, and an attribute set has no way to
/// know which of them it is filling. The message says what to do instead.
#[test]
fn a_spread_on_a_component_tag_is_refused() {
    let message = error(r#"<Card @{..extra} />"#);

    assert!(message.contains("is a component"), "{message}");
    assert!(message.contains("attrs: ::rahti::Attrs"), "{message}");
}

// ------------------------------------------------------------ interpolation

#[test]
fn rust_interpolation_in_text_becomes_a_render_call() {
    let parsed = parse_ok(r#"<h1>@{title}</h1>"#);
    assert!(matches!(parsed.ops[1], Op::Text(_)), "expected a text op");
    assert_eq!(show(&parsed.ops), "<h1>@{title}</h1>");
}

#[test]
fn rust_interpolation_in_an_attribute_becomes_an_attr_call() {
    let parsed = parse_ok(r#"<a href=@{url}>"x"</a>"#);
    assert!(
        parsed.ops.iter().any(|op| matches!(op, Op::Attr { .. })),
        "expected an attribute op"
    );
    assert_eq!(show(&parsed.ops), "<a href=\"@{url}\">x</a>");
}

/// The two sides are told apart by the `@`, and only by it.
#[test]
fn client_binding_is_copied_through_as_text() {
    let parsed = parse_ok(r#"<p>"Count: "{count}</p>"#);
    assert!(
        parsed.ops.iter().all(|op| matches!(op, Op::Static(_))),
        "a client binding is static as far as the server is concerned"
    );
    assert_eq!(show(&parsed.ops), "<p>Count: {count}</p>");
}

#[test]
fn client_binding_in_an_attribute_is_copied_through() {
    assert_eq!(
        render(r#"<button onclick={setCount(count + 1)}>"+"</button>"#),
        "<button pp-onclick=\"{setCount(count + 1)}\">+</button>"
    );
}

/// A binding is the client runtime's expression, in its language: nothing in
/// it is read, evaluated or rewritten.
#[test]
fn client_binding_contents_are_not_interpreted() {
    assert_eq!(
        render(r#"<p>{user.name ?? "anon"}</p>"#),
        "<p>{user.name ?? &quot;anon&quot;}</p>".replace("&quot;", "\"")
    );
}

#[test]
fn expressions_may_be_arbitrary_rust() {
    assert_eq!(
        render(r#"<p>@{format!("{}-{}", a, b)}</p>"#),
        "<p>@{format ! (\"{}-{}\" , a , b)}</p>"
    );
}

/// `@` opens interpolation only when a brace follows it. Everywhere else it
/// is a character in the text.
#[test]
fn bare_at_sign_is_text() {
    assert_eq!(
        render(r#"<p>"ada@example.com"</p>"#),
        "<p>ada@example.com</p>"
    );
}

// -------------------------------------------------------------------- text

/// Indentation is the shape of the source, not of the page.
#[test]
fn a_newline_between_words_collapses_to_one_space() {
    assert_eq!(render(r#"<p>"one two"</p>"#), "<p>one two</p>");
}

/// A gap the author typed on one line survives, so `Hello <b>world</b>`
/// does not run together.
#[test]
fn a_space_beside_a_tag_is_kept() {
    assert_eq!(
        render(r#"<p>"Hello "<b>"world"</b></p>"#),
        "<p>Hello <b>world</b></p>"
    );
}

#[test]
fn text_on_its_own_line_picks_up_no_space() {
    assert_eq!(
        render(
            r#"<p>
    "Hello"
</p>"#
        ),
        "<p>Hello</p>"
    );
}

#[test]
fn adjacent_characters_stay_together() {
    assert_eq!(render(r#"<p>"100%"</p>"#), "<p>100%</p>");
}

/// Spaces typed on one line are the author's, and stay as written.
#[test]
fn spaces_on_one_line_are_kept() {
    assert_eq!(
        render(r#"<p>"  it's  spaced  "</p>"#),
        "<p>  it's  spaced  </p>"
    );
}

/// A quote or an apostrophe in text is a character, not the start of a
/// string.
#[test]
fn quotes_in_text_are_text() {
    assert_eq!(
        render(r#"<p>"a \"quote\" here"</p>"#),
        "<p>a \"quote\" here</p>"
    );
}

#[test]
fn text_is_escaped() {
    assert_eq!(
        render(r#"<p>"1 &lt; 2 &gt; 0"</p>"#),
        "<p>1 &lt; 2 &gt; 0</p>"
    );
}

/// `&amp;` is how you write an ampersand in HTML; escaping it again would put
/// `&amp;amp;` on the page.
#[test]
fn character_references_are_left_alone() {
    assert_eq!(
        render(r#"<p>"a &amp; b &nbsp; &#160; &#x2014;"</p>"#),
        "<p>a &amp; b &nbsp; &#160; &#x2014;</p>"
    );
}

/// A brace reference is the one that cannot be sent as written: the parser
/// would decode it while building the document, and the client runtime would
/// read `{user}` back off the page and evaluate it. Doubled, the reference
/// itself is what stands in the document.
#[test]
fn brace_references_are_sent_doubled() {
    assert_eq!(
        render(r#"<p>"&#123;user&#125;"</p>"#),
        "<p>&amp;#123;user&amp;#125;</p>"
    );
    assert_eq!(
        render(r#"<p>"&#x7B;a&#x7d; &lbrace;b&rbrace;"</p>"#),
        "<p>&amp;#x7B;a&amp;#x7d; &amp;lbrace;b&amp;rbrace;</p>"
    );
}

/// The doubling is for text. An attribute escapes every `&` already, and what
/// the runtime reads there is the decoded value, so it sees the reference.
#[test]
fn a_brace_reference_in_an_attribute_is_escaped_once() {
    assert_eq!(
        render(r#"<p title="&#123;user&#125;">"x"</p>"#),
        "<p title=\"&amp;#123;user&amp;#125;\">x</p>"
    );
}

#[test]
fn a_bare_ampersand_is_escaped() {
    assert_eq!(render(r#"<p>"Tom & Jerry"</p>"#), "<p>Tom &amp; Jerry</p>");
    assert_eq!(
        render(r#"<p>"a &notanentity b"</p>"#),
        "<p>a &amp;notanentity b</p>"
    );
    assert_eq!(render(r#"<p>"a &amp b"</p>"#), "<p>a &amp;amp b</p>");
}

// ---------------------------------------------------------------- raw text

/// A bare `<script>` is a component script: compiled ahead of time and
/// registered, so the page carries only an empty element naming it. A brace
/// there is a block, and `<` a comparison — the body is never markup.
#[test]
fn a_component_script_is_registered_not_written() {
    let parsed = parse_ok(r###"<script>r##"const [a, set] = pp.state(0);"##</script>"###);
    let out = show(&parsed.ops);
    assert!(out.starts_with("<script pp-script=\"s"), "{out}");
    assert!(out.ends_with("\"></script>"), "{out}");
    assert!(!out.contains("pp.state"), "{out}");
    assert_eq!(script_source(&parsed), "const [a, set] = pp.state(0);");
}

#[test]
fn script_body_keeps_braces_and_comparisons() {
    let parsed = parse_ok(r###"<script>r##"if (a < b) { go(); }"##</script>"###);
    assert_eq!(script_source(&parsed), "if (a < b) { go(); }");
}

/// The key names the code, so the same script always lands on the same entry.
#[test]
fn a_script_key_is_the_hash_of_its_code() {
    let one = render(r###"<script>r##"const a = 1;"##</script>"###);
    let same = render(r###"<script>r##"const a = 1;"##</script>"###);
    let other = render(r###"<script>r##"const a = 2;"##</script>"###);
    assert_eq!(one, same);
    assert_ne!(one, other);
}

#[test]
fn style_body_is_not_parsed() {
    let out = render(r###"<style>r##".a { color: red; }"##</style>"###);
    assert!(out.contains(".a { color: red; }"), "{out}");
}

/// `@{expr}` in a component script is a server value: the compiled script
/// reads it as `__pp_server[i]`, and the page carries the values as the
/// script's `pp-data` JSON array, in order.
#[test]
fn script_server_values_become_pp_data() {
    let parsed = parse_ok(
        r###"<script>r##"const [n, setN] = pp.state(@{initial}); const s = @{label};"##</script>"###,
    );
    assert_eq!(
        script_source(&parsed),
        "const [n, setN] = pp.state(__pp_server[0]); const s = __pp_server[1];"
    );
    let out = show(&parsed.ops);
    assert!(
        out.ends_with(" pp-data=\"[@{initial},@{label}]\"></script>"),
        "{out}"
    );
}

/// The end of an `@{…}` is found by counting braces, past Rust strings.
#[test]
fn a_script_finds_the_end_of_a_nested_expression() {
    let parsed = parse_ok(
        r###"<script>r##"const a = @{format!("{}}", Json(&x))}; const b = 1;"##</script>"###,
    );
    assert_eq!(
        script_source(&parsed),
        "const a = __pp_server[0]; const b = 1;"
    );
}

/// A bare `@` stays JavaScript; only `@` followed by a `{…}` group is lifted.
#[test]
fn a_bare_at_in_a_script_stays_raw() {
    let parsed = parse_ok(r###"<script>r##"const p = a @ b;"##</script>"###);
    assert_eq!(script_source(&parsed), "const p = a @ b;");
}

/// `@@{` is a literal `@{`, for the rare script that needs one.
#[test]
fn a_script_escapes_a_literal_at_brace() {
    let parsed = parse_ok(r###"<script>r##"const css = '@@{ not rust }';"##</script>"###);
    assert_eq!(script_source(&parsed), "const css = '@{ not rust }';");
}

/// A script with attributes is not a component script: a `type`d data block
/// is written as it stands, its `@{…}` rendered in place through `RenderJs`.
#[test]
fn a_typed_script_is_written_with_its_values_in_place() {
    let parsed =
        parse_ok(r###"<script type="application/json">r##"{"count": @{count}}"##</script>"###);
    assert!(
        parsed.ops.iter().any(|op| matches!(op, Op::ScriptText(_))),
        "{:?}",
        parsed.ops
    );
    assert_eq!(
        show(&parsed.ops),
        "<script type=\"application/json\">{\"count\": @{count}}</script>"
    );
    assert!(parsed.client_code.is_empty());
}

/// `<style>` keeps the raw-text rule even for `@media`/`@keyframes`: the `@`
/// is part of a CSS at-rule, and the `{…}` is a rule body.
#[test]
fn style_keeps_at_braces_raw() {
    let parsed =
        parse_ok(r###"<style>r##"@media (min-width: 1px) { .a { color: red; } }"##</style>"###);
    assert!(
        parsed.ops.iter().all(|op| matches!(op, Op::Static(_))),
        "expected nothing lifted in a style body, got {:?}",
        parsed.ops
    );
}

// ------------------------------------------------------------------ scripts

/// A script body is JavaScript verbatim: single quotes, template literals and
/// comments included.
#[test]
fn a_script_is_javascript_verbatim() {
    let parsed = parse_ok(
        r###"<script>r##"
        // a comment that stays
        const greeting = 'hi';
        const line = `${greeting}, it's ${new Date().getFullYear()}`;
    "##</script>"###,
    );
    let source = script_source(&parsed);
    assert!(source.contains("// a comment that stays"), "{source}");
    assert!(source.contains("const greeting = 'hi';"), "{source}");
    assert!(
        source.contains("`${greeting}, it's ${new Date().getFullYear()}`"),
        "{source}"
    );
}

#[test]
fn an_unclosed_at_brace_in_a_script_is_an_error() {
    let msg = error(r###"<script>r##"const a = @{x;"##</script>"###);
    assert!(msg.contains("never closed"), "{msg}");
}

/// A body that is one JavaScript string is still just JavaScript.
#[test]
fn a_script_that_is_one_string_is_javascript() {
    let parsed = parse_ok(r###"<script>r##""use strict";"##</script>"###);
    assert_eq!(script_source(&parsed), r#""use strict";"#);
}

/// Every binding, handler and loop collection a block writes is recorded for
/// registration, under the source the runtime reads back off the page.
#[test]
fn client_code_is_recorded_under_its_page_source() {
    let parsed = parse_ok(
        r#"<ul class="list" data-x={x + 1}>
            <template pp-for="(item, i) in items.slice(1)">
                <li key={item.id} onclick={pick(item)}>{item.name}</li>
            </template>
        </ul>"#,
    );
    let recorded: Vec<(ClientKind, &str)> = parsed
        .client_code
        .iter()
        .map(|code| (code.kind, code.key.as_str()))
        .collect();
    assert_eq!(
        recorded,
        [
            (ClientKind::Expression, "x + 1"),
            (ClientKind::Expression, "items.slice(1)"),
            (ClientKind::Expression, "item.id"),
            (ClientKind::Handler, "pick(item)"),
            (ClientKind::Expression, "item.name"),
        ]
    );
}

/// A `{…}` inside quoted text that holds an expression would be hidden client
/// code; braces around anything else are text.
#[test]
fn an_expression_in_quoted_text_is_refused() {
    for src in [
        r#"<p>"Hi {name}"</p>"#,
        r#"<p class="row {active}"></p>"#,
        r#"<p hidden="{count < max}"></p>"#,
    ] {
        let msg = error(src);
        assert!(msg.contains("inside quoted text"), "{src}: {msg}");
    }
    render(r#"<div data-config="{&quot;a&quot;: 1}"></div>"#);
    render(r#"<p>"a {b: 1} c"</p>"#);
}

#[test]
fn empty_script_body() {
    assert_eq!(
        render(r#"<script src="/js/main.js"></script>"#),
        "<script src=\"/js/main.js\"></script>"
    );
}

// -------------------------------------------------------------------- slot

#[test]
fn slot_becomes_the_children_binding() {
    let parsed = parse_ok(r#"<main><slot /></main>"#);
    assert!(parsed.ops.iter().any(|op| matches!(op, Op::Slot)));
    assert_eq!(show(&parsed.ops), "<main><slot/></main>");
}

#[test]
fn paired_slot_is_accepted() {
    assert_eq!(
        render(r#"<main><slot></slot></main>"#),
        "<main><slot/></main>"
    );
}

#[test]
fn slot_takes_no_children() {
    assert!(error(r#"<main><slot>"x"</slot></main>"#).contains("takes no children"));
}

#[test]
fn slot_takes_no_attributes() {
    assert!(error(r#"<main><slot name="a" /></main>"#).contains("takes no attributes"));
}

// -------------------------------------------------------------- components

/// A capital letter opens a component call, not markup.
#[test]
fn an_uppercase_tag_is_a_component() {
    let parsed = parse_ok(r#"<div><Card /></div>"#);
    assert!(
        parsed.ops.iter().any(|op| matches!(op, Op::Component(_))),
        "expected an Op::Component in {:?}",
        parsed.ops
    );
    assert_eq!(show(&parsed.ops), "<div><Card/></div>");
}

#[test]
fn component_props_keep_their_order_and_values() {
    let parsed = parse_ok(r#"<div><Card title="Profile" count=3 wide flag=@{is_on} /></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let props: Vec<(String, String)> = call
        .props
        .iter()
        .map(|(p, v)| (p.to_string(), v.to_string()))
        .collect();
    assert_eq!(props[0], ("title".into(), "\"Profile\"".into()));
    assert_eq!(props[1], ("count".into(), "3".into()));
    // A bare prop is JSX's boolean shorthand.
    assert_eq!(props[2], ("wide".into(), "true".into()));
    // `@{expr}` is parenthesised so the expression stays one value.
    assert_eq!(props[3], ("flag".into(), "(is_on)".into()));
    assert!(call.children.is_none(), "self-closing passes no children");
}

/// A prop is written as HTML writes it: `for="…"` fills the parameter Rust
/// has to spell `r#for`, and a `{…}` keyword prop keeps its HTML name.
#[test]
fn a_keyword_prop_fills_its_raw_parameter() {
    let parsed =
        parse_ok(r#"<div><Label for="email" type="button" title="x" hidden={!open} /></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let names: Vec<String> = call.props.iter().map(|(p, _)| p.to_string()).collect();
    assert_eq!(names, ["r#for", "r#type", "title"]);
    assert_eq!(call.bindings, [("hidden".to_string(), "!open".to_string())]);
}

#[test]
fn component_children_are_a_block_of_their_own() {
    let parsed = parse_ok(r#"<div><Card title="x"><p>"hi"</p></Card></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let child = call.children.as_ref().expect("a tag pair passes children");
    assert_eq!(show(&child.ops), "<p>hi</p>");
    assert!(
        child.component_slot.is_some(),
        "a single-rooted fragment offers its root for a name"
    );
}

#[test]
fn empty_children_are_still_children() {
    let parsed = parse_ok(r#"<div><Card></Card></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let child = call.children.as_ref().expect("a tag pair passes children");
    assert!(child.ops.is_empty(), "…an empty fragment");
}

/// Several roots match the `Html::concat` shape, which never carried a name.
#[test]
fn multi_rooted_children_offer_no_name_slot() {
    let parsed = parse_ok(r#"<div><Card><b>"a"</b><i>"b"</i></Card></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let child = call.children.as_ref().expect("children");
    assert!(child.component_slot.is_none());
    assert_eq!(show(&child.ops), "<b>a</b><i>b</i>");
}

#[test]
fn components_nest() {
    assert_eq!(
        render(r#"<div><Panel title="a"><Badge label="b" /></Panel></div>"#),
        "<div><Panel title=\"a\"><Badge label=\"b\"/></Panel></div>"
    );
}

/// The child fragment is a block of its own, so the outer single-root rule
/// does not reach inside it — and the outer bookkeeping survives it.
#[test]
fn a_component_does_not_disturb_the_outer_root() {
    let parsed = parse_ok(r#"<main><Card><p>"x"</p></Card><footer>"y"</footer></main>"#);
    let (index, offset, _) = parsed
        .component_slot
        .expect("the root element takes a name");
    let Op::Static(markup) = &parsed.ops[index] else {
        panic!("the slot points at static markup");
    };
    assert_eq!(&markup[..offset], "<main");
}

/// A `{…}` prop is the client runtime's, so it is kept as source instead of
/// evaluated. The component receives the expression and decides which
/// element it lands on — which is how `<Button onclick={save()}>` puts a
/// handler on the button.
#[test]
fn a_client_binding_prop_is_carried_as_source() {
    let parsed = parse_ok(r#"<div><Button onclick={setCount(count + 1)}>"x"</Button></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component call in {:?}", parsed.ops);
    };

    assert!(call.props.is_empty(), "{:?}", call.props);
    assert_eq!(
        call.bindings,
        vec![("onclick".to_string(), "setCount(count + 1)".to_string())]
    );
}

/// Rust props and client bindings on the same tag stay in their own lanes.
#[test]
fn a_tag_may_carry_both_kinds_of_prop() {
    let parsed = parse_ok(r#"<Button variant="outline" onclick={save()} />"#);
    let [Op::Component(call)] = parsed.ops.as_slice() else {
        panic!("expected one component call in {:?}", parsed.ops);
    };

    assert_eq!(call.props.len(), 1);
    assert_eq!(call.props[0].0.to_string(), "variant");
    assert_eq!(
        call.bindings,
        vec![("onclick".to_string(), "save()".to_string())]
    );
}

/// Composition: a block may render another component as its whole output,
/// the way a React component returns another component.
#[test]
fn a_component_tag_may_be_the_root() {
    let parsed = parse_ok(r#"<Card title="x" />"#);
    assert!(matches!(parsed.ops.as_slice(), [Op::Component(_)]));
    assert!(
        parsed.component_slot.is_none(),
        "a composition root offers no element to name"
    );
    assert!(!parsed.renders_document);
}

#[test]
fn a_composition_root_takes_children() {
    let parsed = parse_ok(r#"<Panel title="x"><p>"hi"</p></Panel>"#);
    let [Op::Component(call)] = parsed.ops.as_slice() else {
        panic!("expected one component op, got {:?}", parsed.ops);
    };
    let child = call.children.as_ref().expect("children");
    assert!(
        child.component_slot.is_some(),
        "the children fragment still offers its root for a name"
    );
}

#[test]
fn a_component_root_beside_an_element_is_rejected() {
    let msg = error(r#"<Card title="x" /><div>"y"</div>"#);
    assert!(msg.contains("single parent element"), "{msg}");
    assert!(msg.contains("<Card>, <div>"), "{msg}");
}

#[test]
fn two_component_roots_are_rejected() {
    let msg = error(r#"<Card /><Badge />"#);
    assert!(msg.contains("single parent element"), "{msg}");
}

/// A document's parent element is `<html>`, written literally — the doctype
/// prologue and a dissolving component root cannot share a block.
#[test]
fn a_doctype_does_not_precede_a_component_root() {
    let msg = error(r#"<!DOCTYPE html><Card />"#);
    assert!(msg.contains("writes its root element literally"), "{msg}");
}

/// A component named `Html` is a component, not the document element.
#[test]
fn a_component_named_html_is_not_a_document() {
    let parsed = parse_ok(r#"<Html lang="en" />"#);
    assert!(!parsed.renders_document);
}

#[test]
fn an_unclosed_component_is_reported() {
    assert!(error(r#"<div><Card>"x"</div>"#).contains("`<Card>` is closed by `</div>`"));
}

#[test]
fn a_mismatched_component_closing_tag_is_reported() {
    assert!(error(r#"<div><Card>"x"</Cards></div>"#).contains("`<Card>` is closed by `</Cards>`"));
}

#[test]
fn an_uppercase_tag_with_a_hyphen_is_refused() {
    let msg = error(r#"<div><My-thing /></div>"#);
    assert!(msg.contains("PascalCase"), "{msg}");
}

#[test]
fn a_prop_needs_a_usable_value() {
    assert!(error(r#"<div><Card title= /></div>"#).contains("`title` needs a value"));
}

// --------------------------------------------------------------- fragments

/// `<>…</>` — several roots for one block, shipped as a layout-invisible
/// wrapper element that takes the block's name.
#[test]
fn a_fragment_wraps_several_roots() {
    assert_eq!(
        render(r#"<><h1>"a"</h1><p>"b"</p></>"#),
        "<!--pp:--><h1>a</h1><p>b</p><!--/pp-->"
    );
}

#[test]
fn a_fragment_offers_its_wrapper_for_a_name() {
    let parsed = parse_ok(r#"<><p>"x"</p></>"#);
    let (index, offset, kind) = parsed.component_slot.expect("a fragment takes a name");
    let Op::Static(markup) = &parsed.ops[index] else {
        panic!("the slot points at static markup");
    };
    // Just past `<!--pp:`, so the name becomes the marker's text.
    assert_eq!(&markup[..offset], "<!--pp:");
    assert_eq!(kind, SlotKind::Fragment);
    assert!(!parsed.renders_document);
}

#[test]
fn an_empty_fragment_is_allowed() {
    assert_eq!(render(r#"<></>"#), "<!--pp:--><!--/pp-->");
}

/// Interpolation, bindings and scripts are ordinary fragment children.
#[test]
fn a_fragment_holds_any_node() {
    let parsed = parse_ok(r###"<><p>@{title}</p><script>r##"const a = 1;"##</script></>"###);
    assert!(parsed.ops.iter().any(|op| matches!(op, Op::Text(_))));
    let out = show(&parsed.ops);
    assert!(
        out.starts_with("<!--pp:--><p>@{title}</p><script pp-script=\"s"),
        "{out}"
    );
    assert!(out.ends_with("\"></script><!--/pp-->"), "{out}");
    assert_eq!(script_source(&parsed), "const a = 1;");
}

#[test]
fn a_fragment_may_root_component_children() {
    let parsed = parse_ok(r#"<div><Panel><><p>"a"</p><p>"b"</p></></Panel></div>"#);
    let Some(Op::Component(call)) = parsed.ops.iter().find(|op| matches!(op, Op::Component(_)))
    else {
        panic!("expected a component op");
    };
    let child = call.children.as_ref().expect("children");
    assert!(
        child.component_slot.is_some(),
        "a fragment-rooted child block offers its wrapper for a name"
    );
}

#[test]
fn a_fragment_beside_another_root_is_rejected() {
    let msg = error(r#"<><p>"a"</p></><div>"b"</div>"#);
    assert!(msg.contains("single parent element"), "{msg}");
    assert!(msg.contains("<>…</>, <div>"), "{msg}");
}

/// Inside an element siblings need no wrapper, so the form is a mistake —
/// or a literal `<` — and both readings are answered.
#[test]
fn a_fragment_inside_an_element_is_rejected() {
    let msg = error(r#"<div><><p>"a"</p></></div>"#);
    assert!(msg.contains("a fragment is a block's root"), "{msg}");
    assert!(msg.contains("&lt;"), "{msg}");
}

#[test]
fn an_unclosed_fragment_is_reported() {
    assert!(error(r#"<><p>"a"</p>"#).contains("`<>` is never closed"));
}

// ------------------------------------------------------------- single root

#[test]
fn two_root_elements_are_rejected() {
    let msg = error(r#"<h1>"a"</h1><p>"b"</p>"#);
    assert!(msg.contains("single parent element"), "{msg}");
    assert!(msg.contains("<h1>, <p>"), "{msg}");
}

#[test]
fn text_outside_the_root_is_rejected() {
    assert!(error(r#""hello "<p>"a"</p>"#).contains("sits outside the parent element"));
}

#[test]
fn interpolation_outside_the_root_is_rejected() {
    assert!(error(r#"@{title}<p>"a"</p>"#).contains("`@{expr}`"));
    assert!(error(r#"{count}<p>"a"</p>"#).contains("`{expr}`"));
}

#[test]
fn a_slot_outside_the_root_is_rejected() {
    assert!(error(r#"<slot /><p>"a"</p>"#).contains("`<slot />`"));
}

#[test]
fn an_empty_block_is_rejected() {
    assert!(error("").contains("empty"));
}

/// The prologue is not an element, so it does not count against the rule.
#[test]
fn a_doctype_may_precede_the_root() {
    assert_eq!(
        render(r#"<!DOCTYPE html><html>"x"</html>"#),
        "<!DOCTYPE html><html>x</html>"
    );
}

// ------------------------------------------------------------------ errors

#[test]
fn unclosed_element() {
    assert!(error(r#"<div><p>"a"</p>"#).contains("`<div>` is never closed"));
}

#[test]
fn mismatched_closing_tag() {
    assert!(error(r#"<div>"a"</span>"#).contains("`<div>` is closed by `</span>`"));
}

#[test]
fn closing_tag_without_an_opening_one() {
    assert!(
        error(r#"<div>"a"</div></div>"#).contains("closing tag without a matching opening tag")
    );
}

#[test]
fn unterminated_tag() {
    assert!(error(r#"<div class="a""#).contains("unterminated tag"));
}

/// A `<` that opens nothing is text, and is escaped on the way out.
#[test]
fn a_less_than_sign_before_a_space_is_text() {
    assert_eq!(render(r#"<p>"a < 2"</p>"#), "<p>a &lt; 2</p>");
}

/// `a <b` opens the tag `<b`, exactly as in HTML, which is never closed
/// here. A literal `<` before a letter is written `&lt;`.
#[test]
fn a_less_than_sign_before_a_word_opens_a_tag() {
    let msg = error(r#"<p>"a "<b</p>"#);
    assert!(msg.contains("attribute name"), "{msg}");
    assert_eq!(render(r#"<p>"a &lt;b"</p>"#), "<p>a &lt;b</p>");
}

#[test]
fn attribute_without_a_usable_value() {
    assert!(error(r#"<div class=>"x"</div>"#).contains("`class` needs a string literal"));
}

#[test]
fn unclosed_script() {
    assert!(error(r###"<script>r##"const a = 1;"##"###).contains("never closed"));
}

#[test]
fn unterminated_doctype() {
    assert!(error(r#"<!DOCTYPE html"#).contains("unterminated doctype"));
}

// -------------------------------------------------- naming a block by hand

#[test]
fn a_hand_written_component_name_is_recorded() {
    let parsed = parse_ok(r#"<div pp-component="my_block">"x"</div>"#);
    assert!(parsed.declares_component);
}

#[test]
fn a_component_name_must_be_a_javascript_identifier() {
    let msg = error(r#"<div pp-component="my-block">"x"</div>"#);
    assert!(msg.contains("not a valid JavaScript identifier"), "{msg}");
    // The suggestion has to be usable as written.
    assert!(msg.contains("`my_block`"), "{msg}");
}

#[test]
fn a_reserved_word_is_not_a_component_name() {
    assert!(
        error(r#"<div pp-component="class">"x"</div>"#)
            .contains("not a valid JavaScript identifier")
    );
}

/// `#[component]` writes this in front of the markup; nobody types it.
#[test]
fn the_component_directive_names_the_block() {
    let parsed = parse_ok(r#"@component user_card_3b91f0ae <article>"x"</article>"#);
    assert_eq!(
        parsed.forced_component.as_deref(),
        Some("user_card_3b91f0ae")
    );
    assert_eq!(show(&parsed.ops), "<article>x</article>");
}

#[test]
fn the_root_element_offers_a_slot_for_its_name() {
    let parsed = parse_ok(r#"<article class="card">"x"</article>"#);
    let (index, offset, kind) = parsed
        .component_slot
        .expect("the root element takes a name");
    let Op::Static(markup) = &parsed.ops[index] else {
        panic!("the slot points at static markup");
    };
    // Just past `<article`, so the name lands before any attribute.
    assert_eq!(&markup[..offset], "<article");
    assert_eq!(kind, SlotKind::Element);
}

/// An interpolated attribute cuts the open tag in two, and the name has to
/// stay with the first half.
#[test]
fn the_slot_survives_an_interpolated_attribute() {
    let parsed = parse_ok(r#"<article class=@{class}>"x"</article>"#);
    let (index, offset, _) = parsed
        .component_slot
        .expect("the root element takes a name");
    let Op::Static(markup) = &parsed.ops[index] else {
        panic!("the slot points at static markup");
    };
    assert_eq!(&markup[..offset], "<article");
}

#[test]
fn only_the_root_element_takes_a_name() {
    let parsed = parse_ok(r#"<main><article>"x"</article></main>"#);
    let (index, offset, _) = parsed
        .component_slot
        .expect("the root element takes a name");
    let Op::Static(markup) = &parsed.ops[index] else {
        panic!("the slot points at static markup");
    };
    assert_eq!(&markup[..offset], "<main");
}

// --------------------------------------------------------- string literals

#[test]
fn unquote_reads_escapes() {
    assert_eq!(unquote("\"a\\nb\"").unwrap(), "a\nb");
    assert_eq!(unquote("\"a\\tb\"").unwrap(), "a\tb");
    assert_eq!(unquote("\"a\\\\b\"").unwrap(), "a\\b");
    assert_eq!(unquote("\"a\\\"b\"").unwrap(), "a\"b");
    assert_eq!(unquote("\"\\x41\"").unwrap(), "A");
    assert_eq!(unquote("\"\\u{1F600}\"").unwrap(), "\u{1F600}");
}

/// A backslash before a newline swallows the indentation that follows.
#[test]
fn unquote_folds_a_line_continuation() {
    assert_eq!(unquote("\"a\\\n    b\"").unwrap(), "ab");
}

#[test]
fn unquote_reads_raw_strings() {
    assert_eq!(unquote("r\"a\\nb\"").unwrap(), "a\\nb");
    assert_eq!(unquote("r#\"a \"b\" c\"#").unwrap(), "a \"b\" c");
    assert_eq!(unquote("r##\"a \"# b\"##").unwrap(), "a \"# b");
}

#[test]
fn unquote_rejects_what_is_not_a_string() {
    assert_eq!(unquote("42"), None);
    assert_eq!(unquote("\""), None);
}

// ------------------------------------------------------------- escaping

#[test]
fn entity_len_measures_a_reference() {
    assert_eq!(entity_len("&amp;"), Some(5));
    assert_eq!(entity_len("&nbsp; rest"), Some(6));
    assert_eq!(entity_len("&#160;"), Some(6));
    assert_eq!(entity_len("&#x2014;"), Some(8));
    assert_eq!(entity_len("&#X2014;"), Some(8));
}

#[test]
fn entity_len_rejects_what_is_not_one() {
    assert_eq!(entity_len("&amp"), None, "no semicolon");
    assert_eq!(entity_len("&;"), None, "no name");
    assert_eq!(entity_len("& amp;"), None, "a bare ampersand");
    assert_eq!(entity_len("&#;"), None, "no digits");
    assert_eq!(entity_len("&#xzz;"), None, "not hexadecimal");
}

#[test]
fn escape_text_leaves_ordinary_characters_alone() {
    assert_eq!(escape_text("Ada & Ada"), "Ada &amp; Ada");
    assert_eq!(escape_text("héllo — ok"), "héllo — ok");
    assert_eq!(
        escape_text("\"quoted\""),
        "\"quoted\"",
        "text position keeps quotes"
    );
}

#[test]
fn is_brace_entity_knows_the_forms_the_runtime_does() {
    for entity in [
        "&#123;", "&#125;", "&#x7b;", "&#X7D;", "&lbrace;", "&RBRACE;",
    ] {
        assert!(is_brace_entity(entity), "{entity}");
    }
    for entity in ["&amp;", "&#160;", "&#0123;", "&lbrac;"] {
        assert!(!is_brace_entity(entity), "{entity}");
    }
}

#[test]
fn escape_attr_covers_the_quote() {
    assert_eq!(escape_attr("a\"b"), "a&quot;b");
    assert_eq!(escape_attr("<&>"), "&lt;&amp;&gt;");
    assert_eq!(
        escape_attr("&amp;"),
        "&amp;amp;",
        "an attribute escapes unconditionally"
    );
}

#[test]
fn suggest_identifier_offers_something_usable() {
    assert_eq!(suggest_identifier("my-block"), "my_block");
    assert_eq!(suggest_identifier("2cool"), "_2cool");
    assert_eq!(suggest_identifier("a b c"), "a_b_c");
    // Sanitising a reserved word leaves it reserved, so it gains a tail.
    assert_eq!(suggest_identifier("class"), "class_");
    assert!(crate::component::is_valid_js_identifier(
        &suggest_identifier("[[...slug]]")
    ));
}

// ---------------------------------------------- server values in client code

/// A server value as a native handler: the browser decodes the HTML escaping
/// before the handler runs, so the value could end its string and execute.
#[test]
fn a_server_value_cannot_be_a_handler() {
    for src in [
        r#"<button onclick=@{handler}>"x"</button>"#,
        r#"<form onsubmit={save(event)} onclick=@{handler}>"x"</form>"#,
    ] {
        let msg = error(src);
        assert!(
            msg.contains("makes a server value a JavaScript handler"),
            "{msg}"
        );
        assert!(msg.contains("@{Json(&name)}"), "{msg}");
    }
}

/// `@{…}` inside a client binding is copied through as browser source, never
/// interpolated — in text, in an attribute, nested in a call, or in a
/// handler.
#[test]
fn a_server_value_inside_client_code_is_refused() {
    for src in [
        "<p>{greet(@{name})}</p>",
        r#"<p title={label + @{suffix}}>"x"</p>"#,
        r#"<button onclick={pick(@{id})}>"x"</button>"#,
        "<p>{items.filter((i) => i.id === @{id})}</p>",
    ] {
        let msg = error(src);
        assert!(msg.contains("is not interpolated"), "{src}: {msg}");
    }
}

/// The refusals must not catch the ordinary forms around them.
#[test]
fn server_values_beside_client_code_still_parse() {
    parse_ok(r#"<button onclick={pick(id)} title=@{name}>@{label}</button>"#);
    parse_ok(r#"<p data-email="a@b.c" on-select={choose}>{user.email}</p>"#);
    parse_ok(r#"<input value={draft} oninput={setDraft(event.target.value)} />"#);
}

// ---------------------------------------------------------------- context

/// A context provider is a lowercase `<token.provider>` tag, the form the
/// runtime reads; the dot is part of the tag name, closing tag included.
#[test]
fn a_context_provider_tag_keeps_its_dotted_name() {
    let out = render(
        r#"<section><themecontext.provider value={theme}><p>"in"</p></themecontext.provider></section>"#,
    );
    assert!(
        out.contains("<themecontext.provider value=\"{theme}\">"),
        "{out}"
    );
    assert!(out.contains("</themecontext.provider>"), "{out}");
}

#[test]
fn a_provider_tag_must_close_with_the_same_name() {
    error(r#"<section><themecontext.provider value={theme}></themecontext></section>"#);
}

// ------------------------------------------------- authoring refusals

fn refusal(src: &str) -> String {
    let stream = match template(src) {
        Ok(tokens) => tokens,
        Err(msg) => return msg,
    };
    match parse(stream) {
        Ok(_) => panic!("expected `{src}` to be refused"),
        Err(err) => err.msg,
    }
}

/// A binding renders text; JSX markup inside one is refused with the
/// PulsePoint form to write instead.
#[test]
fn jsx_markup_in_a_binding_is_refused() {
    for src in [
        "<div>{open && <p>hi</p>}</div>",
        "<ul>{items.map(i => <li>{i}</li>)}</ul>",
        "<div>{open ? <br/> : null}</div>",
        r#"<div title={open && <b>x</b>}></div>"#,
    ] {
        let msg = refusal(src);
        assert!(msg.contains("renders text, never elements"), "{src}: {msg}");
        assert!(msg.contains("pp-for"), "{msg}");
    }
}

/// Comparisons, arrows, division and strings are ordinary JavaScript.
#[test]
fn ordinary_expressions_are_not_mistaken_for_jsx() {
    for src in [
        "<p>{a < b ? \"less\" : \"more\"}</p>",
        "<p>{a <= b && c >= d}</p>",
        "<p>{(total / 2) > limit}</p>",
        "<p>{items.filter((i) => i.done).length}</p>",
        "<p>{\"</not a tag>\"}</p>",
        "<p hidden={count < max}></p>",
    ] {
        parse_ok(src);
    }
}

#[test]
fn jsx_attribute_names_are_refused_with_the_html_name() {
    for (src, html) in [
        ("<div className=\"card\"></div>", "class"),
        ("<label htmlFor=\"x\"></label>", "for"),
        ("<button onClick={save()}></button>", "onclick"),
        ("<input defaultValue={name} />", "defaultvalue"),
        ("<input defaultChecked={on} />", "defaultchecked"),
    ] {
        let msg = refusal(src);
        assert!(msg.contains(&format!("write `{html}`")), "{src}: {msg}");
    }
    assert!(refusal(r#"<div dangerouslySetInnerHTML={x}></div>"#).contains("Html::from_raw"));
}

/// SVG's camelCase attributes are real HTML, not JSX.
#[test]
fn svg_attributes_keep_their_case() {
    parse_ok(r#"<svg viewBox="0 0 10 10" preserveAspectRatio="none"></svg>"#);
}

#[test]
fn invented_directives_are_refused() {
    for (directive, hint) in [
        ("pp-if", "hidden={…}"),
        ("pp-show", "hidden={…}"),
        ("pp-model", "oninput"),
        ("pp-class", "class={…}"),
        ("pp-key", "key={…}"),
        ("pp-context", "<token.provider"),
    ] {
        let msg = refusal(&format!(r#"<div {directive}={{x}}></div>"#));
        assert!(
            msg.contains("is not a PulsePoint directive"),
            "{directive}: {msg}"
        );
        assert!(msg.contains(hint), "{directive}: {msg}");
    }
}

#[test]
fn pp_for_goes_only_on_a_template() {
    let msg = refusal(r#"<ul><li pp-for="item in items">{item}</li></ul>"#);
    assert!(msg.contains("only on a `<template>`"), "{msg}");
    parse_ok(r#"<ul><template pp-for="item in items"><li key={item}>{item}</li></template></ul>"#);
}

#[test]
fn a_style_object_is_refused_for_css_text() {
    let msg = refusal(r#"<p style={{color: "red"}}></p>"#);
    assert!(msg.contains("takes CSS text, not an object"), "{msg}");
    parse_ok(r#"<p pp-style={"color: " + color}></p>"#);
}

// ------------------------------------------------------------ the token form

/// A template is markup written as tokens. A whole page in one raw string is
/// refused with the form to write instead.
#[test]
fn a_string_template_is_refused() {
    let msg = error(r###"r##"<section class="page">Hello</section>"##"###);
    assert!(msg.contains("written as tokens, not a string"), "{msg}");
    assert!(msg.contains("<script>r##"), "{msg}");
}

/// A `<script>` or `<style>` body is one raw string; JavaScript written as
/// tokens is refused, because the Rust lexer cannot carry it.
#[test]
fn a_token_written_body_is_refused() {
    for src in [
        r#"<section><script>const n = 1;</script></section>"#,
        r#"<section><style>.a { color: red; }</style></section>"#,
    ] {
        let msg = error(src);
        assert!(msg.contains("body is one raw string"), "{src}: {msg}");
    }
}

/// The raw body reaches the browser exactly as written: single quotes,
/// template literals and comments are JavaScript, not Rust.
#[test]
fn a_raw_body_is_written_as_it_is() {
    let out = script_source(&parse_ok(
        r###"<section><script>r##"
            // a comment
            const label = 'it\'s';
            const greeting = `hi ${label}`;
        "##</script></section>"###,
    ));
    assert!(out.contains("// a comment"), "{out}");
    assert!(out.contains(r"const label = 'it\'s';"), "{out}");
    assert!(out.contains("`hi ${label}`"), "{out}");
}

/// A handler is client code, so it is a `{…}` binding — on an element and
/// on a component tag alike. A quoted handler is refused with that form.
#[test]
fn a_quoted_handler_is_refused() {
    for src in [
        r#"<button onclick="save()">"Save"</button>"#,
        r#"<div><Button onclick="save()">"Save"</Button></div>"#,
    ] {
        let msg = error(src);
        assert!(msg.contains("write it as a binding"), "{src}: {msg}");
        assert!(msg.contains("onclick={save()}"), "{src}: {msg}");
    }
}

/// A binding's JavaScript reaches the page as the author spelled it, spacing
/// and all — the token's own source, not a respaced token stream.
#[test]
fn a_binding_keeps_its_source_spelling() {
    let out = render(r#"<p class={open ? "a" : "b"} hidden={!open}>{items.length+1}</p>"#);
    assert!(
        out.contains(r#"class="{open ? &quot;a&quot; : &quot;b&quot;}""#),
        "{out}"
    );
    assert!(out.contains(r#"hidden="{!open}""#), "{out}");
    assert!(out.contains("{items.length+1}"), "{out}");
}

/// Quoted text is verbatim, apostrophes included; bare words are read too.
#[test]
fn text_is_quoted_or_bare() {
    assert_eq!(render(r#"<p>"It's here."</p>"#), "<p>It's here.</p>");
    assert_eq!(render("<p>Hello world</p>"), "<p>Hello world</p>");
}

/// `r##"…"##` is the one spelling for a body: fewer `#`s is refused with that
/// form, so a `"` or a `"#` — `"#fff"`, `querySelector("#id")` — can never
/// end the body early. A body that contains `"##` takes a third `#`.
#[test]
fn a_body_is_written_with_two_hashes() {
    for src in [
        r##"<section><script>r"const n = 1;"</script></section>"##,
        r###"<section><script>r#"const n = 1;"#</script></section>"###,
        r###"<section><style>r#".a { color: red; }"#</style></section>"###,
    ] {
        let msg = error(src);
        assert!(msg.contains("with two `#`s"), "{src}: {msg}");
        assert!(msg.contains("r##\"…\"##"), "{src}: {msg}");
    }
    let out = script_source(&parse_ok(
        r###"<section><script>r##"const c = "#fff";"##</script></section>"###,
    ));
    assert_eq!(out, r##"const c = "#fff";"##);
    let out = script_source(&parse_ok(
        r####"<section><script>r###"const s = "a"##b";"###</script></section>"####,
    ));
    assert_eq!(out, r###"const s = "a"##b";"###);
}

#[test]
fn inert_event_metadata_is_generated_instead_of_authored() {
    let message = error(r#"<button pp-onclick={save()}>"Save"</button>"#);
    assert!(
        message.contains("compiler-owned event metadata"),
        "{message}"
    );
    assert!(message.contains("onclick={handler(event)}"), "{message}");
    assert!(
        error(r#"<button PP-onclick={save()}>"Save"</button>"#)
            .contains("compiler-owned event metadata")
    );
    parse_ok(r#"<button onclick={save()}>"Save"</button>"#);
}

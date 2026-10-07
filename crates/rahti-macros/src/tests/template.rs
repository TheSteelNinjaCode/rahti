//! `html! {r##"…"##}`: a template written as HTML text.

use super::*;
use crate::parse::{self, Op};
use std::str::FromStr;

/// The `html!` input for a template literal, as the macro receives it.
fn input(template: &str) -> TokenStream {
    TokenStream::from_str(&format!("r##\"{template}\"##")).expect("the template is one literal")
}

fn ops(template: &str, file: &str) -> Result<Vec<Op>, String> {
    let tokens = crate::template_tokens(input(template), file, |_| (10, 9)).map_err(|e| e.msg)?;
    let parsed = parse::parse(tokens).map_err(|e| e.msg)?;
    Ok(crate::stamp(parsed, file))
}

fn join(ops: &[Op]) -> String {
    ops.iter()
        .map(|op| match op {
            Op::Static(s) => s.clone(),
            Op::Text(expr) => format!("@{{{expr}}}"),
            Op::ScriptText(expr) => format!("@js{{{expr}}}"),
            Op::Attr { name, expr, .. } => format!("{name}=@{{{expr}}}"),
            Op::OpenTag(_) => "<open-tag>".to_string(),
            Op::Slot | Op::DeferredSlot => "<slot/>".to_string(),
            Op::Component(call) => {
                let props: String = call
                    .props
                    .iter()
                    .map(|(name, value)| format!(" {name}={value}"))
                    .collect();
                let bindings: String = call
                    .bindings
                    .iter()
                    .map(|(name, expr)| format!(" {name}={{{expr}}}"))
                    .collect();
                match &call.children {
                    None => format!("<{}{props}{bindings}/>", call.name),
                    Some(child) => {
                        format!(
                            "<{}{props}{bindings}>{}</{}>",
                            call.name,
                            join(&child.ops),
                            call.name
                        )
                    }
                }
            }
        })
        .collect()
}

fn render(template: &str) -> String {
    join(&ops(template, "src/lib/widgets.rs").unwrap_or_else(|e| panic!("{e}")))
}

fn refusal(template: &str) -> String {
    match ops(template, "src/app/shop/page.rs") {
        Ok(ops) => panic!("expected a refusal, it rendered:\n{}", join(&ops)),
        Err(msg) => msg,
    }
}

// ------------------------------------------------------------------ text

/// Text is HTML text: apostrophes and quotes need nothing, and the
/// indentation of the markup disappears the way JSX drops it.
#[test]
fn text_is_written_plainly() {
    assert_eq!(
        render(
            "
            <p>
                Don't panic — it's \"fine\".
            </p>
        "
        ),
        "<p>Don&#39;t panic — it&#39;s \"fine\".</p>".replace("&#39;", "'")
    );
}

/// A space typed on one line stays; wrapped lines join with one space.
#[test]
fn spaces_follow_jsx() {
    assert_eq!(
        render("<p>Hello <b>world</b> again\n   and again</p>"),
        "<p>Hello <b>world</b> again and again</p>"
    );
}

/// `<pre>` keeps its whitespace.
#[test]
fn pre_keeps_its_whitespace() {
    assert_eq!(render("<pre>  a\n    b</pre>"), "<pre>  a\n    b</pre>");
}

/// A template comment is the author's note, not markup.
#[test]
fn html_comments_are_not_sent() {
    assert_eq!(
        render("<div><!-- todo: <b>x</b> --><p>a</p></div>"),
        "<div><p>a</p></div>"
    );
}

/// `@@{` is a literal `@{`.
#[test]
fn a_doubled_at_is_a_literal() {
    assert_eq!(
        render("<p>write @@{expr} here</p>"),
        "<p>write @{expr} here</p>"
    );
}

// ------------------------------------------------------------ the dialects

/// `@{…}` is Rust and `{…}` is PulsePoint, in text.
#[test]
fn both_dialects_in_text() {
    assert_eq!(
        render("<p>Hi @{user.name}, you have {count} items</p>"),
        "<p>Hi @{user . name}, you have {count} items</p>"
    );
}

/// A binding holding JavaScript the Rust lexer rejects arrives verbatim.
#[test]
fn a_binding_keeps_its_javascript() {
    assert_eq!(
        render("<p>{open ? 'Hide' : `Show ${n}`}</p>"),
        "<p>{open ? 'Hide' : `Show ${n}`}</p>"
    );
}

/// A whole quoted attribute value is a binding; a server value is unquoted.
#[test]
fn attributes_in_both_dialects() {
    assert_eq!(
        render(
            "<button class=\"btn\" hidden=\"{!open}\" title=@{label} onclick=\"setOpen(!open)\" disabled>Go</button>"
        ),
        "<button class=\"btn\" hidden=\"{!open}\" title=@{label} onclick=\"{setOpen(!open)}\" disabled>Go</button>"
    );
}

/// A script body is JavaScript exactly as written, with `@{…}` lifted.
#[test]
fn a_script_is_plain_javascript() {
    let out = render(
        "<section><script>
            // kept
            const color = \"#fff\";
            const label = 'single';
            const seed = @{Json(&items)};
        </script></section>",
    );
    assert!(out.contains("// kept"), "{out}");
    assert!(out.contains("const color = \"#fff\";"), "{out}");
    assert!(out.contains("const label = 'single';"), "{out}");
    assert!(out.contains("@js{Json (& items)}"), "{out}");
}

/// A style body is CSS exactly as written.
#[test]
fn a_style_is_plain_css() {
    assert_eq!(
        render("<div><style>a::after { content: '→'; }</style></div>"),
        "<div><style>a::after { content: '→'; }</style></div>"
    );
}

/// Components, props, client props, children and slots read as before.
#[test]
fn components_slots_and_children() {
    assert_eq!(
        render(
            "<main><Card title=\"Hi\" count=@{3} onclick=\"{save()}\" wide><p>{x}</p></Card><slot /></main>"
        ),
        "<main><Card title=\"Hi\" count=(3) wide=true onclick={save()}><p>{x}</p></Card><slot/></main>"
    );
}

/// A root layout's document: doctype, `<html>`, and the slot it wraps.
#[test]
fn a_document() {
    let out = join(
        &ops(
            "<!DOCTYPE html>\n<html lang=\"en\"><body><slot /></body></html>",
            "src/app/layout.rs",
        )
        .unwrap_or_else(|e| panic!("{e}")),
    );
    assert!(
        out.starts_with("<!DOCTYPE html><html lang=\"en\"><body><template pp-component="),
        "{out}"
    );
}

/// `#[component]`'s directive rides in front of the literal.
#[test]
fn the_component_directive_is_kept() {
    let tokens = crate::template_tokens(
        TokenStream::from_str("@component \"card_1\" r##\"<div>a</div>\"##").unwrap(),
        "src/components/card.rs",
        |_| (1, 1),
    )
    .unwrap_or_else(|e| panic!("{}", e.msg));
    let parsed = parse::parse(tokens).unwrap_or_else(|e| panic!("{}", e.msg));
    assert_eq!(parsed.forced_component.as_deref(), Some("card_1"));
}

// ------------------------------------------------------------- refusals

/// Every refusal names the file, line and column inside the template. The
/// literal starts at line 10, column 9; the text after `r##"` at column 13.
#[test]
fn refusals_name_their_position() {
    let msg = refusal("<div>\n  <p className=\"a\">x</p>\n</div>");
    assert!(
        msg.starts_with("rahti: src/app/shop/page.rs:11:6 — "),
        "{msg}"
    );
    assert!(msg.contains("`className` is JSX"), "{msg}");

    let msg = refusal("<div class={x}>a</div>");
    assert!(
        msg.starts_with("rahti: src/app/shop/page.rs:10:24 — "),
        "{msg}"
    );
    assert!(
        msg.contains("write it quoted") || msg.contains("is unquoted"),
        "{msg}"
    );
}

#[test]
fn jsx_in_a_binding_is_refused() {
    let msg = refusal("<div>{open && <b>x</b>}</div>");
    assert!(msg.contains("renders text, never elements"), "{msg}");
}

#[test]
fn a_server_value_in_client_code_is_refused() {
    assert!(refusal("<p>{greet(@{name})}</p>").contains("is not interpolated"));
    assert!(refusal("<button onclick=@{h}>x</button>").contains("JavaScript handler"));
    assert!(
        refusal("<button onclick=\"pick(@{id})\">x</button>").contains("quoted `onclick` handler")
    );
    assert!(refusal("<p title=\"Hi @{name}\">x</p>").contains("never interpolated"));
}

#[test]
fn a_style_object_is_refused() {
    assert!(refusal("<p pp-style=\"{{color: 'red'}}\">x</p>").contains("takes CSS text"));
}

#[test]
fn an_unknown_directive_is_refused() {
    assert!(refusal("<p pp-if=\"{x}\">x</p>").contains("not a PulsePoint directive"));
}

#[test]
fn unclosed_things_are_named() {
    assert!(refusal("<div><p>a</div>").contains("closed by"));
    assert!(refusal("<div>{open</div>").contains("never closed"));
    assert!(refusal("<div><!-- note</div>").contains("never closed"));
}

/// A component's handler is a binding; a plain quoted one would be a Rust prop.
#[test]
fn a_component_handler_is_a_binding() {
    let msg = refusal("<main><Card onclick=\"save()\" /></main>");
    assert!(msg.contains("`onclick=\"{save()}\"`"), "{msg}");
}

/// The template form is the only form: tokens, or a string that is not raw,
/// get the form to write.
#[test]
fn only_a_raw_string_is_a_template() {
    for source in ["<div>\"a\"</div>", "\"<div>a</div>\""] {
        let err = crate::template_tokens(
            TokenStream::from_str(source).unwrap(),
            "src/app/page.rs",
            |_| (1, 1),
        )
        .err()
        .unwrap_or_else(|| panic!("{source} was accepted"));
        assert!(
            err.msg.contains("html! {r##\"<section>…</section>\"##}"),
            "{}",
            err.msg
        );
    }
}

/// A component's unquoted prop is the Rust value it spells; a quoted one is a
/// string.
#[test]
fn component_props_keep_their_rust_types() {
    assert_eq!(
        render("<main><Card count=3 ratio=0.5 wide=false label=\"3\" /></main>"),
        "<main><Card count=3 ratio=0.5 wide=(false) label=\"3\"/></main>"
    );
}

/// Every template the convention documents teach is one `html!` accepts: the
/// lexer and grammar read each `html! {r##"…"##}` in their Rust code blocks.
/// A document that drifts from the macro fails here, naming file and line.
#[test]
fn the_documented_templates_are_templates() {
    let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/conventions");
    let Ok(entries) = std::fs::read_dir(&docs) else {
        return; // outside the workspace, where the documents live
    };
    let mut checked = 0;
    let mut failures = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for (line, template) in documented_templates(&text) {
            checked += 1;
            let result = crate::template_tokens(
                TokenStream::from_str(&template).expect("a template literal"),
                "src/app/page.rs",
                |_| (1, 1),
            )
            .map_err(|e| e.msg)
            .and_then(|tokens| parse::parse(tokens).map(|_| ()).map_err(|e| e.msg));
            if let Err(msg) = result {
                failures.push(format!("{name}:{line}: {msg}"));
            }
        }
    }
    assert!(
        checked > 30,
        "found only {checked} documented templates in {}",
        docs.display()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// `(line, literal)` for each top-level `html! {r#…"…"#…}` inside a Rust code
/// block of a Markdown document.
fn documented_templates(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    // Code blocks, by fence pairs.
    let mut rest = text;
    let mut base = 0;
    while let Some(open) = rest.find("```rust") {
        let body_start = open + rest[open..].find('\n').map_or(rest.len() - open, |n| n + 1);
        let Some(close) = rest[body_start..].find("```") else {
            break;
        };
        let body = &rest[body_start..body_start + close];
        let mut at = 0;
        while let Some(found_at) = body[at..].find("html! {r") {
            let start = at + found_at + "html! {".len();
            let hashes = body[start + 1..].chars().take_while(|c| *c == '#').count();
            let terminator = format!("\"{}}}", "#".repeat(hashes));
            if let Some(end) = body[start..].find(&terminator) {
                let literal = &body[start..start + end + 1 + hashes];
                let line = text[..base + body_start + start].matches('\n').count() + 1;
                found.push((line, literal.to_string()));
                at = start + end + terminator.len();
            } else {
                break;
            }
        }
        let consumed = body_start + close + 3;
        base += consumed;
        rest = &rest[consumed..];
    }
    found
}

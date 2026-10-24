//! Authoring mistakes `html!` refuses at compile time, with the form to
//! write instead.
//!
//! PulsePoint markup is HTML with `{…}` bindings, and its script API mirrors
//! React. Code written from JSX habit compiles as HTML and then fails in the
//! browser without an error: `className` is an attribute nothing reads, and
//! `pp-if` is a directive that does not exist. The parser (`parse.rs`) asks
//! these questions as it reads each attribute and binding, and reports the
//! refusal on the token that caused it.

const NOT_JSX: &str = "See \"PulsePoint Is Not JSX\" in docs/conventions/pulsepoint.md.";

/// An attribute name on a native element that comes from JSX, or a
/// directive PulsePoint does not have.
pub fn attribute_mistake(tag: &str, name: &str) -> Option<String> {
    let normalized = name.to_ascii_lowercase();
    if let Some(event) = normalized.strip_prefix("pp-")
        && crate::parse::is_native_event_name(event)
    {
        return Some(format!(
            "rahti: `{name}` is compiler-owned event metadata.\n  Write `{event}={{handler(event)}}`; html! emits the inert attribute and registers the handler."
        ));
    }
    let rename = |write: &str, why: &str| {
        Some(format!(
            "rahti: `{name}` is JSX; this is HTML — write `{write}`.\n  {why} {NOT_JSX}"
        ))
    };
    match name {
        "className" => return rename("class", "HTML has no `className` attribute."),
        "htmlFor" => return rename("for", "HTML has no `htmlFor` attribute."),
        "defaultValue" => {
            return rename(
                "defaultvalue",
                "The lowercase attribute seeds an uncontrolled field once.",
            );
        }
        "defaultChecked" => {
            return rename(
                "defaultchecked",
                "The lowercase attribute seeds an uncontrolled checkbox once.",
            );
        }
        "dangerouslySetInnerHTML" => {
            return Some(format!(
                "rahti: `dangerouslySetInnerHTML` does not exist.\n  Render trusted markup on the \
                 server with `@{{Html::from_raw(…)}}`; a `{{…}}` binding is always escaped. \
                 {NOT_JSX}"
            ));
        }
        _ => {}
    }
    if let Some(event) = name.strip_prefix("on")
        && event.starts_with(|c: char| c.is_ascii_uppercase())
    {
        let lowercase = name.to_ascii_lowercase();
        return rename(
            &lowercase,
            "Event attributes are the native lowercase DOM names.",
        );
    }

    let directive = |instead: &str| {
        Some(format!(
            "rahti: `{name}` is not a PulsePoint directive — PulsePoint has no `pp-if`, \
             `pp-show`, `pp-else`, `pp-model`, `pp-bind`, `pp-class`, `pp-text`, `pp-html`, \
             `pp-on`, `pp-key` or `pp-context`.\n  {instead} See \"Complete Directive and API \
             Surface\" in docs/conventions/pulsepoint.md."
        ))
    };
    match name {
        "pp-if" | "pp-show" | "pp-else" => return directive("Bind the element's `hidden={…}`."),
        "pp-model" | "pp-bind" => {
            return directive("Bind `value={state}` and update it from `oninput`.");
        }
        "pp-class" => return directive("Bind `class={…}`."),
        "pp-text" => return directive("Write the `{…}` binding as the element's text."),
        "pp-html" => {
            return directive("Render trusted markup on the server with `@{Html::from_raw(…)}`.");
        }
        "pp-on" => return directive("Write the native `on…` attribute, such as `onclick={…}`."),
        "pp-key" => return directive("Write a plain `key={…}` on the repeated element."),
        "pp-context" => {
            return directive(
                "Provide a value with a lowercase `<token.provider value={…}>` tag and read it \
                 with `pp.context(token)`.",
            );
        }
        _ => {}
    }

    if name == "pp-for" && tag != "template" {
        return Some(format!(
            "rahti: `pp-for` goes only on a `<template>`, not on `<{tag}>`.\n  Wrap the repeated \
             element: `<template pp-for=\"item in items\"><{tag} key={{item.id}}>…</{tag}></template>`."
        ));
    }
    None
}

const SERVER_VALUE_FIX: &str = "Hand the value to the script as data — `const name = \
     @{Json(&name)};` inside `<script>`, or `pp.state(@{Json(&name)})` — and write the \
     name in the binding: `onclick={pick(name)}`.";

/// `onclick=@{handler}` — a server value as a native handler. The runtime
/// runs an `on…` attribute as JavaScript, and HTML escaping is not
/// JavaScript escaping: the browser decodes `&#39;` back to `'` before the
/// handler runs, so a value that reaches one can close its string and execute.
pub fn server_value_as_handler(name: &str) -> String {
    format!(
        "rahti: `{name}=@{{…}}` makes a server value a JavaScript handler.\n  The browser \
         decodes the HTML escaping before the handler runs, so the value can end its string \
         and execute. Write the handler as a client binding, `{name}={{…}}`. \
         {SERVER_VALUE_FIX}"
    )
}

/// `onclick="save()"` — a handler written as a string. A handler is client
/// code, and every piece of client code in a template is a `{…}` binding.
pub fn handler_as_text(name: &str) -> String {
    format!(
        "rahti: `{name}` is a handler, and a handler is client code: write it as a binding, \
         `{name}={{save()}}`.\n  A quoted value is text. {NOT_JSX}"
    )
}

/// `title="Hi @{name}"` — `@{…}` inside a quoted value, where it is text.
pub fn server_value_in_text(name: &str) -> String {
    format!(
        "rahti: `@{{…}}` inside the quoted value of `{name}` is text, never interpolated.\n  A \
         server value is the whole value, unquoted: `{name}=@{{expr}}` — build the text in \
         Rust (`@{{format!(…)}}`) when it mixes with literal text."
    )
}

/// The refusals a client `{…}` binding gets: markup inside it, a style
/// object, or a server value. `attr` is the attribute it is the value of, or
/// `None` for a binding in text. `source` is the binding with its braces.
pub fn binding_mistake(attr: Option<&str>, source: &str) -> Option<String> {
    let inner = source
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(source);
    let code = js_outside_strings(inner);
    if code.contains("</") || code.contains("/>") {
        return Some(format!(
            "rahti: a `{{…}}` binding renders text, never elements, and this one contains \
             markup.\n  Show or hide an element with `hidden={{…}}` on the element itself, and \
             repeat one with `<template pp-for=\"item in items\">…</template>`. {NOT_JSX}"
        ));
    }
    if let Some(name @ ("style" | "pp-style")) = attr {
        let trimmed = inner.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            return Some(format!(
                "rahti: `{name}` takes CSS text, not an object.\n  Bind a string: \
                 `pp-style={{\"color: \" + color}}`."
            ));
        }
    }
    if code.contains("@{") || code.contains("@ {") {
        return Some(format!(
            "rahti: `@{{…}}` inside a client `{{…}}` binding is not interpolated: the binding is \
             browser JavaScript, copied through as written.\n  {SERVER_VALUE_FIX}"
        ));
    }
    None
}

/// `js` with the contents of its string and template literals blanked, so a
/// check for markup does not trip on `"</not a tag>"`.
fn js_outside_strings(js: &str) -> String {
    let mut out = String::with_capacity(js.len());
    let mut chars = js.chars();
    while let Some(c) = chars.next() {
        if matches!(c, '"' | '\'' | '`') {
            out.push(c);
            while let Some(inner) = chars.next() {
                if inner == '\\' {
                    chars.next();
                } else if inner == c {
                    break;
                }
            }
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

/// A `{` or `}` inside quoted text or a quoted attribute value.
pub const RAW_BRACE: &str = "rahti: `{…}` inside quoted text holds a JavaScript expression — the \
     browser runtime would read it as client code, and only code written as a binding is \
     compiled into the page.\n  Write a binding outside the quotes, `\"Hi \"{name}` or \
     `class={\"row \" + state}`, or a literal brace as `&#123;` / `&#125;`.";

/// `pp.rpc(name, …)` — an rpc named by anything but a string literal. The
/// name is checked at compile time against the `#[rpc]` it calls, which only
/// a literal can be. `argument` is what was written.
pub fn rpc_name_not_literal(argument: &str) -> String {
    format!(
        "rahti[RAHTI_RPC_NAME_LITERAL]: `pp.rpc` names its `#[rpc]` with a string literal, and `{argument}` is not \
         one.\n  Write the function's name: `pp.rpc(\"save\", {{ … }})`. A call that chooses \
         between rpcs is one literal call per branch: \
         `editing ? pp.rpc(\"update\", {{ … }}) : pp.rpc(\"create\", {{ … }})`."
    )
}

/// `pp.rpc()` — a call that names no rpc at all.
pub fn rpc_name_missing() -> String {
    "rahti[RAHTI_RPC_NAME_MISSING]: `pp.rpc()` names no `#[rpc]`.\n  Write the function's name first, as a string \
     literal: `pp.rpc(\"save\", { … })`."
        .to_string()
}

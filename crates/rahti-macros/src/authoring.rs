//! Authoring mistakes `html!` refuses at compile time, with the form to
//! write instead.
//!
//! PulsePoint markup is HTML with `{…}` bindings, and its script API mirrors
//! React. Code written from JSX habit compiles as HTML and then fails in the
//! browser without an error: `className` is an attribute nothing reads, and
//! `pp-if` is a directive that does not exist. The template lexer
//! (`template.rs`) asks these questions as it reads each attribute, so the
//! refusal carries the file, line and column; it checks bindings themselves —
//! JSX markup, a style object, a server value — where it reads them.

const NOT_JSX: &str = "See \"PulsePoint Is Not JSX\" in docs/conventions/pulsepoint.md.";

/// An attribute name on a native element that comes from JSX, or a
/// directive PulsePoint does not have.
pub fn attribute_mistake(tag: &str, name: &str) -> Option<String> {
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

/// `onclick="pick('@{name}')"` — `@{…}` inside a quoted handler, where it is
/// text and never interpolated.
pub fn server_value_in_handler_text(name: &str, value: &str) -> Option<String> {
    (crate::parse::is_native_event_name(name) && value.contains("@{")).then(|| {
        format!(
            "rahti: `@{{…}}` inside the quoted `{name}` handler is text, never interpolated, and \
             a server value has no place in a handler.\n  {SERVER_VALUE_FIX}"
        )
    })
}

//! What `#[component]` does to the function it is written on.

use super::*;
use proc_macro2::TokenStream;
use std::str::FromStr;

const FILE: &str = "src/components/user_card.rs";

fn ts(src: &str) -> TokenStream {
    TokenStream::from_str(src).expect("test source does not tokenize")
}

/// Expand the attribute over a function, as the compiler would.
fn expanded(item: &str) -> String {
    match expand(TokenStream::new(), ts(item), FILE) {
        Ok(out) => out.to_string(),
        Err(err) => panic!("expected `{item}` to expand, got:\n{}", err.msg),
    }
}

fn error(item: &str) -> String {
    error_with("", item)
}

fn error_with(attr: &str, item: &str) -> String {
    match expand(ts(attr), ts(item), FILE) {
        Ok(out) => panic!("expected a failure, it expanded to:\n{out}"),
        Err(err) => err.msg,
    }
}

/// The name the block should end up carrying.
fn id(function: &str) -> String {
    component::function_identifier(FILE, function)
}

// ------------------------------------------------------------------ naming

#[test]
fn the_markup_is_named_after_the_function() {
    let out = expanded("pub fn UserCard(name: &str) -> Html { html! { <article>x</article> } }");
    assert!(
        out.contains(&format!("@ component \"{}\"", id("UserCard"))),
        "{out}"
    );
}

/// Nothing else about the function changes — the props, the body and the
/// return type are the author's.
#[test]
fn the_function_is_otherwise_untouched() {
    let out = expanded(
        "pub fn UserCard(name: &str, children: Html) -> Html { html! { <article><slot /></article> } }",
    );
    assert!(out.contains("pub fn UserCard"), "{out}");
    assert!(out.contains("name : & str"), "{out}");
    assert!(out.contains("children : Html"), "{out}");
    assert!(out.contains("slot"), "the markup is passed through: {out}");
}

/// PascalCase is the convention and rustc lints against it, so the attribute
/// silences the lint rather than every component file carrying an `#[allow]`.
#[test]
fn the_case_lint_is_silenced() {
    let out = expanded("fn UserCard() -> Html { html! { <p>x</p> } }");
    assert!(out.contains("non_snake_case"), "{out}");
}

#[test]
fn an_async_component_is_expanded_the_same_way() {
    let out = expanded("pub async fn UserCard() -> Html { html! { <article>x</article> } }");
    assert!(out.contains("async fn UserCard"), "{out}");
    assert!(out.contains("@ component"), "{out}");
}

// ------------------------------------------------- which block gets named

/// Both arms are the value the function hands back, so both are its root.
#[test]
fn every_branch_of_the_tail_is_named() {
    let out = expanded(
        "fn Badge(on: bool) -> Html { if on { html! { <b>on</b> } } else { html! { <i>off</i> } } }",
    );
    assert_eq!(out.matches("@ component").count(), 2, "{out}");
}

/// A block bound to a `let` is a fragment this component assembles, not the
/// component itself — naming it too would put one name on two elements.
#[test]
fn a_fragment_built_on_the_way_is_not_named() {
    let out = expanded(
        "fn Badge() -> Html { let inner = html! { <i>x</i> }; html! { <b>@{inner}</b> } }",
    );
    assert_eq!(out.matches("@ component").count(), 1, "{out}");
    // The one that is named is the tail, not the `let`.
    let named = out.find("@ component").unwrap();
    assert!(out[named..].contains("@ { inner }"), "{out}");
}

#[test]
fn an_explicit_return_is_named() {
    let out = expanded("fn Badge() -> Html { return html! { <b>x</b> }; }");
    assert_eq!(out.matches("@ component").count(), 1, "{out}");
}

/// An `html!` interpolated inside the root renders below it, and the root
/// already carries the name.
#[test]
fn a_nested_block_is_not_named_again() {
    let out = expanded("fn Badge() -> Html { html! { <b>@{html! { <i>x</i> }}</b> } }");
    assert_eq!(out.matches("@ component").count(), 1, "{out}");
}

// ------------------------------------------------------- named-props shim

/// Beside the function, the attribute generates the companion a component
/// tag calls: a generic props struct sharing the function's name, whose
/// `__rahti_call` hands the fields to the function in declaration order.
#[test]
fn a_companion_struct_is_generated() {
    let out = expanded(
        "pub fn UserCard(name: &str, children: Html) -> Html { html! { <article><slot /></article> } }",
    );
    assert!(out.contains("pub struct UserCard"), "{out}");
    assert!(out.contains("__rahti_call"), "{out}");
    assert!(out.contains("FnOnce"), "{out}");
}

/// The struct's fields are the parameters, in declaration order — the order
/// `__rahti_call` restores when it hands them to the function.
#[test]
fn the_shim_knows_the_parameters_in_order() {
    let out = expanded(
        "pub fn Card(title: &str, count: usize, children: Html) -> Html { html! { <b>x</b> } }",
    );
    let title = out.find("pub title").expect("a title field");
    let count = out.find("pub count").expect("a count field");
    let children = out.find("pub children").expect("a children field");
    assert!(title < count && count < children, "{out}");
    assert!(
        out.contains("(self . title , self . count , self . children)"),
        "{out}"
    );
}

/// A `mut` pattern still names its parameter.
#[test]
fn a_mut_parameter_is_a_prop_by_its_name() {
    let out = expanded("fn Badge(mut label: String) -> Html { html! { <b>x</b> } }");
    assert!(out.contains("pub label"), "{out}");
}

/// The companion mirrors the function's visibility, so a private page-local
/// component does not export a public struct.
#[test]
fn the_shim_mirrors_the_visibility() {
    let out = expanded("fn Badge(label: &str) -> Html { html! { <b>x</b> } }");
    assert!(!out.contains("pub struct Badge"), "{out}");
    let out = expanded("pub(crate) fn Badge(label: &str) -> Html { html! { <b>x</b> } }");
    assert!(out.contains("pub (crate) struct Badge"), "{out}");
}

#[test]
fn a_component_with_no_props_still_gets_a_companion() {
    let out = expanded("pub fn Logo() -> Html { html! { <b>x</b> } }");
    assert!(out.contains("pub struct Logo"), "{out}");
    assert!(out.contains("__rahti_call"), "{out}");
}

#[test]
fn a_self_parameter_is_refused() {
    let msg = error("pub fn Badge(&self) -> Html { html! { <b>x</b> } }");
    assert!(msg.contains("free function"), "{msg}");
}

// ------------------------------------------------------------------ errors

#[test]
fn the_attribute_takes_no_arguments() {
    let msg = error_with("\"name\"", "fn Badge() -> Html { html! { <b>x</b> } }");
    assert!(msg.contains("takes no arguments"), "{msg}");
    assert!(msg.contains("pp-component"), "{msg}");
}

#[test]
fn it_goes_on_a_function() {
    let msg = error("struct Badge;");
    assert!(msg.contains("goes on a function"), "{msg}");
}

#[test]
fn a_component_is_pascal_case() {
    let msg = error("fn user_card() -> Html { html! { <b>x</b> } }");
    assert!(msg.contains("PascalCase"), "{msg}");
    assert!(msg.contains("`UserCard`"), "{msg}");
}

#[test]
fn a_component_returns_html() {
    let msg = error("fn Badge() { html! { <b>x</b> }; }");
    assert!(msg.contains("returns nothing"), "{msg}");
    assert!(msg.contains("-> Html"), "{msg}");
}

/// Building the markup into a `let` and returning that hides it from the
/// attribute, which then has nothing to name.
#[test]
fn a_component_has_to_end_in_its_markup() {
    let msg = error("fn Badge() -> Html { let out = html! { <b>x</b> }; out }");
    assert!(
        msg.contains("has to end with the markup it renders"),
        "{msg}"
    );
}

#[test]
fn a_component_that_renders_nothing_is_rejected() {
    let msg = error("fn Badge() -> Html { todo!() }");
    assert!(
        msg.contains("has to end with the markup it renders"),
        "{msg}"
    );
}

/// The error is reported alongside the function, so everything that calls it
/// still resolves and the one real problem is the one rustc prints.
#[test]
fn a_failure_is_reported_as_an_item() {
    let err = Error {
        span: proc_macro2::Span::call_site(),
        msg: "boom".into(),
    };
    let out = to_compile_error_item(&err).to_string();
    assert!(out.contains("compile_error !"), "{out}");
    assert!(
        out.trim_end().ends_with(';'),
        "an item, not an expression: {out}"
    );
}

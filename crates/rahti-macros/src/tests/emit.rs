//! Stamping a block with its name, and folding it into writes.

use super::*;
use proc_macro2::TokenStream;
use std::str::FromStr;

/// One attribute of a runtime-built open tag, as it was written.
fn show_tag_attr(attr: &parse::TagAttr) -> String {
    match attr {
        parse::TagAttr::Literal { name, value } => match value {
            Some(value) => format!(" {name}=\"{value}\""),
            None => format!(" {name}"),
        },
        parse::TagAttr::Value { name, .. } => format!(" {name}=\"@{{}}\""),
        parse::TagAttr::Spread(expr) => format!(" @{{..{expr}}}"),
    }
}

fn ops_of(src: &str, file: &str) -> Vec<Op> {
    let stream = TokenStream::from_str(src).expect("test source does not tokenize");
    let parsed = parse::parse(stream).unwrap_or_else(|e| panic!("{}", e.msg));
    stamp(parsed, file)
}

fn markup(src: &str, file: &str) -> String {
    fn join(ops: &[Op]) -> String {
        ops.iter()
            .map(|op| match op {
                Op::Static(s) => s.clone(),
                Op::Text(_) | Op::ScriptText(_) => "@{}".to_string(),
                Op::Attr { name, .. } => format!("{name}=\"@{{}}\""),
                Op::OpenTag(attrs) => attrs.iter().map(show_tag_attr).collect(),
                Op::Slot => "<slot/>".to_string(),
                Op::Component(call) => match &call.children {
                    None => format!("<{}/>", call.name),
                    Some(child) => {
                        format!("<{}>{}</{}>", call.name, join(&child.ops), call.name)
                    }
                },
            })
            .collect()
    }
    join(&ops_of(src, file))
}

/// A convention file under `app/` is named after its path.
#[test]
fn a_page_is_stamped_with_its_name() {
    let out = markup("<main class=\"a\">x</main>", "src/app/users/page.rs");
    let expected =
        component::identifier("src/app/users/page.rs").expect("page.rs is a convention file");
    assert_eq!(
        out,
        format!("<main pp-component=\"{expected}\" class=\"a\">x</main>")
    );
}

/// The name goes on the root element and nowhere else — the client runtime
/// tells blocks apart by it.
#[test]
fn only_the_root_element_is_stamped() {
    let out = markup("<main><article>x</article></main>", "src/app/page.rs");
    assert_eq!(out.matches("pp-component").count(), 1);
    assert!(out.starts_with("<main pp-component="), "{out}");
}

#[test]
fn a_name_the_author_wrote_wins() {
    let out = markup("<div pp-component=\"mine\">x</div>", "src/app/page.rs");
    assert_eq!(out, "<div pp-component=\"mine\">x</div>");
}

/// `#[component]` knows which function the block was written in, which the
/// file path cannot say.
#[test]
fn a_forced_name_beats_the_path() {
    let out = markup(
        "@component \"user_card_3b91f0ae\" <article>x</article>",
        "src/app/page.rs",
    );
    assert_eq!(
        out,
        "<article pp-component=\"user_card_3b91f0ae\">x</article>"
    );
}

/// A root layout renders the document, and `<html>` is no place for the name:
/// the client runtime would take the whole document — `<head>` included — for
/// a component to re-render. It goes on a `<template>` around the slot, which
/// is where the reactive part of the page actually starts.
#[test]
fn a_document_is_named_around_its_slot() {
    let out = markup(
        "<!DOCTYPE html><html lang=\"en\"><head></head><body><slot /></body></html>",
        "src/app/layout.rs",
    );
    let expected =
        component::identifier("src/app/layout.rs").expect("layout.rs is a convention file");
    assert_eq!(
        out,
        format!(
            "<!DOCTYPE html><html lang=\"en\"><head></head><body>\
             <template pp-component=\"{expected}\"><slot/></template></body></html>"
        )
    );
}

/// The injected marker is the same name a non-document block would carry, so
/// the manifest and the response header describe the markup that ships.
#[test]
fn the_injected_name_is_the_one_derived_from_the_path() {
    let out = markup(
        "<html><body><slot /></body></html>",
        "src/app/(portal)/layout.rs",
    );
    let expected = component::identifier("src/app/(portal)/layout.rs")
        .expect("layout.rs is a convention file");
    assert_eq!(out.matches("pp-component").count(), 1, "{out}");
    assert!(
        out.contains(&format!("<template pp-component=\"{expected}\">")),
        "{out}"
    );
}

/// A layout that wants the boundary somewhere else writes its own marker, and
/// then nothing is injected — the same override every other block has.
#[test]
fn a_document_with_its_own_name_is_left_alone() {
    let out = markup(
        "<html><body><template pp-component=\"mine\"><slot /></template></body></html>",
        "src/app/layout.rs",
    );
    assert_eq!(
        out,
        "<html><body><template pp-component=\"mine\"><slot/></template></body></html>"
    );
}

/// Nothing to own: a document that renders no children gets no boundary.
#[test]
fn a_document_without_a_slot_gets_no_template() {
    let out = markup("<html><body>x</body></html>", "src/app/layout.rs");
    assert_eq!(out, "<html><body>x</body></html>");
}

/// A helper module has no route, so there is nothing to name it after.
#[test]
fn markup_outside_the_app_directory_is_left_alone() {
    let out = markup("<div>x</div>", "src/lib/widgets.rs");
    assert_eq!(out, "<div>x</div>");
}

// -------------------------------------------------------------------- emit

/// Adjacent static markup collapses, so a page with no interpolation is one
/// `push_str` into a string sized up front.
#[test]
fn static_markup_folds_into_one_write() {
    let ops = ops_of("<main><p>hello</p></main>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert_eq!(out.matches("push_str").count(), 1, "{out}");
    assert!(out.contains("with_capacity (25usize)"), "{out}");
    assert!(out.contains("Html :: from_raw"), "{out}");
}

/// Interpolation breaks the run, and each side is written on its own.
#[test]
fn interpolation_splits_the_static_runs() {
    let ops = ops_of("<h1>@{title}</h1>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert_eq!(out.matches("push_str").count(), 2, "{out}");
    assert!(out.contains("render_into (& (title)"), "{out}");
}

/// The name goes into the call with the value, because the value is what
/// decides whether the attribute is written at all.
#[test]
fn an_attribute_expression_writes_the_whole_attribute() {
    let ops = ops_of("<a href=@{url}>x</a>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(
        out.contains("attribute (\" href\" , & (url) , false ,"),
        "{out}"
    );
    assert!(
        !out.contains("href = \\\""),
        "the name is not static: {out}"
    );
}

/// The same call, with the flag that tells `rahti::attribute` this is a name
/// HTML reads by presence.
#[test]
fn a_boolean_attribute_expression_is_marked_as_one() {
    let ops = ops_of("<input disabled=@{off} />", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(
        out.contains("attribute (\" disabled\" , & (off) , true ,"),
        "{out}"
    );
}

/// A tag carrying a spread builds its attributes instead of folding them, so
/// that a later one replaces an earlier one.
#[test]
fn a_spread_builds_the_open_tag() {
    let ops = ops_of("<div class=\"a\" @{..extra}>x</div>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(out.contains("AttrList :: new ()"), "{out}");
    assert!(out.contains("set (\"class\" , \"a\")"), "{out}");
    assert!(out.contains("spread (& (extra))"), "{out}");
    assert!(out.contains("write_into (& mut __rahti)"), "{out}");
}

/// And a tag without one does not: the cost of the list is paid only where
/// the feature is used.
#[test]
fn a_tag_without_a_spread_still_folds_into_static_markup() {
    let ops = ops_of("<div class=\"a\" id=\"b\">x</div>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(!out.contains("AttrList"), "{out}");
    assert_eq!(out.matches("push_str").count(), 1, "{out}");
}

/// `<slot />` is the `children` binding, so a block using one outside a
/// layout fails with rustc's ordinary "cannot find value".
#[test]
fn slot_renders_the_children_binding() {
    let ops = ops_of("<main><slot /></main>", "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(out.contains("render_into (& children"), "{out}");
}

// -------------------------------------------------------------- components

/// A component tag compiles to a struct literal of the named-props companion
/// `#[component]` generates, handed to the function by `__rahti_call`.
#[test]
fn a_component_tag_calls_through_its_companion() {
    let ops = ops_of(
        "<main><Card title=\"Profile\" count=@{n} /></main>",
        "src/lib/widgets.rs",
    );
    let out = emit(ops).to_string();
    assert!(out.contains("Card {"), "{out}");
    assert!(out.contains(". __rahti_call (Card)"), "{out}");
    assert!(out.contains("title : \"Profile\""), "{out}");
    assert!(out.contains("count : (n)"), "{out}");
    assert!(
        !out.contains("children"),
        "self-closing passes no children: {out}"
    );
}

/// Children between the tags become the `children` field, built by this same
/// emitter — a nested `Html` the shim passes on.
#[test]
fn component_children_become_the_children_field() {
    let ops = ops_of(
        "<main><Card title=\"x\"><p>hi</p></Card></main>",
        "src/lib/widgets.rs",
    );
    let out = emit(ops).to_string();
    assert!(out.contains("children : {"), "{out}");
    assert!(out.contains("<p>hi</p>"), "{out}");
}

/// In a convention file, a single-rooted child fragment is named by the file
/// — the same `pp-component` a separate `html! { … }` child block carries, so
/// the two spellings ship byte-identical markup and keep the same reactive
/// scope.
#[test]
fn component_children_are_stamped_like_an_inline_block() {
    let out = markup(
        "<main><Card title=\"x\"><p>hi</p></Card></main>",
        "src/app/users/page.rs",
    );
    let expected =
        component::identifier("src/app/users/page.rs").expect("page.rs is a convention file");
    assert!(
        out.contains(&format!("<p pp-component=\"{expected}\">")),
        "{out}"
    );
    assert!(
        out.starts_with(&format!("<main pp-component=\"{expected}\"")),
        "{out}"
    );
}

/// Outside the app directory there is no file name to stamp with — the same
/// rule an inline `html! { … }` block follows in a component file.
#[test]
fn component_children_outside_the_app_directory_are_left_alone() {
    let out = markup(
        "<main><Card title=\"x\"><p>hi</p></Card></main>",
        "src/components/frame.rs",
    );
    assert!(!out.contains("pp-component"), "{out}");
}

/// Several roots match the `Html::concat` shape, which never carried a name.
#[test]
fn multi_rooted_children_are_not_stamped() {
    let out = markup(
        "<main><Card><b>a</b><i>b</i></Card></main>",
        "src/app/page.rs",
    );
    assert_eq!(
        out.matches("pp-component").count(),
        1,
        "only the root: {out}"
    );
}

/// Nested component tags stamp their fragments at every level.
#[test]
fn nested_component_children_are_stamped_too() {
    let out = markup(
        "<main><Card><div><Panel><p>x</p></Panel></div></Card></main>",
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert_eq!(
        out.matches(&format!("pp-component=\"{expected}\"")).count(),
        3,
        "{out}"
    );
}

// ------------------------------------------------------------- composition

/// A page rooted in a component tag has no element of its own to name, so
/// no page marker is injected: the component's own stamp — carried inside
/// its markup — is the outermost identity the client runtime sees.
#[test]
fn a_composition_root_dissolves_the_page_stamp() {
    let ops = ops_of("<Card title=\"x\" />", "src/app/users/page.rs");
    let out = emit(ops).to_string();
    assert!(!out.contains("pp-component"), "{out}");
    assert!(out.contains(". __rahti_call (Card)"), "{out}");
}

/// …but the children the page writes into that root are still the page's
/// own fragment, named by the file, exactly as under an element root.
#[test]
fn a_composition_root_still_stamps_its_children() {
    let out = markup(
        "<Panel title=\"x\"><p>hi</p></Panel>",
        "src/app/users/page.rs",
    );
    let expected =
        component::identifier("src/app/users/page.rs").expect("page.rs is a convention file");
    assert!(
        out.contains(&format!("<p pp-component=\"{expected}\">")),
        "{out}"
    );
}

/// A `#[component]` may render another component as its whole output — the
/// forced name simply goes unused, since composition owns no element.
#[test]
fn a_forced_name_on_a_composition_root_goes_unused() {
    let out = markup(
        "@component \"framed_41d26bb9\" <Panel title=\"x\"><p>y</p></Panel>",
        "src/components/panel.rs",
    );
    assert!(!out.contains("pp-component"), "{out}");
    assert!(out.contains("<Panel>"), "{out}");
}

// --------------------------------------------------------------- fragments

/// A fragment-rooted page is named in its open marker, exactly where an
/// element root would carry the name as an attribute.
#[test]
fn a_fragment_root_takes_the_page_stamp() {
    let out = markup("<><h1>a</h1><p>b</p></>", "src/app/users/page.rs");
    let expected =
        component::identifier("src/app/users/page.rs").expect("page.rs is a convention file");
    assert_eq!(
        out,
        format!("<!--pp:{expected}--><h1>a</h1><p>b</p><!--/pp-->")
    );
}

/// A `#[component]` rooted in a fragment keeps its own name and scope — the
/// marker pair is where the runtime anchors its script and re-renders.
#[test]
fn a_fragment_root_takes_a_forced_name() {
    let out = markup(
        "@component \"tally_12345678\" <><button>x</button><p>y</p></>",
        "src/components/tally.rs",
    );
    assert!(out.starts_with("<!--pp:tally_12345678-->"), "{out}");
}

/// Fragment-rooted children of a component tag are their own named block.
#[test]
fn fragment_rooted_children_are_stamped() {
    let out = markup(
        "<main><Card><><p>a</p><p>b</p></></Card></main>",
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(out.contains(&format!("<!--pp:{expected}-->")), "{out}");
}

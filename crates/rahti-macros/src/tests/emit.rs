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
        parse::TagAttr::OwnsEvents => " @owns-events".to_string(),
    }
}

/// The tokens `parse` reads for `html! { {src} }`, optionally behind the
/// `@component "id"` that `#[component]` writes. The file only matters to
/// [`stamp`], which names the block after it.
fn template(src: &str, _file: &str, component: Option<&str>) -> Result<TokenStream, String> {
    let directive = component.map_or(String::new(), |id| format!("@component \"{id}\" "));
    TokenStream::from_str(&format!("{directive}{src}")).map_err(|e| e.to_string())
}

/// A component's template, parsed, or the message it is refused with.
fn component_block(id: &str, src: &str) -> Result<Parsed, String> {
    let tokens = template(src, "src/components/widget.rs", Some(id))?;
    parse::parse(tokens).map_err(|e| e.msg)
}

/// `src` is a template, optionally led by `@component <id>` standing for the
/// directive `#[component]` writes in front of it.
fn ops_of(src: &str, file: &str) -> Vec<Op> {
    let (component, src) = match src.strip_prefix("@component ") {
        Some(rest) => {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            (Some(&rest[..end]), &rest[end..])
        }
        None => (None, src),
    };
    let tokens = template(src, file, component).unwrap_or_else(|msg| panic!("{msg}"));
    let parsed = parse::parse(tokens).unwrap_or_else(|e| panic!("{}", e.msg));
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
                Op::Slot | Op::DeferredSlot => "<slot/>".to_string(),
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
    let out = markup(r#"<main class="a">"x"</main>"#, "src/app/users/page.rs");
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
    let out = markup(r#"<main><article>"x"</article></main>"#, "src/app/page.rs");
    assert_eq!(out.matches("pp-component").count(), 1);
    assert!(out.starts_with("<main pp-component="), "{out}");
}

#[test]
fn a_name_the_author_wrote_wins() {
    let out = markup(r#"<div pp-component="mine">"x"</div>"#, "src/app/page.rs");
    assert_eq!(out, "<div pp-component=\"mine\">x</div>");
}

/// `#[component]` knows which function the block was written in, which the
/// file path cannot say.
#[test]
fn a_forced_name_beats_the_path() {
    let out = markup(
        r#"@component user_card_3b91f0ae <article>"x"</article>"#,
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
        r#"<!DOCTYPE html><html lang="en"><head></head><body><slot /></body></html>"#,
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
        r#"<html><body><slot /></body></html>"#,
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
        r#"<html><body><template pp-component="mine"><slot /></template></body></html>"#,
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
    let out = markup(r#"<html><body>"x"</body></html>"#, "src/app/layout.rs");
    assert_eq!(out, "<html><body>x</body></html>");
}

/// A helper module has no route, so there is nothing to name it after.
#[test]
fn markup_outside_the_app_directory_is_left_alone() {
    let out = markup(r#"<div>"x"</div>"#, "src/lib/widgets.rs");
    assert_eq!(out, "<div>x</div>");
}

// -------------------------------------------------------------------- emit

/// Adjacent static markup collapses, so a page with no interpolation is one
/// `push_str` into a string sized up front.
#[test]
fn static_markup_folds_into_one_write() {
    let ops = ops_of(r#"<main><p>"hello"</p></main>"#, "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert_eq!(out.matches("push_str").count(), 1, "{out}");
    assert!(out.contains("with_capacity (25usize)"), "{out}");
    assert!(out.contains("Html :: from_raw"), "{out}");
}

/// Interpolation breaks the run, and each side is written on its own.
#[test]
fn interpolation_splits_the_static_runs() {
    let ops = ops_of(r#"<h1>@{title}</h1>"#, "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert_eq!(out.matches("push_str").count(), 2, "{out}");
    assert!(out.contains("render_into (& (title)"), "{out}");
}

/// The name goes into the call with the value, because the value is what
/// decides whether the attribute is written at all.
#[test]
fn an_attribute_expression_writes_the_whole_attribute() {
    let ops = ops_of(r#"<a href=@{url}>"x"</a>"#, "src/lib/widgets.rs");
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
    let ops = ops_of(r#"<input disabled=@{off} />"#, "src/lib/widgets.rs");
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
    let ops = ops_of(
        r#"<div class="a" @{..extra}>"x"</div>"#,
        "src/lib/widgets.rs",
    );
    let out = emit(ops).to_string();
    assert!(out.contains("AttrList :: new ()"), "{out}");
    assert!(out.contains("set_source (\"class\" , \"a\")"), "{out}");
    assert!(out.contains("spread (& (extra))"), "{out}");
    assert!(out.contains("write_into (& mut __rahti)"), "{out}");
}

/// And a tag without one does not: the cost of the list is paid only where
/// the feature is used.
#[test]
fn a_tag_without_a_spread_still_folds_into_static_markup() {
    let ops = ops_of(r#"<div class="a" id="b">"x"</div>"#, "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(!out.contains("AttrList"), "{out}");
    assert_eq!(out.matches("push_str").count(), 1, "{out}");
}

/// `<slot />` is the `children` binding, so a block using one outside a
/// layout fails with rustc's ordinary "cannot find value".
#[test]
fn slot_renders_the_children_binding() {
    let ops = ops_of(r#"<main><slot /></main>"#, "src/lib/widgets.rs");
    let out = emit(ops).to_string();
    assert!(out.contains("render_into (& children"), "{out}");
}

// -------------------------------------------------------------- components

/// A component tag compiles to a struct literal of the named-props companion
/// `#[component]` generates, handed to the function by `__rahti_call`.
#[test]
fn a_component_tag_calls_through_its_companion() {
    let ops = ops_of(
        r#"<main><Card title="Profile" count=@{n} /></main>"#,
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
        r#"<main><Card title="x"><p>"hi"</p></Card></main>"#,
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
        r#"<main><Card title="x"><p>"hi"</p></Card></main>"#,
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
        r#"<main><Card title="x"><p>"hi"</p></Card></main>"#,
        "src/components/frame.rs",
    );
    assert!(!out.contains("pp-component"), "{out}");
}

/// Several roots match the `Html::concat` shape, which never carried a name.
#[test]
fn multi_rooted_children_are_not_stamped() {
    let out = markup(
        r#"<main><Card><b>"a"</b><i>"b"</i></Card></main>"#,
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
        r#"<main><Card><div><Panel><p>"x"</p></Panel></div></Card></main>"#,
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
    let ops = ops_of(r#"<Card title="x" />"#, "src/app/users/page.rs");
    let out = emit(ops).to_string();
    assert!(!out.contains("pp-component"), "{out}");
    assert!(out.contains(". __rahti_call (Card)"), "{out}");
}

/// …but the children the page writes into that root are still the page's
/// own fragment, named by the file, exactly as under an element root.
#[test]
fn a_composition_root_still_stamps_its_children() {
    let out = markup(
        r#"<Panel title="x"><p>"hi"</p></Panel>"#,
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
        r#"@component framed_41d26bb9 <Panel title="x"><p>"y"</p></Panel>"#,
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
    let out = markup(r#"<><h1>"a"</h1><p>"b"</p></>"#, "src/app/users/page.rs");
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
        r#"@component tally_12345678 <><button>"x"</button><p>"y"</p></>"#,
        "src/components/tally.rs",
    );
    assert!(out.starts_with("<!--pp:tally_12345678-->"), "{out}");
}

/// Fragment-rooted children of a component tag are their own named block.
#[test]
fn fragment_rooted_children_are_stamped() {
    let out = markup(
        r#"<main><Card><><p>"a"</p><p>"b"</p></></Card></main>"#,
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(out.contains(&format!("<!--pp:{expected}-->")), "{out}");
}

// --------------------------------------------------------- owned children

/// Children that read the page's state ship as an owned template: the client
/// runtime renders `<template pp-owner="…">` content in the owner's scope
/// and re-renders it when that owner's state changes — React's children
/// semantics. Inlined into the component's own boundary instead, a bare
/// `{count}` child would render once and never update.
#[test]
fn binding_children_are_wrapped_for_their_owner() {
    let out = markup(r#"<main><Badge>{count}</Badge></main>"#, "src/app/page.rs");
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(
        out.contains(&format!(
            "<template pp-owner=\"{expected}\">{{count}}</template>"
        )),
        "{out}"
    );
}

/// A single-rooted child with a binding is the owner's content, not a block
/// of its own — the owner wrap replaces the `pp-component` stamp.
#[test]
fn a_single_rooted_binding_child_is_owned_not_stamped() {
    let out = markup(
        r#"<main><Card><p>{seen}</p></Card></main>"#,
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(
        out.contains(&format!(
            "<template pp-owner=\"{expected}\"><p>{{seen}}</p></template>"
        )),
        "{out}"
    );
    // Only the page root carries a stamp; the child fragment does not.
    assert_eq!(out.matches("pp-component").count(), 1, "{out}");
}

/// An element attribute binding counts the same as a text binding.
#[test]
fn an_attribute_binding_child_is_owned() {
    let out = markup(
        r#"<main><Card><p hidden={done}>"x"</p></Card></main>"#,
        "src/app/page.rs",
    );
    assert!(out.contains("<template pp-owner="), "{out}");
}

/// So does a `{…}` prop on a component tag inside the children: the inner
/// component writes it on its root, which is evaluated in the owner's scope.
#[test]
fn a_component_prop_binding_child_is_owned() {
    let out = markup(
        r#"<main><Card><Input value={query} /></Card></main>"#,
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(
        out.contains(&format!("<template pp-owner=\"{expected}\">")),
        "{out}"
    );
}

/// Children with their own `<script>` are self-contained blocks — their
/// bindings read state they declare themselves, so they keep their own
/// stamp and take no owner wrap.
#[test]
fn scripted_children_stay_their_own_block() {
    let out = markup(
        r###"<main><Card><div><p>{n}</p><script>r##"const [n, setN] = pp.state(0);"##</script></div></Card></main>"###,
        "src/app/page.rs",
    );
    let expected = component::identifier("src/app/page.rs").expect("a convention file");
    assert!(!out.contains("pp-owner"), "{out}");
    assert!(
        out.contains(&format!("<div pp-component=\"{expected}\">")),
        "{out}"
    );
}

/// Static children are served inline, visible without JavaScript — no
/// template hides them, and they are named the way any block is.
#[test]
fn static_children_take_no_owner_wrap() {
    let out = markup(
        r#"<main><Card><p>"hi"</p></Card></main>"#,
        "src/app/page.rs",
    );
    assert!(!out.contains("pp-owner"), "{out}");
}

/// In a `#[component]` function, the children's owner is that function's
/// block — the forced name — not the file the function happens to live in.
#[test]
fn a_forced_name_owns_the_children_written_in_its_block() {
    let out = markup(
        r#"@component shell_12345678 <div><Card>{open}</Card></div>"#,
        "src/components/shell.rs",
    );
    assert!(
        out.contains("<template pp-owner=\"shell_12345678\">{open}</template>"),
        "{out}"
    );
}

/// A binding inside a deeper component tag belongs to that deeper child
/// block — the outer children stay inline, and only the inner block is
/// wrapped, so the wrap sits as close to the content as it can.
#[test]
fn only_the_block_that_holds_the_binding_is_wrapped() {
    let out = markup(
        r#"<main><Card><div><Panel><p>{x}</p></Panel></div></Card></main>"#,
        "src/app/page.rs",
    );
    assert_eq!(out.matches("pp-owner").count(), 1, "{out}");
    assert!(
        out.contains("<template pp-owner=") && out.contains("<p>{x}</p></template>"),
        "{out}"
    );
}

// ------------------------------------------------------ root event ownership

/// A handler a block writes on its own root belongs to that block. The client
/// runtime evaluates a root's attributes in the caller's scope — where
/// forwarded props come from — so the root names its block as event owner,
/// or `<form onsubmit={save(event)}>` throws before `preventDefault()` runs.
#[test]
fn a_root_that_handles_its_own_event_names_its_owner() {
    let out = markup(
        r#"@component form_12345678 <form onsubmit={save(event)}><button>"go"</button></form>"#,
        "src/components/form.rs",
    );
    assert_eq!(
        out,
        "<form pp-component=\"form_12345678\" pp-event-owner=\"form_12345678\" \
         onsubmit=\"{save(event)}\"><button>go</button></form>"
    );
}

/// A page root is a block root like any other.
#[test]
fn a_page_root_owns_its_own_handlers() {
    let out = markup(r#"<main onclick={count()}>"x"</main>"#, "src/app/page.rs");
    let expected = component::identifier("src/app/page.rs").expect("page.rs is a convention file");
    assert!(
        out.starts_with(&format!(
            "<main pp-component=\"{expected}\" pp-event-owner=\"{expected}\""
        )),
        "{out}"
    );
}

/// Only the root: an inner element's handler already runs in the block that
/// holds it, and a callback prop (`on-select`) is not a native event.
#[test]
fn inner_handlers_and_callback_props_leave_the_root_alone() {
    let inner = markup(
        r#"<main><button onclick={go()}>"x"</button></main>"#,
        "src/app/page.rs",
    );
    assert!(!inner.contains("pp-event-owner"), "{inner}");

    let prop = markup(r#"<main on-select={pick}>"x"</main>"#, "src/app/page.rs");
    assert!(!prop.contains("pp-event-owner"), "{prop}");
}

/// Any lowercase, unhyphenated `on…` counts — there is no list of events to
/// keep current. The runtime decides what is native; for anything else the
/// marker is inert.
#[test]
fn events_are_recognized_by_naming_not_a_list() {
    let out = markup(
        r#"@component x_12345678 <div onsomefutureevent={react(event)}>"x"</div>"#,
        "src/components/x.rs",
    );
    assert!(out.contains("pp-event-owner=\"x_12345678\""), "{out}");
}

/// A self-contained child block keeps its own identity, and its root's own
/// handler is its own too.
#[test]
fn a_self_contained_child_block_owns_its_root_handler() {
    let out = markup(
        r###"<main><Card><div onclick={hit()}><script>r##"function hit() {}"##</script></div></Card></main>"###,
        "src/app/page.rs",
    );
    let owners = out.matches("pp-event-owner=").count();
    assert_eq!(owners, 1, "{out}");
    assert!(out.contains("<div pp-component="), "{out}");
}

/// A spread beside an owned handler is marked, so the tag built at runtime
/// refuses a caller's handler the spread would bring.
#[test]
fn a_spread_beside_an_owned_handler_is_marked() {
    let out = markup(
        r#"@component f_12345678 <form onsubmit={save(event)} @{..attrs}>"x"</form>"#,
        "src/components/f.rs",
    );
    assert!(out.contains("pp-event-owner=\"f_12345678\""), "{out}");
    assert!(out.contains(" @owns-events"), "{out}");

    let plain = markup(r#"<div @{..attrs}>"x"</div>"#, "src/app/page.rs");
    assert!(!plain.contains("@owns-events"), "{plain}");
    assert!(!plain.contains("pp-event-owner"), "{plain}");
}

/// A script-less component whose own markup binds a client value gets an
/// empty `<script>` before its root closes: the runtime compiles a boundary
/// without one in the caller's scope, and `{pp.props.label}` must read this
/// component's props.
#[test]
fn a_scriptless_component_with_its_own_bindings_claims_its_markup() {
    let out = markup(
        r#"@component label_12345678 <p class="label"><b>{pp.props.label}</b></p>"#,
        "src/components/label.rs",
    );
    assert!(
        out.ends_with("<b>{pp.props.label}</b><script></script></p>"),
        "{out}"
    );

    let fragment = markup(
        r#"@component pair_12345678 <><dt>{pp.props.term}</dt><dd>"x"</dd></>"#,
        "src/components/pair.rs",
    );
    assert!(
        fragment.contains("<dd>x</dd><script></script><!--/pp-->"),
        "{fragment}"
    );
}

/// Markup that is not the component's own keeps the runtime's rule.
#[test]
fn a_component_without_bindings_of_its_own_is_left_alone() {
    let cases = [
        // No client binding at all.
        r#"@component a_12345678 <p>"static"</p>"#,
        // A script of its own already.
        r###"@component b_12345678 <p>{n}<script>r##"const n = 1;"##</script></p>"###,
        // Children between a component tag are a block of their own.
        "@component c_12345678<div><Card><b>{count}</b></Card></div>",
        // A spread below the root may carry the caller's `bindings`.
        "@component d_12345678<label><input @{..bindings} /><span>{pp.props.hint}</span></label>",
        // Only the root's own attributes, which are the caller's.
        r#"@component e_12345678 <button onclick={pp.props.onPick()}>"go"</button>"#,
    ];
    for src in cases {
        let out = markup(src, "src/components/x.rs");
        assert!(!out.contains("<script></script>"), "{src} -> {out}");
    }
}

/// A page is not a component and is not claimed.
#[test]
fn a_page_is_not_claimed() {
    let out = markup(r#"<main><p>{count}</p></main>"#, "src/app/page.rs");
    assert!(!out.contains("<script></script>"), "{out}");
}

/// A client binding a component writes on its own root is evaluated in the
/// caller's scope, so it can never read the component's own state: refused.
#[test]
fn a_component_binding_on_its_own_root_is_refused() {
    let Err(msg) = component_block(
        "dialog_12345678",
        r###"<section hidden={!open}><script>r##"const { open } = pp.props;"##</script></section>"###,
    ) else {
        panic!("a root binding in a component must not compile");
    };
    assert!(msg.contains("`hidden={!open}`"), "{msg}");
    assert!(msg.contains("caller's scope"), "{msg}");
}

/// What is allowed on a component root: handlers, JSON data, runtime-managed
/// attributes, server values — and any binding on an inner element.
#[test]
fn allowed_root_attributes_still_compile() {
    let cases = [
        r#"<form onsubmit={save(event)}>"x"</form>"#,
        r#"<div data-config="{\"a\": 1}">"x"</div>"#,
        r#"<div pp-ref={root}>"x"</div>"#,
        r#"<div class=@{cls}>"x"</div>"#,
        r###"<section><div hidden={!open}>"x"</div><script>r##"const { open } = pp.props;"##</script></section>"###,
    ];
    for src in cases {
        assert!(component_block("a_12345678", src).is_ok(), "{src}");
    }
}

/// A page's root is not a component's and is not checked.
#[test]
fn a_page_root_binding_compiles() {
    let tokens = template(r#"<main class={theme}>"x"</main>"#, "src/app/page.rs", None).unwrap();
    assert!(parse::parse(tokens).is_ok());
}

/// Children handed on (`<Panel>@{children}</Panel>`) are opaque `Html`
/// already wrapped for their owner at the caller: the passing block adds no
/// owner wrap of its own.
#[test]
fn children_handed_on_are_not_wrapped_again() {
    let out = markup(
        r#"@component framed_12345678 <div><Panel title="t">@{children}</Panel></div>"#,
        "src/components/panel.rs",
    );
    assert!(!out.contains("pp-owner"), "{out}");
}

/// A document's slot lands in a deferred template, so its children are
/// written through `rahti::render_deferred_children` — injected marker or
/// the layout's own.
#[test]
fn a_document_slot_raises_its_childrens_brace_references() {
    for src in [
        "<html><body><slot /></body></html>",
        "<html><body><template pp-component=\"mine\"><slot /></template></body></html>",
    ] {
        let ops = ops_of(src, "src/app/layout.rs");
        assert!(ops.iter().any(|op| matches!(op, Op::DeferredSlot)), "{src}");
        assert!(!ops.iter().any(|op| matches!(op, Op::Slot)), "{src}");
    }
    let ops = ops_of(r#"<main><slot /></main>"#, "src/app/dashboard/layout.rs");
    assert!(ops.iter().any(|op| matches!(op, Op::Slot)));
}

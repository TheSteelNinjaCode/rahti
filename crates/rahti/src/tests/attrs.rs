//! Whether an attribute is written, and what a spread does to a tag.

use super::*;

use crate::Html;

/// The attribute a value produces, name and all.
fn write(name: &str, value: &impl Render, boolean: bool) -> String {
    let mut out = String::new();
    attribute(&format!(" {name}"), value, boolean, &mut out);
    out
}

/// The attributes a list produces.
fn list(build: impl FnOnce(&mut AttrList)) -> String {
    let mut list = AttrList::new();
    build(&mut list);
    let mut out = String::new();
    list.write_into(&mut out);
    out
}

// ------------------------------------------------------------------ presence

#[test]
fn an_ordinary_value_is_written() {
    assert_eq!(write("href", &"/users", false), " href=\"/users\"");
    assert_eq!(write("tabindex", &0, false), " tabindex=\"0\"");
    assert_eq!(
        write("class", &String::from("card"), false),
        " class=\"card\""
    );
}

/// `href=""` is a link to the current page, which is not what "no href"
/// means. An absent optional writes nothing at all.
#[test]
fn an_absent_optional_writes_no_attribute() {
    let missing: Option<&str> = None;
    assert_eq!(write("href", &missing, false), "");
}

#[test]
fn a_present_optional_writes_what_is_inside_it() {
    let present: Option<&str> = Some("/users");
    assert_eq!(write("href", &present, false), " href=\"/users\"");
}

/// HTML reads a boolean attribute by whether it is there, so `disabled="false"`
/// disables the control. A `false` on one of these has to disappear.
#[test]
fn a_boolean_attribute_follows_the_flag() {
    assert_eq!(write("disabled", &true, true), " disabled=\"\"");
    assert_eq!(write("disabled", &false, true), "");
}

/// The same `false` on a name HTML does not read that way is a value, and
/// `aria-hidden="false"` says something no absent attribute says.
#[test]
fn a_flag_on_any_other_name_is_an_ordinary_value() {
    assert_eq!(
        write("aria-hidden", &false, false),
        " aria-hidden=\"false\""
    );
    assert_eq!(write("aria-hidden", &true, false), " aria-hidden=\"true\"");
}

/// The two rules compose, so `Option<bool>` behaves like the `bool` in it.
#[test]
fn an_optional_flag_answers_for_what_is_inside_it() {
    assert_eq!(write("disabled", &Some(true), true), " disabled=\"\"");
    assert_eq!(write("disabled", &Some(false), true), "");
    assert_eq!(write("disabled", &None::<bool>, true), "");
}

/// Everything else keeps the answer it always gave, including a type an
/// application implements `Render` for itself.
#[test]
fn anything_else_is_present_by_default() {
    struct Currency(u32);
    impl Render for Currency {
        fn render_into(&self, out: &mut String) {
            out.push_str(&format!("{}.00", self.0));
        }
    }

    assert_eq!(
        write("data-total", &Currency(12), false),
        " data-total=\"12.00\""
    );
    assert_eq!(
        write("data-total", &Currency(12), true),
        " data-total=\"12.00\""
    );
}

#[test]
fn a_value_is_escaped_where_it_is_written() {
    assert_eq!(
        write("title", &"a \" b & c < d", false),
        " title=\"a &quot; b &amp; c &lt; d\""
    );
}

/// Markup that is already trusted passes through, the same as it does in text
/// position.
#[test]
fn trusted_markup_is_not_escaped_again() {
    let value = Html::from_raw("a &amp; b".to_string());
    assert_eq!(write("data-x", &value, false), " data-x=\"a &amp; b\"");
}

// ---------------------------------------------------------------- the list

#[test]
fn the_list_writes_what_it_was_given_in_order() {
    let out = list(|list| {
        list.set("class", "card");
        list.set("id", "a");
    });

    assert_eq!(out, " class=\"card\" id=\"a\"");
}

/// The whole point of building the tag: writing a name twice replaces it,
/// rather than emitting a duplicate for the browser to take the first of.
#[test]
fn a_later_write_replaces_an_earlier_one_in_place() {
    let out = list(|list| {
        list.set("class", "card");
        list.set("id", "a");
        list.set("class", "panel");
    });

    assert_eq!(out, " class=\"panel\" id=\"a\"");
}

#[test]
fn a_value_that_writes_nothing_removes_what_came_before() {
    let out = list(|list| {
        list.set("hidden", "");
        list.value("hidden", &false, true);
    });

    assert_eq!(out, "");
}

#[test]
fn the_list_escapes_what_it_is_given() {
    let out = list(|list| list.set("title", "a \" b"));

    assert_eq!(out, " title=\"a &quot; b\"");
}

/// A value rendered through `Render` escapes itself, and must not be escaped
/// a second time on the way into the list.
#[test]
fn a_rendered_value_is_escaped_exactly_once() {
    let out = list(|list| list.value("title", &"a & b", false));

    assert_eq!(out, " title=\"a &amp; b\"");
}

// -------------------------------------------------------------------- spread

#[test]
fn a_spread_folds_its_attributes_in_where_it_was_written() {
    let out = list(|list| {
        list.set("class", "card");
        list.spread(&Attrs::new().set("id", "a").set("class", "panel"));
    });

    assert_eq!(out, " class=\"panel\" id=\"a\"");
}

/// Written before the attributes, a spread is a set of defaults; after them,
/// it overrides. Same rule, read in the order it is written — which is what
/// `{...props}` does in JSX.
#[test]
fn where_a_spread_sits_decides_whether_it_wins() {
    let defaults = list(|list| {
        list.spread(&Attrs::new().set("class", "spread"));
        list.set("class", "written");
    });
    assert_eq!(defaults, " class=\"written\"");

    let overrides = list(|list| {
        list.set("class", "written");
        list.spread(&Attrs::new().set("class", "spread"));
    });
    assert_eq!(overrides, " class=\"spread\"");
}

/// The one thing no attribute value can express: taking an attribute away.
#[test]
fn a_spread_can_remove_an_attribute() {
    let out = list(|list| {
        list.set("class", "card");
        list.set("stroke-width", "2");
        list.spread(&Attrs::new().unset("stroke-width"));
    });

    assert_eq!(out, " class=\"card\"");
}

#[test]
fn a_spread_reads_pairs_from_the_usual_shapes() {
    assert_eq!(
        list(|list| list.spread(&[("a", "1"), ("b", "2")])),
        " a=\"1\" b=\"2\""
    );
    assert_eq!(
        list(|list| list.spread(&vec![("a".to_string(), "1".to_string())])),
        " a=\"1\""
    );
    assert_eq!(
        list(|list| list.spread(&[("a", "1")][..].to_vec())),
        " a=\"1\""
    );
}

/// A spread that is sometimes nothing is an `Option`, rather than a branch
/// around the whole tag.
#[test]
fn an_optional_spread_writes_nothing_when_absent() {
    let nothing: Option<Attrs> = None;
    assert_eq!(list(|list| list.spread(&nothing)), "");

    let something = Some(Attrs::new().set("id", "a"));
    assert_eq!(list(|list| list.spread(&something)), " id=\"a\"");
}

/// A map has no order of its own, and markup that changes between two runs of
/// the same code is markup nobody can review.
#[test]
fn a_hash_map_spreads_in_a_stable_order() {
    let map: std::collections::HashMap<&str, &str> =
        [("z", "1"), ("a", "2"), ("m", "3")].into_iter().collect();

    assert_eq!(list(|list| list.spread(&map)), " a=\"2\" m=\"3\" z=\"1\"");
}

// ------------------------------------------------------------------ refusals

#[test]
fn an_ordinary_attribute_name_is_accepted() {
    for name in [
        "id",
        "class",
        "stroke-width",
        "aria-label",
        "data-user-id",
        "xlink:href",
        "xml:space",
        "_private",
        "data-x.y",
    ] {
        assert!(is_valid_name(name), "{name} should be a legal name");
    }
}

#[test]
fn a_name_that_could_escape_the_tag_is_refused() {
    for name in [
        "", " id", "id class", "id=\"x\"", "id>", "id/", "1st", "-dash",
    ] {
        assert!(!is_valid_name(name), "{name:?} should not be a legal name");
    }
}

/// A name is written verbatim, so one markup cannot carry is dropped.
/// Development says so; production serves the page without it.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "not an attribute name")]
fn an_illegal_name_is_reported_in_development() {
    let _ = list(|list| list.set("id=\"x\" onload", "boom()"));
}

#[test]
#[cfg(not(debug_assertions))]
fn an_illegal_name_is_dropped_in_release() {
    assert_eq!(list(|list| list.set("id=\"x\" onload", "boom()")), "");
}

/// The client runtime binds this name, and `html!` has already written the
/// block's own. A spread does not get to rename a component.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "pp-component")]
fn a_spread_cannot_write_the_component_name() {
    let _ = list(|list| list.spread(&Attrs::new().set("pp-component", "elsewhere")));
}

/// Written on the tag it is honoured, which is the documented way to pin a
/// block's name — and the refusal above is about where the name came from,
/// not about the name.
#[test]
fn the_component_name_written_on_the_tag_is_kept() {
    assert_eq!(
        list(|list| list.set("pp-component", "pinned")),
        " pp-component=\"pinned\""
    );
}

/// The flag comes back down after a spread, so an attribute written after one
/// is not still treated as part of it.
#[test]
fn the_tag_is_the_tag_again_once_a_spread_is_folded_in() {
    assert_eq!(
        list(|list| {
            list.spread(&Attrs::new().set("id", "a"));
            list.set("pp-component", "pinned");
        }),
        " id=\"a\" pp-component=\"pinned\""
    );
}

// -------------------------------------------------------------------- attrs

#[test]
fn set_replaces_and_unset_removes() {
    let attrs = Attrs::new().set("id", "a").set("id", "b").unset("class");

    assert_eq!(list(|list| list.set("class", "card")), " class=\"card\"");
    assert_eq!(
        list(|list| {
            list.set("class", "card");
            list.spread(&attrs);
        }),
        " id=\"b\""
    );
}

#[test]
fn a_set_can_be_built_from_an_array_or_an_iterator() {
    let from_array = Attrs::from([("id", "avatar"), ("data-user", "42")]);
    assert_eq!(
        list(|list| list.spread(&from_array)),
        " id=\"avatar\" data-user=\"42\""
    );

    let collected: Attrs = (1..=2)
        .map(|n| (format!("data-{n}"), n.to_string()))
        .collect();
    assert_eq!(
        list(|list| list.spread(&collected)),
        " data-1=\"1\" data-2=\"2\""
    );
}

/// A set is usually built and spread, but a component that merges a caller's
/// attributes into its own has to read what the caller named first.
#[test]
fn a_set_can_be_read_back() {
    let attrs = Attrs::new()
        .set("id", "a")
        .unset("class")
        .set("role", "img");

    assert_eq!(attrs.names().collect::<Vec<_>>(), ["id", "class", "role"]);
    assert_eq!(
        attrs.iter().collect::<Vec<_>>(),
        [("id", Some("a")), ("class", None), ("role", Some("img"))]
    );
    assert_eq!((&attrs).into_iter().count(), 3);
}

/// Three answers, not two: not named at all, named as a removal, or named
/// with a value.
#[test]
fn get_tells_a_removal_apart_from_an_absence() {
    let attrs = Attrs::new().set("id", "a").unset("class");

    assert_eq!(attrs.get("id"), Some(Some("a")));
    assert_eq!(attrs.get("class"), Some(None));
    assert_eq!(attrs.get("title"), None);
}

#[test]
fn an_empty_set_is_empty() {
    let attrs = Attrs::new();

    assert!(attrs.is_empty());
    assert_eq!(attrs.len(), 0);
    assert_eq!(attrs, Attrs::default());
    assert_eq!(list(|list| list.spread(&attrs)), "");
}

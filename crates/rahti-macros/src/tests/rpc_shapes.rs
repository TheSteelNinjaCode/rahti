//! Reads of an rpc's result, walked through the shape the build wrote.
//!
//! Each refusal is paired with the ordinary reads it must not catch: this
//! check stops a build, so it speaks only when the field is certainly absent.

use super::*;

const FILE: &str = "src/app/contacts/page.rs";

/// The shapes file a build writes for a page with these rpcs.
fn shapes() -> Shapes {
    let text = "\
R\tsrc/app/contacts/page.rs\tsave\tSaved
R\tsrc/app/contacts/page.rs\tlist\tPage<Contact>
R\tsrc/app/contacts/page.rs\tfind\tContact | null
R\tsrc/app/contacts/page.rs\tevent\tEvent
R\tsrc/app/contacts/page.rs\tloose\t{ id: number; extra: unknown } & Record<string, string>
R\tsrc/app/contacts/page.rs\tmerged\t{ id: number } & Stamp
R\tsrc/app/contacts/page.rs\tnames\tstring[]
R\tsrc/app/contacts/page.rs\tanything\tunknown
R\tsrc/app/contacts/page.rs\tforeign\tElsewhere
R\tsrc/app/contacts/page.rs\tpair\t[Contact, number]
R\tsrc/app/other/page.rs\tsave\t{ other: number }
T\tsrc/app/contacts/page.rs\tSaved\t{ contactId: number; tags: string[]; owner: Contact; note?: string | null; \"content-type\": string }
T\tsrc/app/contacts/page.rs\tContact\t{ id: number; name: string; manager: Contact | null; phones: Array<Phone | null> }
T\tsrc/app/contacts/page.rs\tPhone\t{ digits: string }
T\tsrc/app/contacts/page.rs\tPage<Row>\t{ rows: Row[]; total: number }
T\tsrc/app/contacts/page.rs\tEvent\t{ type: \"ping\" } | { type: \"text\"; body: string }
T\tsrc/app/contacts/page.rs\tStamp\t{ at: string }
";
    Shapes::parse(text, FILE)
}

fn steps(path: &str) -> Vec<Step> {
    path.split('.')
        .map(|name| match name {
            "[]" => Step::Index,
            name => Step::Field {
                name: name.to_string(),
                span: 0..0,
            },
        })
        .collect()
}

/// What reading `path` off `rpc`'s result is refused with, if it is.
fn missing(rpc: &str, path: &str) -> Option<Missing> {
    shapes().missing(rpc, &steps(path))
}

#[test]
fn a_field_the_response_does_not_have_is_named_with_the_ones_it_has() {
    let found = missing("save", "contact_id").expect("a refusal");
    assert_eq!(found.step, 0);
    assert_eq!(found.owner, "Saved");
    assert_eq!(
        found.fields,
        ["contactId", "content-type", "note", "owner", "tags"]
    );
}

#[test]
fn the_fields_a_response_has_are_read_freely() {
    for path in [
        "contactId",
        "tags",
        "owner",
        "note",
        "owner.name",
        "owner.id",
    ] {
        assert_eq!(missing("save", path), None, "{path}");
    }
}

#[test]
fn a_nested_miss_names_the_value_it_was_read_from() {
    let found = missing("save", "owner.nmae").expect("a refusal");
    assert_eq!(found.step, 1);
    assert_eq!(found.owner, "Saved.owner");
    assert_eq!(found.fields, ["id", "manager", "name", "phones"]);
}

#[test]
fn nullable_values_and_array_elements_are_read_through() {
    // `Contact | null`, `Contact | null` inside itself, `Array<Phone | null>`.
    assert_eq!(missing("find", "name"), None);
    assert_eq!(missing("find", "manager.manager.name"), None);
    assert_eq!(missing("save", "owner.phones.[].digits"), None);
    assert_eq!(missing("list", "rows.[].name"), None);
    assert_eq!(missing("find", "nmae").unwrap().owner, "Contact | null");
    assert_eq!(missing("save", "owner.phones.[].number").unwrap().step, 3);
    assert_eq!(missing("list", "rows.[].nmae").unwrap().step, 2);
    assert_eq!(missing("list", "totl").unwrap().fields, ["rows", "total"]);
}

#[test]
fn what_an_array_or_a_string_answers_to_is_never_refused() {
    for (rpc, path) in [
        ("save", "tags.length"),
        ("save", "tags.map"),
        ("save", "tags.[].toUpperCase"),
        ("save", "owner.name.length"),
        ("save", "contactId.toFixed"),
        ("names", "length"),
        ("names", "[].trim"),
        ("list", "rows.find"),
        ("pair", "[].name"),
        ("pair", "length"),
    ] {
        assert_eq!(missing(rpc, path), None, "{rpc}: {path}");
    }
}

#[test]
fn a_shape_that_is_not_fully_known_is_never_refused() {
    for (rpc, path) in [
        // `unknown`, a type the build could not follow, a flattened map.
        ("anything", "whatever.deeper"),
        ("foreign", "whatever"),
        ("loose", "whatever"),
        ("loose", "extra.deeper"),
        // An rpc this file does not declare, and one another file declares.
        ("absent", "whatever"),
        // What every object answers to.
        ("save", "hasOwnProperty"),
        ("save", "toString"),
    ] {
        assert_eq!(missing(rpc, path), None, "{rpc}: {path}");
    }
    // The same name in another file is another rpc.
    assert_eq!(missing("save", "other").unwrap().owner, "Saved");
}

#[test]
fn a_field_of_any_variant_or_any_merged_part_is_a_field() {
    assert_eq!(missing("event", "type"), None);
    assert_eq!(missing("event", "body"), None);
    assert_eq!(missing("event", "bdy").unwrap().fields, ["body", "type"]);
    assert_eq!(missing("merged", "id"), None);
    assert_eq!(missing("merged", "at"), None);
    assert_eq!(missing("merged", "att").unwrap().fields, ["at", "id"]);
}

#[test]
fn every_type_the_build_writes_is_read() {
    for text in [
        "string",
        "number | null",
        "string[]",
        "Array<string | null>",
        "[string, number[]]",
        "Record<string, boolean>",
        "{}",
        "{ id: number; note?: string | null }",
        "{ \"content-type\": string; type: string }",
        "{ id: number } & More",
        "{ a: number } & (B | C)",
        "\"running\" | \"shut_down\"",
        "{ t: \"Ping\" } | { t: \"Text\"; c: string }",
        "{ One: number } | { Pair: [number, string] }",
        "Page<Hit> | null",
        "Traffic<HttpRow>",
        "File[]",
        "unknown",
        "never",
        "null",
    ] {
        assert!(parse(text).is_some(), "{text}");
    }
    // Anything else is a shape nothing is checked against.
    for text in [
        "",
        "{ id: }",
        "Page<",
        "(string",
        "{ a: number",
        "string garbage",
    ] {
        assert!(parse(text).is_none(), "{text:?}");
    }
}

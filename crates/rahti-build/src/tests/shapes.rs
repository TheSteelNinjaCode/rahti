use super::*;
use crate::tree::Tree;

/// The shape of `ty`, written in `app/page.rs` of a crate whose root is the
/// tree. `page` is that file's source; the other files are `(path, source)`.
fn shape_in(page: &str, files: &[(&str, &str)], ty: &str, direction: Direction) -> (Shape, String) {
    let tree = Tree::new("shapes");
    tree.file("lib.rs", "mod services; mod models;");
    tree.file("services/mod.rs", "pub mod contacts; mod labels;");
    tree.file("models/mod.rs", "pub mod user;");
    tree.file("app/page.rs", page);
    for (path, source) in files {
        tree.file(path, source);
    }
    let file = tree.path().join("app/page.rs");
    let mut sources = Sources::default();
    let mut scope = Scope::new(&mut sources, file.to_str().unwrap());
    let shape = scope.shape(&syn::parse_str(ty).unwrap(), direction);
    let types = scope
        .types
        .iter()
        .map(|(head, body)| format!("type {head} = {body};"))
        .collect::<Vec<_>>()
        .join("\n");
    (shape, types)
}

fn sent(page: &str, ty: &str) -> (Shape, String) {
    shape_in(page, &[], ty, Direction::Send)
}

fn text(ty: &str) -> String {
    sent("", ty).0.typescript
}

#[test]
fn standard_types_have_their_json_shapes() {
    assert_eq!(text("String"), "string");
    assert_eq!(text("&str"), "string");
    assert_eq!(text("u64"), "number");
    assert_eq!(text("bool"), "boolean");
    assert_eq!(text("()"), "null");
    assert_eq!(text("Option<i32>"), "number | null");
    assert_eq!(text("Vec<String>"), "string[]");
    assert_eq!(text("Vec<Option<String>>"), "Array<string | null>");
    assert_eq!(text("[u8; 4]"), "number[]");
    assert_eq!(text("(String, Vec<u32>)"), "[string, number[]]");
    assert_eq!(
        text("std::collections::HashMap<String, bool>"),
        "Record<string, boolean>"
    );
    assert_eq!(text("Box<std::sync::Arc<f64>>"), "number");
    assert_eq!(sent("", "Vec<u8>").0.status, Status::Resolved);
}

#[test]
fn a_struct_in_the_same_file_is_declared_by_name() {
    let (shape, types) = sent(
        "#[derive(Serialize)]\npub struct Created { pub id: i32, note: Option<String> }",
        "Vec<Created>",
    );
    assert_eq!(shape.typescript, "Created[]");
    assert_eq!(shape.status, Status::Resolved);
    assert_eq!(types, "type Created = { id: number; note: string | null };");
}

#[test]
fn a_use_declaration_is_followed_to_the_file_that_defines_the_type() {
    let (shape, types) = shape_in(
        "use crate::services::contacts::{Contact as Person, Phone};\nuse crate::models::*;",
        &[
            (
                "services/contacts.rs",
                "use super::labels::Label;\n\
                 #[derive(Clone, Serialize)]\n\
                 pub struct Contact { pub name: String, pub phones: Vec<Phone>, pub label: Label }\n\
                 #[derive(Serialize)]\npub struct Phone(pub String);",
            ),
            (
                "services/labels.rs",
                "#[derive(Serialize)]\npub struct Label { pub color: String }",
            ),
            (
                "models/mod.rs",
                "pub mod user;\n#[derive(Serialize)]\npub struct Account { pub owner: user::Model }",
            ),
            (
                "models/user.rs",
                "#[derive(DeriveEntityModel, Serialize)]\npub struct Model { pub id: i32 }",
            ),
        ],
        "(Person, Account)",
        Direction::Send,
    );
    assert_eq!(shape.typescript, "[Contact, Account]");
    assert_eq!(shape.status, Status::Resolved);
    assert_eq!(
        types,
        "type Account = { owner: Model };\n\
         type Contact = { name: string; phones: Phone[]; label: Label };\n\
         type Label = { color: string };\n\
         type Model = { id: number };\n\
         type Phone = string;"
    );
}

#[test]
fn a_module_path_resolves_through_a_use_or_a_child_module() {
    let files = [(
        "models/user.rs",
        "#[derive(Serialize)]\npub struct Public { pub id: i32 }",
    )];
    for page in [
        "use crate::models::user;",
        "use crate::models::{self as m, user};",
    ] {
        let (shape, _) = shape_in(page, &files, "user::Public", Direction::Send);
        assert_eq!(shape.typescript, "Public", "{page}");
    }
    let (shape, _) = shape_in("", &files, "crate::models::user::Public", Direction::Send);
    assert_eq!(shape.status, Status::Resolved);
}

#[test]
fn serde_attributes_decide_the_keys() {
    let (_, types) = sent(
        r#"
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Row {
            pub first_name: String,
            #[serde(rename = "ID")]
            pub id: u32,
            #[serde(skip)]
            pub secret: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub note: Option<String>,
            #[serde(rename = "content-type")]
            pub kind: String,
            #[serde(flatten)]
            pub more: More,
            #[serde(with = "stamp")]
            pub at: u64,
        }
        #[derive(Serialize)]
        pub struct More { pub r#type: String }
        "#,
        "Row",
    );
    assert_eq!(
        types,
        "type More = { type: string };\n\
         type Row = { firstName: string; ID: number; note?: string | null; \
         \"content-type\": string; at: unknown } & More;"
    );
}

#[test]
fn an_unreadable_part_makes_the_shape_partial_not_wrong() {
    let page = "use other_crate::Thing;\n\
                #[derive(Serialize)]\npub struct Row { pub id: u32, pub thing: Thing }\n\
                pub struct Manual { pub id: u32 }";
    let (shape, types) = sent(page, "Row");
    assert_eq!(shape.status, Status::Partial);
    assert_eq!(types, "type Row = { id: number; thing: unknown };");
    // A hand-written `Serialize` is not described by its fields.
    assert_eq!(sent(page, "Manual").0.status, Status::Unresolved);
    assert_eq!(sent(page, "Missing").0.status, Status::Unresolved);
    assert_eq!(sent(page, "Thing").0.typescript, "unknown");
}

#[test]
fn json_of_any_shape_is_dynamic_and_dates_are_strings() {
    let page = "use serde_json::Value;\nuse chrono::{DateTime, Utc};";
    let (shape, _) = sent(page, "Value");
    assert_eq!(
        (shape.typescript.as_str(), shape.status),
        ("unknown", Status::Dynamic)
    );
    let (shape, _) = sent(page, "Vec<DateTime<Utc>>");
    assert_eq!(
        (shape.typescript.as_str(), shape.status),
        ("string[]", Status::Resolved)
    );
}

#[test]
fn enums_follow_their_tagging() {
    let page = r#"
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum State { Running, ShutDown, #[serde(rename = "gone")] Removed }

        #[derive(Serialize)]
        pub enum External { Empty, One(u32), Pair(u32, String), Named { id: u32 } }

        #[derive(Serialize)]
        #[serde(tag = "type", rename_all = "lowercase")]
        pub enum Internal { Ping, Text { body: String } }

        #[derive(Serialize)]
        #[serde(tag = "t", content = "c")]
        pub enum Adjacent { Ping, Text(String) }

        #[derive(Serialize)]
        #[serde(untagged)]
        pub enum Untagged { Number(u32), Text(String) }
    "#;
    let (_, types) = sent(page, "(State, External, Internal, Adjacent, Untagged)");
    assert_eq!(
        types,
        "type Adjacent = { t: \"Ping\" } | { t: \"Text\"; c: string };\n\
         type External = \"Empty\" | { One: number } | { Pair: [number, string] } | \
         { Named: { id: number } };\n\
         type Internal = { type: \"ping\" } | { type: \"text\"; body: string };\n\
         type State = \"running\" | \"shut_down\" | \"gone\";\n\
         type Untagged = number | string;"
    );
}

#[test]
fn generic_types_keep_their_parameters() {
    let (shape, types) = sent(
        "#[derive(Serialize)]\npub struct Page<Row> { pub rows: Vec<Row>, pub total: u64 }\n\
         #[derive(Serialize)]\npub struct Hit { pub url: String }\n\
         pub type Hits = Page<Hit>;",
        "Option<Hits>",
    );
    assert_eq!(shape.typescript, "Hits | null");
    assert_eq!(shape.status, Status::Resolved);
    assert_eq!(
        types,
        "type Hit = { url: string };\n\
         type Hits = Page<Hit>;\n\
         type Page<Row> = { rows: Row[]; total: number };"
    );
}

#[test]
fn a_type_that_contains_itself_refers_to_its_own_name() {
    let (shape, types) = sent(
        "#[derive(Serialize)]\npub struct Node { pub name: String, pub children: Vec<Node> }",
        "Node",
    );
    assert_eq!(shape.status, Status::Resolved);
    assert_eq!(types, "type Node = { name: string; children: Node[] };");
}

#[test]
fn direction_decides_which_attributes_and_derives_apply() {
    let page = r#"
        #[derive(Serialize, Deserialize)]
        pub struct Draft {
            pub title: String,
            #[serde(default)]
            pub tags: Vec<String>,
            pub note: Option<String>,
            #[serde(skip_deserializing)]
            pub id: u32,
        }
        #[derive(Serialize)]
        pub struct Sent { pub id: u32 }
    "#;
    // Read in both directions by one file, a type whose two shapes differ is
    // two declarations, and one whose shapes agree is one.
    let both = "#[derive(Serialize, Deserialize)]
pub struct Same { pub id: u32 }";
    let tree = Tree::new("shapes-both");
    tree.file(
        "app/page.rs",
        &format!(
            "{page}
{both}"
        ),
    );
    let file = tree.path().join("app/page.rs");
    let mut sources = Sources::default();
    let mut scope = Scope::new(&mut sources, file.to_str().unwrap());
    for name in ["Draft", "Same"] {
        let ty: Type = syn::parse_str(name).unwrap();
        let out = scope.shape(&ty, Direction::Send).typescript;
        let input = scope.shape(&ty, Direction::Receive).typescript;
        assert_eq!((out == input), name == "Same", "{name}: {out} / {input}");
    }
    assert!(scope.types.contains_key("DraftInput"));
    assert_eq!(scope.types.len(), 3);

    let (received, types) = shape_in(page, &[], "Draft", Direction::Receive);
    assert_eq!(received.status, Status::Resolved);
    assert_eq!(
        types,
        "type Draft = { title: string; tags?: string[]; note?: string | null };"
    );
    let (_, types) = sent(page, "Draft");
    assert_eq!(
        types,
        "type Draft = { title: string; tags: string[]; note: string | null; id: number };"
    );
    // `Sent` cannot be deserialized, so it describes no payload.
    let (shape, _) = shape_in(page, &[], "Sent", Direction::Receive);
    assert_eq!(shape.status, Status::Unresolved);
}

#[test]
fn every_file_a_shape_was_read_from_is_reported() {
    let tree = Tree::new("shapes-read");
    tree.file("lib.rs", "");
    tree.file("app/page.rs", "use crate::wire::Row;");
    tree.file(
        "wire.rs",
        "#[derive(Serialize)]\npub struct Row { pub id: u32 }",
    );
    let file = tree.path().join("app/page.rs");
    let mut sources = Sources::default();
    Scope::new(&mut sources, file.to_str().unwrap())
        .shape(&syn::parse_str("Row").unwrap(), Direction::Send);
    assert!(sources.read.contains(&tree.path().join("wire.rs")));
}

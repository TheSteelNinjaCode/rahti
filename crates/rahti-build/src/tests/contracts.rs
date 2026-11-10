use super::*;

/// A contract for a function in no file: nothing it names can be followed.
fn describe_alone(function: &syn::ItemFn) -> Value {
    let mut sources = Sources::default();
    describe(function, &mut Scope::new(&mut sources, "nowhere/page.rs"))
}

#[test]
fn discovery_keeps_page_scopes_urls_and_component_ownership() {
    let tree = crate::tree::Tree::new("rpc-contracts");
    tree.file(
        "app/page.rs",
        "#[rpc]\npub fn load(id: u32) -> String { todo!() }",
    );
    tree.file(
        "app/other/page.rs",
        "#[rpc(auth)]\npub fn load(id: u32) -> String { todo!() }",
    );
    tree.file(
        "components/card.rs",
        "#[rpc]\npub fn save(name: String) -> bool { todo!() }",
    );
    let pages = crate::scanner::scan(&tree.path().join("app"));
    let components = crate::components::scan(&tree.path().join("components"));
    let contracts = inventory(
        &pages.entries,
        &components.rpcs,
        "nowhere/routes.rs",
        &components.files,
    )
    .contracts;
    let values = contracts.as_array().unwrap();
    assert_eq!(values.len(), 3);
    let loads: Vec<_> = values.iter().filter(|c| c["name"] == "load").collect();
    assert_eq!(loads.len(), 2);
    assert_ne!(loads[0]["urls"], loads[1]["urls"]);
    assert_ne!(loads[0]["authRequired"], loads[1]["authRequired"]);
    let save = values.iter().find(|c| c["name"] == "save").unwrap();
    assert_eq!(save["scope"], "component");
    assert_eq!(save["urls"], json!([]));
    assert_eq!(save["module"], "card");
    assert_eq!(save["line"], 2);
}

#[test]
fn contracts_describe_auth_files_and_success_without_inventing_a_schema() {
    let f: syn::ItemFn = syn::parse_str(r#"
            #[rpc(auth)]
            pub async fn save(id: i32, note: Option<String>, files: Vec<RpcFile>) -> rahti::Result<Contact> { todo!() }
        "#).unwrap();
    let contract = describe_alone(&f);
    assert_eq!(contract["authRequired"], true);
    assert_eq!(contract["parameters"][0]["required"], true);
    assert_eq!(contract["parameters"][1]["required"], false);
    assert_eq!(contract["parameters"][2]["transport"], "files");
    assert_eq!(contract["parameters"][2]["required"], false);
    assert_eq!(contract["response"]["rustType"], "Contact");
    assert_eq!(contract["response"]["schemaStatus"], "unresolved");
    assert_eq!(contract["response"]["typescript"], "unknown");
    assert_eq!(contract["response"]["fallible"], true);
    assert_eq!(contract["parameters"][0]["typescript"], "number");
    assert_eq!(contract["parameters"][0]["schemaStatus"], "resolved");
    assert_eq!(contract["parameters"][2]["typescript"], "File[]");
    assert_eq!(
        contract["payload"]["typescript"],
        "{ id: number; note?: string | null; files?: File[] }"
    );
    assert_eq!(contract["payload"]["schemaStatus"], "resolved");
}

#[test]
fn contracts_carry_the_shapes_their_file_can_account_for() {
    let tree = crate::tree::Tree::new("rpc-shapes");
    tree.file("lib.rs", "mod wire;");
    tree.file(
        "wire.rs",
        "#[derive(Serialize, Deserialize)]\npub struct Contact { pub id: i32, pub name: String }",
    );
    tree.file(
        "app/page.rs",
        "use crate::wire::Contact;\n\
         #[rpc]\npub async fn save(contact: Contact) -> Result<Vec<Contact>> { todo!() }\n\
         #[rpc]\npub async fn remove(id: i32) -> Result<()> { todo!() }",
    );
    tree.file(
        "app/other/page.rs",
        "#[rpc]\npub fn ping() -> bool { true }",
    );
    let pages = crate::scanner::scan(&tree.path().join("app"));
    let found = inventory(&pages.entries, &[], "nowhere/routes.rs", &[]);
    let contracts = found.contracts.as_array().unwrap();
    let save = contracts.iter().find(|c| c["name"] == "save").unwrap();
    assert_eq!(save["payload"]["typescript"], "{ contact: Contact }");
    assert_eq!(save["response"]["typescript"], "Contact[]");
    assert_eq!(save["response"]["schemaStatus"], "resolved");
    let remove = contracts.iter().find(|c| c["name"] == "remove").unwrap();
    assert_eq!(remove["response"]["typescript"], "null");

    // One declaration per file, however many rpcs name it; a file whose
    // rpcs name no type has no entry.
    let types = found.types.as_object().unwrap();
    assert_eq!(types.len(), 1);
    let file = save["file"].as_str().unwrap();
    assert_eq!(
        types[file],
        json!({ "Contact": "{ id: number; name: string }" })
    );
    // What `html!` reads: one line per response and one per declaration.
    let lines: Vec<&str> = found.shapes.lines().collect();
    assert!(lines.contains(&format!("R	{file}	save	Contact[]").as_str()));
    assert!(lines.contains(&format!("R	{file}	remove	null").as_str()));
    assert!(lines.contains(&format!("T	{file}	Contact	{{ id: number; name: string }}").as_str()));
    assert_eq!(save["checks"]["literalKeys"], "compile-time");
    assert!(
        save["checks"]["responseUsage"]
            .as_str()
            .unwrap()
            .starts_with("compile-time")
    );
    assert!(found.read.iter().any(|path| path.ends_with("/wire.rs")));
    assert!(found.read.iter().all(|path| !path.contains('\\')));
}

#[test]
fn streams_are_not_described_as_json_responses() {
    let f = syn::parse_str("#[rpc] fn events() -> Result<RpcStream> { todo!() }").unwrap();
    let contract = describe_alone(&f);
    assert_eq!(contract["response"]["transport"], "stream");
    assert_eq!(contract["response"]["schemaStatus"], "stream");
    assert_eq!(contract["response"]["typescript"], Value::Null);
}

/// Every struct the module generated beside the routes can name, by path,
/// and nothing it might not be able to.
#[test]
fn the_crates_reachable_structs_are_listed_by_path() {
    let tree = crate::tree::Tree::new("crate-wide-shapes");
    tree.file(
        "lib.rs",
        "mod services;
         pub mod routes;
         mod hidden { #[derive(serde::Serialize)] pub struct Inline { id: u32 } }
         #[cfg(test)]
         mod only_in_tests;
         #[derive(serde::Serialize)]
         struct AtTheRoot { id: u32 }",
    );
    tree.file(
        "services.rs",
        "pub mod projects;
         mod internal;
         #[derive(serde::Serialize)]
         pub(crate) struct Summary { total: u32 }
         #[derive(serde::Serialize)]
         struct Private { id: u32 }",
    );
    tree.file(
        "services/projects.rs",
        "use serde::Serialize;
         #[derive(Serialize)]
         #[serde(rename_all = \"camelCase\")]
         pub struct Project { id: u32, display_name: String, owner: Option<Owner> }
         #[derive(Serialize)]
         pub struct Owner { email: String }
         #[derive(Serialize)]
         pub struct Page<T> { rows: Vec<T> }
         #[derive(Serialize)]
         pub struct Newtype(u32);
         pub struct Manual { id: u32 }
         #[cfg(feature = \"extra\")]
         #[derive(Serialize)]
         pub struct Gated { id: u32 }",
    );
    tree.file(
        "services/internal.rs",
        "#[derive(serde::Serialize)] pub struct Unreachable { id: u32 }",
    );
    tree.file(
        "only_in_tests.rs",
        "#[derive(serde::Serialize)] pub struct Fixture { id: u32 }",
    );
    tree.file(
        "routes.rs",
        "#[path = \"app/page.rs\"]
         mod p_root;",
    );
    tree.file(
        "app/page.rs",
        "#[derive(serde::Serialize)]
         pub struct Row { id: u32 }
         #[derive(serde::Serialize)]
         struct Local { id: u32 }",
    );
    let routes = tree.path().join("routes.rs");
    let shapes = inventory(&[], &[], &routes.to_string_lossy(), &[]).shapes;
    let listed: Vec<&str> = shapes
        .lines()
        .filter_map(|line| line.strip_prefix("G\t*\t"))
        .collect();
    assert_eq!(
        listed,
        [
            // Private at the root is visible below the root.
            "crate::AtTheRoot\tAtTheRoot",
            // A private module of the root is visible below the root; a
            // private struct inside it is not.
            "crate::services::Summary\tSummary",
            "crate::services::projects::Project\tProject",
            "crate::services::projects::Owner\tOwner",
            // A page's module is private to the routes, which the generated
            // module sits in; the page's own private struct is out of reach.
            "crate::routes::p_root::Row\tRow",
            "crate::hidden::Inline\tInline",
        ],
        "{shapes}"
    );
    // The JSON Serde writes, renames included.
    assert!(
        shapes.lines().any(|line| line
            == "T\t*\tProject\t{ id: number; displayName: string; owner: Owner | null }"),
        "{shapes}"
    );
}

#[test]
fn a_crate_without_a_routes_module_lists_nothing() {
    let tree = crate::tree::Tree::new("no-routes-module");
    tree.file(
        "lib.rs",
        "#[derive(serde::Serialize)] pub struct Row { id: u32 }",
    );
    let routes = tree.path().join("routes.rs");
    let shapes = inventory(&[], &[], &routes.to_string_lossy(), &[]).shapes;
    assert!(!shapes.contains("G\t"), "{shapes}");
}

// ----- the props a component reads ------------------------------------------

fn read(source: &str) -> Option<Vec<String>> {
    props_read(source).map(|names| names.into_iter().collect())
}

#[test]
fn the_props_a_component_file_reads_are_its_pp_props_names() {
    let source = "
        #[component]
        pub fn Dialog(title: &str, bindings: Bindings) -> Html {
            html! {
                <div @{..bindings}>
                    <p hidden={!pp.props.open}>{pp.props?.current ?? \"\"}</p>
                    <script>r##\"
                        const label = pp.props.name || app.props.other;
                    \"##</script>
                </div>
            }
        }";
    assert_eq!(
        read(source),
        Some(vec!["current".into(), "name".into(), "open".into()])
    );
    // A component that reads none.
    assert_eq!(
        read("#[component] pub fn Divider() -> Html { html! { <hr /> } }"),
        Some(vec![])
    );
}

#[test]
fn a_component_that_may_read_a_prop_unseen_reads_any() {
    for source in [
        // `pp.props` used whole.
        "const { open } = pp.props;",
        "show(pp.props)",
        "pp.props[key]",
        // The bindings forwarded to another component, which reads them.
        "fn Outer(bindings: Bindings) -> Html { html! { <Inner @{..bindings} /> } }",
        // Or handed on, or built into something else.
        "fn Outer(bindings: Bindings) -> Html { inner(bindings) }",
        "fn Outer(bindings: Bindings) -> Html { let all = bindings.merge(more); html! { <div @{..all}></div> } }",
    ] {
        assert_eq!(read(source), None, "{source}");
    }
    // A spread on a plain element, however the tag is laid out.
    assert_eq!(
        read(
            "fn Card(bindings: rahti::Bindings) -> Html { html! { <div\n class=\"card\"\n @{ ..bindings }>{pp.props.open}</div> } }"
        ),
        Some(vec!["open".into()])
    );
}

#[test]
fn a_file_lists_each_component_it_can_call_with_its_props() {
    let tree = crate::tree::Tree::new("component-props");
    tree.file("lib.rs", "mod components; pub mod routes;");
    tree.file(
        "components.rs",
        "pub mod dialog; pub mod button; pub mod plain;",
    );
    tree.file(
        "components/dialog.rs",
        "#[component]
         pub fn Dialog(bindings: Bindings) -> Html {
             html! { <div @{..bindings} hidden={!pp.props.open}>{pp.props.current}</div> }
         }
         pub fn Helper() -> Html { todo!() }",
    );
    tree.file(
        "components/button.rs",
        "#[component]
         pub fn Button(bindings: Bindings) -> Html { html! { <Inner @{..bindings} /> } }",
    );
    tree.file("components/plain.rs", "pub struct Plain;");
    tree.file("routes.rs", "#[path = \"app/page.rs\"] mod p_root;");
    tree.file(
        "app/page.rs",
        "use crate::components::dialog::{Dialog as Modal, Helper};
         use crate::components::button::Button;
         use crate::components::plain::Plain;
         use serde::Serialize;
         #[component]
         fn Local() -> Html { html! { <p>{pp.props.label}</p> } }
         pub fn page() -> Html { todo!() }",
    );
    let pages = crate::scanner::scan(&tree.path().join("app"));
    let routes = tree.path().join("routes.rs");
    let shapes = inventory(&pages.entries, &[], &routes.to_string_lossy(), &[]).shapes;
    let listed: Vec<String> = shapes
        .lines()
        .filter(|line| line.starts_with("C\t"))
        .map(|line| line.splitn(3, '\t').nth(2).unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        [
            // A component may read any prop: its bindings go to another.
            "Button\t*",
            // One this file declares.
            "Local\tlabel",
            // One imported under another name: the name the tag is written with.
            "Modal\tcurrent open",
        ],
        "{shapes}"
    );
}

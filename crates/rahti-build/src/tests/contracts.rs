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
    let contracts = inventory(&pages.entries, &components.rpcs).contracts;
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
    let found = inventory(&pages.entries, &[]);
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

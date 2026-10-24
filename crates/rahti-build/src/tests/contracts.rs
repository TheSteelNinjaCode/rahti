use super::*;

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
    let contracts = inventory(&pages.entries, &components.rpcs);
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
    let contract = describe(&f);
    assert_eq!(contract["authRequired"], true);
    assert_eq!(contract["parameters"][0]["required"], true);
    assert_eq!(contract["parameters"][1]["required"], false);
    assert_eq!(contract["parameters"][2]["transport"], "files");
    assert_eq!(contract["parameters"][2]["required"], false);
    assert_eq!(contract["response"]["rustType"], "Contact");
    assert_eq!(contract["response"]["schemaStatus"], "unresolved");
    assert_eq!(contract["response"]["fallible"], true);
}

#[test]
fn streams_are_not_described_as_json_responses() {
    let f = syn::parse_str("#[rpc] fn events() -> Result<RpcStream> { todo!() }").unwrap();
    assert_eq!(describe(&f)["response"]["transport"], "stream");
}

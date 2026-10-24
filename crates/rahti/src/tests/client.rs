use super::*;

#[test]
fn the_module_registers_each_entry_once_in_a_stable_order() {
    let entries = [
        ClientCode {
            kind: ClientKind::Handler,
            key: "save()",
            js: "function (__pp_scope, __pp_event) {}",
        },
        ClientCode {
            kind: ClientKind::Expression,
            key: "b + 1",
            js: "(__pp_scope) => 2",
        },
        ClientCode {
            kind: ClientKind::Expression,
            key: "a",
            js: "(__pp_scope) => 1",
        },
        ClientCode {
            kind: ClientKind::Expression,
            key: "a",
            js: "(__pp_scope) => 1",
        },
    ];

    let module = build(&entries);
    let reversed = build(entries.iter().rev());

    assert_eq!(module.source, reversed.source);
    assert_eq!(module.etag, reversed.etag);
    assert_eq!(module.source.matches("\"a\":").count(), 1);
    assert!(module.source.contains("globalThis.pp.registerClient({"));
    let a = module.source.find("\"a\":").unwrap();
    let b = module.source.find("\"b + 1\":").unwrap();
    let save = module.source.find("\"save()\":").unwrap();
    assert!(a < b && b < save);
}

#[test]
fn script_data_is_one_escaped_json_array() {
    let mut out = String::new();
    write_script_data(&[&"a\"</script>", &3_u8, &Option::<u8>::None], &mut out);
    assert_eq!(
        out,
        " pp-data=\"[&quot;a\\&quot;\\u003c/script&gt;&quot;,3,null]\""
    );
}

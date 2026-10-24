use super::*;

#[test]
fn an_expression_reads_its_free_names_from_the_scope() {
    let js = expression("items.filter((x) => x.ok).length + offset").unwrap();
    assert!(js.contains(r#"let items = __pp_read(__pp_scope, "items");"#));
    assert!(js.contains(r#"let offset = __pp_read(__pp_scope, "offset");"#));
    assert!(!js.contains(r#""x""#));
}

#[test]
fn a_binding_must_be_one_expression() {
    assert!(expression("a) + (b").is_err());
    assert!(expression("color: red;").is_err());
    assert!(expression("").is_err());
    assert!(is_expression("a ? b : c"));
}

#[test]
fn a_handler_is_an_expression_or_statements() {
    let call = handler("save(e)").unwrap();
    assert!(call.contains("return (\nsave(e)\n);"));
    assert!(call.contains(r#"let e = "e" in __pp_scope"#));
    let body = handler("if (open) { close(); }").unwrap();
    assert!(body.contains("{\nif (open) { close(); }\n}"));
}

#[test]
fn a_script_returns_its_top_level_bindings() {
    let script = component_script(
        "const [count, setCount] = pp.state(__pp_server[0]);\nfunction inc() {}\nclass A {}",
    )
    .unwrap();
    assert_eq!(script.bindings, ["count", "setCount", "inc", "A"]);
    assert!(
        script
            .js
            .contains("return { pp, count, setCount, inc, A };")
    );
    assert!(component_script("const x = ;").is_err());
}

fn literal(name: &str) -> impl Fn(&RpcCall) -> bool + '_ {
    move |call| matches!(&call.name, RpcName::Literal { value, .. } if value == name)
}

fn keys(call: &RpcCall) -> (Vec<&str>, bool) {
    match &call.payload {
        Payload::Object { keys, open, .. } => {
            (keys.iter().map(|key| key.name.as_str()).collect(), *open)
        }
        other => panic!("not an object literal: {other:?}"),
    }
}

#[test]
fn a_literal_name_and_object_literal_are_collected_with_spans() {
    let source = r#"pp.rpc("greet", { name: who, "count": 2 })"#;
    let calls = rpc_calls(Code::Expression, source);
    assert_eq!(calls.len(), 1);
    let RpcName::Literal { value, span } = &calls[0].name else {
        panic!("{:?}", calls[0].name);
    };
    assert_eq!(value, "greet");
    assert_eq!(&source[span.clone()], r#""greet""#);
    let Payload::Object { keys, open, .. } = &calls[0].payload else {
        panic!("{:?}", calls[0].payload);
    };
    assert!(!open);
    assert_eq!(&source[keys[0].span.clone()], "name");
    assert_eq!(&source[keys[1].span.clone()], r#""count""#);
    assert_eq!(keys[1].name, "count");
}

#[test]
fn shorthand_keys_are_keys() {
    let calls = rpc_calls(Code::Handler, "pp.rpc('create', { title, done: false })");
    assert_eq!(keys(&calls[0]), (vec!["title", "done"], false));
}

#[test]
fn literal_shapes_preserve_effective_values_and_value_spans() {
    let source = r#"pp.rpc("save", { name: 12, name: "Ada", count: -2, ok: false, note: null, tags: [], data: {}, dynamic: getValue() })"#;
    let calls = rpc_calls(Code::Expression, source);
    let Payload::Object { keys, .. } = &calls[0].payload else {
        panic!()
    };
    assert_eq!(
        keys.iter().map(|k| k.value).collect::<Vec<_>>(),
        [
            Literal::String,
            Literal::Number,
            Literal::Boolean,
            Literal::Null,
            Literal::Array,
            Literal::Object,
            Literal::Unknown,
        ]
    );
    assert_eq!(&source[keys[0].value_span.clone()], "\"Ada\"");
    assert_eq!(&source[keys[1].value_span.clone()], "-2");
}

#[test]
fn getters_are_not_their_expression_shapes() {
    let calls = rpc_calls(
        Code::Expression,
        r#"pp.rpc("save", { get name() { return "Ada"; } })"#,
    );
    let Payload::Object { keys, .. } = &calls[0].payload else {
        panic!()
    };
    assert_eq!(keys[0].value, Literal::Unknown);
}

#[test]
fn json_hooks_and_nonfinite_numbers_have_no_proven_wire_shape() {
    for expression in [
        "{ toJSON() { return 'text'; } }",
        "{ ...data }",
        "{ __proto__: custom }",
        "1e999",
        "-1e999",
    ] {
        let calls = rpc_calls(
            Code::Expression,
            &format!("pp.rpc('save', {{ value: {expression} }})"),
        );
        let Payload::Object { keys, .. } = &calls[0].payload else {
            panic!()
        };
        assert_eq!(keys[0].value, Literal::Unknown, "{expression}");
    }
}

#[test]
fn a_spread_or_computed_key_leaves_the_object_open() {
    let calls = rpc_calls(
        Code::Script,
        "pp.rpc(\"a\", { ...rest, id });\npp.rpc(\"b\", { [field]: 1, id });",
    );
    assert_eq!(keys(&calls[0]), (vec!["id"], true));
    assert_eq!(keys(&calls[1]), (vec!["id"], true));
}

#[test]
fn a_payload_built_at_runtime_is_unchecked_and_a_missing_one_is_missing() {
    let calls = rpc_calls(
        Code::Script,
        "pp.rpc(\"a\", Object.fromEntries(new FormData(event.currentTarget)));\n\
         pp.rpc(\"b\", new FormData(form));\npp.rpc(\"c\", data);\npp.rpc(\"d\");",
    );
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].payload, Payload::Unchecked);
    assert_eq!(calls[1].payload, Payload::Unchecked);
    assert_eq!(calls[2].payload, Payload::Unchecked);
    assert_eq!(calls[3].payload, Payload::Missing);
}

#[test]
fn calls_inside_functions_and_arrows_are_collected() {
    let source = "async function save() { await pp.rpc(\"save\", {}); }\n\
                  const load = async () => pp.rpc(\"load\", { id });\n\
                  items.forEach((item) => { pp.rpc(\"each\", { item }); });";
    let calls = rpc_calls(Code::Script, source);
    assert!(literal("save")(&calls[0]));
    assert!(literal("load")(&calls[1]));
    assert!(literal("each")(&calls[2]));
    let handler = rpc_calls(Code::Handler, "if (ok) { pp.rpc(\"x\", { y }); }");
    assert!(literal("x")(&handler[0]));
}

#[test]
fn a_name_that_is_not_a_literal_is_reported_as_such() {
    let source = "pp.rpc(name, {}); pp.rpc(\"a\" + b); pp.rpc(`x${y}`); pp.rpc(`plain`); pp.rpc()";
    let calls = rpc_calls(Code::Handler, source);
    assert!(
        matches!(&calls[0].name, RpcName::NotLiteral { span } if &source[span.clone()] == "name")
    );
    assert!(matches!(calls[1].name, RpcName::NotLiteral { .. }));
    assert!(matches!(calls[2].name, RpcName::NotLiteral { .. }));
    assert!(literal("plain")(&calls[3]));
    assert!(
        matches!(&calls[4].name, RpcName::Missing { span } if &source[span.clone()] == "pp.rpc()")
    );
}

#[test]
fn only_pp_dot_rpc_is_collected() {
    let source = "pp[\"rpc\"](\"a\", {}); rpc(\"b\", {}); api.rpc(\"c\", {}); pp.rpcs(\"d\"); \
                  pp.socket(\"e\", {})";
    assert!(rpc_calls(Code::Handler, source).is_empty());
}

#[test]
fn code_that_does_not_parse_has_no_calls() {
    assert!(rpc_calls(Code::Expression, "pp.rpc(\"a\", {").is_empty());
}

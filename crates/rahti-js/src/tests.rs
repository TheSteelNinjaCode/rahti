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

/// The reads of an rpc result in a script, as `rpc: a.b` lines.
fn reads(source: &str) -> Vec<String> {
    rpc_reads(Code::Script, source)
        .into_iter()
        .map(|read| {
            let path: Vec<String> = read
                .steps
                .iter()
                .map(|step| match step {
                    Step::Field { name, .. } => name.clone(),
                    Step::Index => "[]".to_string(),
                })
                .collect();
            format!("{}: {}", read.rpc, path.join("."))
        })
        .collect()
}

#[test]
fn a_field_read_off_an_awaited_rpc_result_is_found() {
    let source = r#"
        async function save() {
            const result = await pp.rpc("save", { name });
            setId(result.contactId);
            const first = result.tags[0].label;
            if (result?.owner?.name) show((result).note);
        }
    "#;
    assert_eq!(
        reads(source),
        [
            "save: contactId",
            "save: tags.[].label",
            "save: tags",
            "save: owner.name",
            "save: owner",
            "save: note",
        ]
    );
}

#[test]
fn a_read_names_the_span_of_the_field_it_reads() {
    let source = r#"const r = await pp.rpc("load"); r.total;"#;
    let read = &rpc_reads(Code::Script, source)[0];
    let Step::Field { name, span } = &read.steps[0] else {
        panic!("a field step");
    };
    assert_eq!(name, "total");
    assert_eq!(&source[span.clone()], "total");
}

#[test]
fn destructuring_an_awaited_rpc_result_reads_each_key() {
    let source = r#"const { tokens, message: text, ...rest } = await pp.rpc(`issue`);"#;
    assert_eq!(reads(source), ["issue: tokens", "issue: message"]);
}

#[test]
fn only_a_script_holds_an_awaited_result() {
    let source = r#"async function f() { const made = await pp.rpc("create", {}); go(made.id); }"#;
    assert_eq!(rpc_reads(Code::Script, source).len(), 1);
    assert!(rpc_reads(Code::Handler, source).is_empty());
    assert!(rpc_reads(Code::Expression, r#"pp.rpc("create").id"#).is_empty());
}

#[test]
fn values_that_are_not_certainly_the_response_are_left_alone() {
    for source in [
        // Assigned again: the second value is something else.
        r#"let r = await pp.rpc("a"); r = other(); r.field;"#,
        // Given fields of its own.
        r#"const r = await pp.rpc("a"); r.extra = 1; r.extra;"#,
        r#"const r = await pp.rpc("a"); r.count++; r.count;"#,
        r#"const r = await pp.rpc("a"); delete r.gone; r.gone;"#,
        r#"const r = await pp.rpc("a"); Object.assign(r, { extra: 1 }); r.extra;"#,
        // Not awaited, not `pp.rpc`, or not named by a literal.
        r#"const r = pp.rpc("a"); r.then;"#,
        r#"const r = await other.rpc("a"); r.field;"#,
        r#"const r = await pp.rpc(which); r.field;"#,
        // Another variable of the same name, in another scope.
        r#"async function a() { const r = await pp.rpc("a"); } function b(r) { r.field; }"#,
        // Handed on: what the callee reads is its own business.
        r#"const r = await pp.rpc("a"); setState(r); show(r);"#,
        r#"pp.rpc("a").then(pick).then((r) => r.field);"#,
        // A computed key is only known at run time.
        r#"const r = await pp.rpc("a"); r[key];"#,
    ] {
        assert!(reads(source).is_empty(), "{source}: {:?}", reads(source));
    }
}

#[test]
fn a_shadowing_variable_is_not_the_result() {
    let source = r#"
        const r = await pp.rpc("outer");
        r.kept;
        function inner(r) { return r.other; }
        items.map((r) => r.row);
    "#;
    assert_eq!(reads(source), ["outer: kept"]);
}

// ----- state ---------------------------------------------------------------

fn uses_of<'a>(found: &'a StateUses, name: &str) -> Vec<&'a Use> {
    found
        .uses
        .iter()
        .filter(|used| used.name == name)
        .map(|used| &used.what)
        .collect()
}

fn fields(steps: &[Step]) -> Vec<&str> {
    steps
        .iter()
        .map(|step| match step {
            Step::Field { name, .. } => name.as_str(),
            Step::Index => "[]",
        })
        .collect()
}

fn rpc_origin(origin: &Origin) -> (&str, Vec<&str>) {
    match origin {
        Origin::Rpc { rpc, steps } => (rpc.as_str(), fields(steps)),
        other => panic!("not an rpc origin: {other:?}"),
    }
}

#[test]
fn a_script_declares_its_states_and_their_initial_values() {
    let found = state_uses(
        Code::Script,
        "const [items, setItems] = pp.state([]);\n\
         const [user, setUser] = pp.state(__pp_server[0]);\n\
         const [tab, setTab] = pp.state(\"all\");\n\
         const [only] = pp.state(null);",
    );
    let names: Vec<_> = found.states.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["items", "user", "tab", "only"]);
    assert_eq!(found.states[0].setter.as_deref(), Some("setItems"));
    assert_eq!(found.states[0].initial, [Origin::Empty]);
    assert_eq!(found.states[1].initial, [Origin::Server(Some(0))]);
    assert_eq!(found.states[2].initial, [Origin::Unknown("\"all\"".into())]);
    assert_eq!(found.states[3].setter, None);
}

#[test]
fn a_setter_is_called_with_an_rpc_result_or_a_part_of_one() {
    let found = state_uses(
        Code::Script,
        "const [items, setItems] = pp.state([]);\n\
         const [owner, setOwner] = pp.state(null);\n\
         async function load() {\n\
           setItems(await pp.rpc(\"list\", {}));\n\
           const page = await pp.rpc(\"page\", {});\n\
           setItems(page.rows ?? []);\n\
           setOwner(page.owner);\n\
           setItems([]);\n\
         }",
    );
    let sets = uses_of(&found, "setItems");
    let [Use::Call(direct), Use::Call(part), Use::Call(cleared)] = sets.as_slice() else {
        panic!("{sets:?}");
    };
    assert_eq!(rpc_origin(&direct[0]), ("list", vec![]));
    assert_eq!(rpc_origin(&part[0]), ("page", vec!["rows"]));
    assert_eq!(part[1], Origin::Empty);
    assert_eq!(cleared.as_slice(), [Origin::Empty]);
    let [Use::Call(owner)] = uses_of(&found, "setOwner")[..] else {
        panic!();
    };
    assert_eq!(rpc_origin(&owner[0]), ("page", vec!["owner"]));
}

#[test]
fn a_result_awaited_in_either_branch_comes_from_both_rpcs() {
    let found = state_uses(
        Code::Script,
        "const [item, setItem] = pp.state(null);\n\
         async function save(creating) {\n\
           const next = creating ? await pp.rpc(\"create\", {}) : await pp.rpc(\"update\", {});\n\
           setItem(next);\n\
         }",
    );
    let [Use::Call(origins)] = uses_of(&found, "setItem")[..] else {
        panic!();
    };
    assert_eq!(rpc_origin(&origins[0]).0, "create");
    assert_eq!(rpc_origin(&origins[1]).0, "update");
}

#[test]
fn a_setter_given_anything_else_says_what_it_was_given() {
    let found = state_uses(
        Code::Script,
        "const [items, setItems] = pp.state([]);\n\
         async function load() {\n\
           const list = await pp.rpc(\"list\", {});\n\
           setItems([...items, list[0]]);\n\
           setItems((current) => current.slice(1));\n\
           list.extra = 1;\n\
           setItems(list);\n\
         }",
    );
    let sets = uses_of(&found, "setItems");
    assert_eq!(sets.len(), 3);
    for set in sets {
        let Use::Call(origins) = set else { panic!() };
        assert!(
            matches!(origins.as_slice(), [Origin::Unknown(_)]),
            "{origins:?}"
        );
    }
}

#[test]
fn a_setter_handed_on_is_not_a_call() {
    let found = state_uses(
        Code::Script,
        "const [items, setItems] = pp.state([]);\n\
         load().then(setItems);\n\
         const alias = setItems;",
    );
    assert_eq!(uses_of(&found, "setItems"), [&Use::Other, &Use::Other]);
}

#[test]
fn a_script_reads_fields_off_its_state_once_per_chain() {
    let found = state_uses(
        Code::Script,
        "const [items, setItems] = pp.state([]);\n\
         const first = items[0].owner.name;\n\
         const count = items?.length;\n\
         function shadow(items) { return items.nothing; }\n\
         items[0].open = true;",
    );
    let reads = uses_of(&found, "items");
    let [Use::Read(deep), Use::Read(length), Use::Write] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(fields(deep), ["[]", "owner", "name"]);
    assert_eq!(fields(length), ["length"]);
}

#[test]
fn a_binding_reads_fields_off_the_names_it_does_not_declare() {
    let found = state_uses(
        Code::Expression,
        "items.filter((item) => item.ok).length + user.name",
    );
    assert!(found.states.is_empty());
    let [Use::Read(items)] = uses_of(&found, "items")[..] else {
        panic!();
    };
    assert_eq!(fields(items), ["filter"]);
    let [Use::Read(user)] = uses_of(&found, "user")[..] else {
        panic!();
    };
    assert_eq!(fields(user), ["name"]);
    assert!(uses_of(&found, "item").is_empty());
}

#[test]
fn a_handler_calls_setters_and_writes_fields() {
    let found = state_uses(Code::Handler, "setItems([])");
    assert_eq!(
        uses_of(&found, "setItems"),
        [&Use::Call(vec![Origin::Empty])]
    );
    let found = state_uses(Code::Handler, "row.open = !row.open; pick(setItems)");
    // The assignment writes `row.open`; its right-hand side reads it.
    let row = uses_of(&found, "row");
    let [Use::Write, Use::Read(read)] = row.as_slice() else {
        panic!("{row:?}");
    };
    assert_eq!(fields(read), ["open"]);
    assert_eq!(uses_of(&found, "setItems"), [&Use::Other]);
}

#[test]
fn code_that_does_not_parse_reports_nothing() {
    assert_eq!(
        state_uses(Code::Script, "const [a, b] = ;"),
        StateUses::default()
    );
}

// ----- helpers ---------------------------------------------------------------

const ACT: &str = "
async function act(request) {
    setBusy(true);
    try {
        return await request();
    } catch (error) {
        setFailure(error.message);
        return null;
    } finally {
        setBusy(false);
    }
}
";

/// The rpcs whose results `body` reads, with a helper declared as `helper`.
fn read_through(helper: &str, body: &str) -> Vec<(String, Vec<String>)> {
    let source = format!("{helper}\nasync function load() {{\n{body}\n}}");
    rpc_reads(Code::Script, &source)
        .into_iter()
        .map(|read| {
            let steps = fields(&read.steps).into_iter().map(String::from).collect();
            (read.rpc, steps)
        })
        .collect()
}

#[test]
fn a_result_handed_back_by_a_helper_is_the_rpcs_response() {
    let reads = read_through(
        ACT,
        "const found = await act(() => pp.rpc(\"tables\", { id }));\nfound.rows;",
    );
    assert_eq!(reads, [("tables".to_string(), vec!["rows".to_string()])]);

    // The same through an async callback, a returned call, and a later
    // parameter of an arrow helper.
    for (helper, call) in [
        (ACT, "act(async () => await pp.rpc(\"tables\", {}))"),
        (ACT, "act(() => { return pp.rpc(\"tables\", {}); })"),
        (ACT, "act(function () { return pp.rpc(\"tables\", {}); })"),
        (
            "const run = async (label, request) => (await request()) ?? null;",
            "run(\"load\", () => pp.rpc(\"tables\", {}))",
        ),
        (
            "function run(label, request) { return request(); }",
            "run(\"load\", () => pp.rpc(\"tables\", {}))",
        ),
    ] {
        let reads = read_through(helper, &format!("const found = await {call};\nfound.rows;"));
        assert_eq!(reads.len(), 1, "{helper} / {call}");
        assert_eq!(reads[0].0, "tables");
    }
}

#[test]
fn a_helper_that_returns_anything_else_is_not_followed() {
    let call = "const found = await act(() => pp.rpc(\"tables\", {}));\nfound.rows;";
    for helper in [
        // A fallback of the helper's own.
        "async function act(request) { try { return await request(); } catch { return { rows: [] }; } }",
        // The result changed on the way.
        "async function act(request) { const result = await request(); return result; }",
        "async function act(request) { return normalize(await request()); }",
        "async function act(request) { return (await request()).data; }",
        // Returns nothing of the callback's.
        "async function act(request) { await request(); }",
        // Two callbacks, either of which may be the one returned.
        "async function act(request, other) { if (x) return await other(); return await request(); }",
        // Not one function.
        "let act = async (request) => await request(); act = other;",
        // The parameter is replaced before it is called.
        "async function act(request) { request = wrap(request); return await request(); }",
        // Not a function the script declares.
        "",
    ] {
        assert!(read_through(helper, call).is_empty(), "{helper}");
    }
}

#[test]
fn a_callback_that_does_more_than_return_one_rpc_is_not_followed() {
    for call in [
        "act(() => pp.rpc(\"tables\", {}).then((r) => r.data))",
        "act(async () => { const r = await pp.rpc(\"tables\", {}); return r.data; })",
        "act(async () => { await prepare(); return pp.rpc(\"tables\", {}); })",
        "act(loadTables)",
        "act(() => other(\"tables\"))",
        // The parameter that is returned is given something else.
        "act(1, () => pp.rpc(\"tables\", {}))",
    ] {
        let body = format!(
            "const found = await {call};
found.rows;"
        );
        // Nothing is read off `found`. (A result the callback itself holds
        // is still read there, as in any function.)
        let reads = read_through(ACT, &body);
        assert!(
            reads.iter().all(|(_, steps)| steps != &["rows"]),
            "{call}: {reads:?}"
        );
    }
    // Not awaited: a promise, not the response.
    let reads = read_through(
        ACT,
        "const found = act(() => pp.rpc(\"tables\", {}));
found.rows;",
    );
    assert!(reads.is_empty());
}

#[test]
fn a_return_inside_a_nested_function_is_not_the_helpers() {
    // The inner arrow returns an object; the helper still returns only what
    // its callback returns.
    let helper = "
        async function act(request) {
            const stamp = () => { return { at: Date.now() }; };
            log(stamp());
            return await request();
        }";
    let reads = read_through(
        helper,
        "const found = await act(() => pp.rpc(\"tables\", {}));\nfound.rows;",
    );
    assert_eq!(reads.len(), 1);
}

#[test]
fn a_state_takes_a_result_that_came_back_through_a_helper() {
    let source = format!(
        "{ACT}
        const [tables, setTables] = pp.state([]);
        async function load() {{
            const found = await act(() => pp.rpc(\"tables\", {{}}));
            setTables(found.rows || []);
            setTables(await act(() => pp.rpc(\"tables\", {{}})));
        }}"
    );
    let found = state_uses(Code::Script, &source);
    let sets = uses_of(&found, "setTables");
    let [Use::Call(part), Use::Call(whole)] = sets.as_slice() else {
        panic!("{sets:?}");
    };
    assert_eq!(rpc_origin(&part[0]), ("tables", vec!["rows"]));
    assert_eq!(rpc_origin(&whole[0]), ("tables", vec![]));
}

#[test]
fn every_field_name_code_assigns_is_reported() {
    let found = state_uses(
        Code::Script,
        "items.forEach((item) => { item.open = false; item.count++; });
         delete first.draft;
         const copy = Object.assign({}, first);
         const keys = Object.keys(first);
         map[key] = 1;",
    );
    assert_eq!(found.assigned, ["count", "draft", "open"]);
    assert!(!found.assigns_unnamed);

    for source in [
        "Object.assign(row, extra);",
        "Object.defineProperty(row, key, { value: 1 });",
    ] {
        assert!(state_uses(Code::Script, source).assigns_unnamed, "{source}");
    }
    assert_eq!(
        state_uses(Code::Handler, "row.open = !row.open").assigned,
        ["open"]
    );
}

// ----- derived values --------------------------------------------------------

/// A [`Derived`] written out, so a test reads as the expression it describes.
fn show(value: &Derived) -> String {
    match value {
        Derived::Name { name, steps } => {
            let mut out = name.clone();
            for step in fields(steps) {
                out.push('.');
                out.push_str(step);
            }
            out
        }
        Derived::Call(name) => format!("{name}()"),
        Derived::Empty => "empty".into(),
        Derived::Either(values) => {
            let values: Vec<String> = values.iter().map(show).collect();
            format!("({})", values.join(" | "))
        }
        Derived::SameArray(inner) => format!("same({})", show(inner)),
        Derived::Element(inner) => format!("element({})", show(inner)),
        Derived::Path(base, steps) => format!("{}.{}", show(base), fields(steps).join(".")),
        Derived::Unknown => "unknown".into(),
    }
}

#[test]
fn a_binding_expression_is_followed_while_its_shape_is_certain() {
    for (source, expected) in [
        ("domains", "domains"),
        ("detail?.variables || []", "(detail.variables | empty)"),
        ("(plan ? plan.phases : [])", "(plan.phases | empty)"),
        ("backup.recoveries ?? []", "(backup.recoveries | empty)"),
        (
            "rows.filter((row) => row.live).slice(0, 4)",
            "same(same(rows))",
        ),
        ("rows.toSorted(byName).reverse()", "same(same(rows))"),
        ("rows.find((row) => row.id === id)", "element(rows)"),
        ("rows[0].tags", "rows.[].tags"),
        ("shown()", "shown()"),
        ("preview().ticks", "preview().ticks"),
        ("(rows || []).filter(Boolean)", "same((rows | empty))"),
    ] {
        assert_eq!(show(&derive(source)), expected, "{source}");
    }
}

#[test]
fn anything_that_may_change_the_shape_is_unknown() {
    for source in [
        "rows.map((row) => row.name)",
        "[...rows, extra]",
        "Object.values(drafts)",
        "sizesFor(template)",
        "rows.concat(more)",
        "rows.flat()",
        "ready && rows",
        "ready ? rows : rows.map(label)",
        "rows.length",
        "[first]",
        "\"text\".split(\",\")",
        "not javascript (",
    ] {
        let value = derive(source);
        // `rows.length` is a path the shape then answers for; the rest are
        // not followed at all.
        if source == "rows.length" {
            assert_eq!(show(&value), "rows.length");
        } else {
            assert_eq!(value, Derived::Unknown, "{source}");
        }
    }
}

/// The followed top-level bindings of a script, as `name = value`.
fn followed(script: &str) -> Vec<String> {
    state_uses(Code::Script, script)
        .derived
        .iter()
        .map(|binding| {
            let call = if binding.function { "()" } else { "" };
            format!("{}{call} = {}", binding.name, show(&binding.value))
        })
        .collect()
}

#[test]
fn a_script_function_returns_what_its_returns_come_from() {
    let script = "
        const [domains, setDomains] = pp.state([]);
        const [query, setQuery] = pp.state(\"\");
        const [detail, setDetail] = pp.state(null);
        function shown() {
            const all = domains || [];
            if (!query) return all;
            return all.filter((domain) => domain.name.includes(query));
        }
        const live = () => domains.filter((domain) => domain.live);
        const first = domains[0];
        function variables() { return detail ? detail.variables : []; }
        function both() { return shown().slice(0, 3); }
        function selected() { return domains.find((domain) => domain.id === query) || null; }
    ";
    assert_eq!(
        followed(script),
        [
            "shown() = ((domains | empty) | same((domains | empty)))",
            "live() = same(domains)",
            "first = domains.[]",
            "variables() = (detail.variables | empty)",
            "both() = same(shown())",
            "selected() = (element(domains) | empty)",
        ]
    );
}

#[test]
fn a_script_function_that_builds_its_value_is_not_followed() {
    let script = "
        const [domains, setDomains] = pp.state([]);
        const LIMIT = 4;
        const labels = domains.map((domain) => domain.name);
        function rows() { return domains.map((domain) => ({ ...domain, open: false })); }
        function figures() { return [{ label: \"Total\", value: domains.length }]; }
        function maybe(flag) { if (flag) return domains; return figures(); }
        function nothing() { domains.forEach(log); }
        async function load() { return domains; }
        function* each() { yield* domains; }
        function shadowed(domains) { return domains; }
        function local() { let list = domains; list = other; return list; }
        function inner() { const pick = () => { return 1; }; return domains; }
        let swapped = function () { return domains; };
    ";
    // `figures()` is a call the macro then finds nothing behind; `inner`
    // returns the state, and its nested function's `return` is not its own.
    assert_eq!(
        followed(script),
        ["maybe() = (domains | figures())", "inner() = domains",]
    );
}

#[test]
fn a_function_that_is_assigned_again_is_not_followed() {
    let script = "
        const [domains, setDomains] = pp.state([]);
        function shown() { return domains; }
        shown = other;
    ";
    assert!(followed(script).is_empty());
}

// ----- .then -----------------------------------------------------------------

#[test]
fn a_then_callback_is_handed_the_response() {
    let reads = read_through(
        "",
        "pp.rpc(\"tables\", {}).then((found) => { show(found.rows); });\n\
         pp.rpc(\"tables\", {}).then(function (found) { return found.total; }, fail);",
    );
    assert_eq!(
        reads,
        [
            ("tables".to_string(), vec!["rows".to_string()]),
            ("tables".to_string(), vec!["total".to_string()]),
        ]
    );
    // Through a helper of the script, too.
    let reads = read_through(
        "function run(request) { return request(); }",
        "run(() => pp.rpc(\"tables\", {})).then((found) => found.rows);",
    );
    assert_eq!(reads.len(), 1);
}

#[test]
fn only_the_first_then_on_the_call_is_handed_the_response() {
    for body in [
        // The second `.then` is handed what the first returned.
        "pp.rpc(\"tables\", {}).then(pick).then((found) => found.rows);",
        // `.catch` and `.finally` are handed something else.
        "pp.rpc(\"tables\", {}).catch((found) => found.rows);",
        "pp.rpc(\"tables\", {}).finally((found) => found.rows);",
        // Not a promise of a response.
        "load().then((found) => found.rows);",
        "pp.rpc(name, {}).then((found) => found.rows);",
        // The parameter is not one plain name, or is assigned again.
        "pp.rpc(\"tables\", {}).then(({ rows }) => rows.length);",
        "pp.rpc(\"tables\", {}).then((found) => { found = other; return found.rows; });",
        // The failure callback's parameter is the error.
        "pp.rpc(\"tables\", {}).then(done, (found) => found.rows);",
    ] {
        assert!(read_through("", body).is_empty(), "{body}");
    }
}

#[test]
fn a_setter_handed_to_then_is_called_with_the_response() {
    let found = state_uses(
        Code::Script,
        "const [tables, setTables] = pp.state([]);\n\
         const [edges, setEdges] = pp.state([]);\n\
         const [other, setOther] = pp.state([]);\n\
         pp.rpc(\"tables\", {}).then(setTables).catch(() => setTables([]));\n\
         pp.rpc(\"edges\", {}).then((result) => setEdges(result || []));\n\
         load().then(setOther);\n\
         pp.rpc(\"tables\", {}).then(pick).then(setOther);\n\
         pp.rpc(\"tables\", {}).catch(setOther);",
    );
    let tables = uses_of(&found, "setTables");
    let [Use::Call(given), Use::Call(cleared)] = tables.as_slice() else {
        panic!("{tables:?}");
    };
    assert_eq!(rpc_origin(&given[0]), ("tables", vec![]));
    assert_eq!(cleared.as_slice(), [Origin::Empty]);

    let [Use::Call(edges)] = uses_of(&found, "setEdges")[..] else {
        panic!();
    };
    assert_eq!(rpc_origin(&edges[0]), ("edges", vec![]));
    assert_eq!(edges[1], Origin::Empty);

    // Handed on any other way, a setter is still only handed on.
    assert_eq!(
        uses_of(&found, "setOther"),
        [&Use::Other, &Use::Other, &Use::Other]
    );
}

// ----- payloads built elsewhere --------------------------------------------------

/// The keys of the one call in `script`, followed ones marked `~`, and
/// whether more may be sent.
fn sent(script: &str) -> Option<(Vec<String>, bool)> {
    let calls = rpc_calls(Code::Script, script);
    assert_eq!(calls.len(), 1, "{script}");
    match &calls[0].payload {
        Payload::Object { keys, open, .. } => Some((
            keys.iter()
                .map(|key| {
                    let mark = if key.followed { "~" } else { "" };
                    format!("{mark}{}", key.name)
                })
                .collect(),
            *open,
        )),
        _ => None,
    }
}

#[test]
fn a_payload_built_in_the_script_is_followed_to_its_literal() {
    let ids = "function ids() { return { project_id: projectId, service_id: serviceId }; }\n";
    for (call, expected) in [
        (
            "pp.rpc(\"load\", ids());",
            vec!["~project_id", "~service_id"],
        ),
        (
            "pp.rpc(\"load\", { ...ids(), path });",
            vec!["~project_id", "~service_id", "path"],
        ),
        // The value written last is the one sent.
        (
            "pp.rpc(\"load\", { project_id: 1, ...ids() });",
            vec!["~project_id", "~service_id"],
        ),
        (
            "pp.rpc(\"load\", { ...ids(), project_id: 1 });",
            vec!["~service_id", "project_id"],
        ),
        (
            "const filter = { status: \"open\" };\npp.rpc(\"load\", { ...ids(), ...filter });",
            vec!["~project_id", "~service_id", "~status"],
        ),
        (
            "const params = { ...ids(), page: 2 };\nawait pp.rpc(\"load\", params);",
            vec!["~project_id", "~service_id", "~page"],
        ),
        (
            "const scope = () => ({ ...ids(), all: true });\npp.rpc(\"load\", scope());",
            vec!["~project_id", "~service_id", "~all"],
        ),
    ] {
        let script = format!("{ids}async function go() {{\n{call}\n}}");
        let (keys, open) = sent(&script).unwrap_or_else(|| panic!("not followed: {call}"));
        assert_eq!(keys, expected, "{call}");
        assert!(!open, "{call}");
    }
}

#[test]
fn a_payload_that_may_hold_anything_else_is_not_followed() {
    for script in [
        // Changed, handed on, or aliased after it was written.
        "const params = { id }; params.extra = 1; pp.rpc(\"load\", params);",
        "const params = { id }; fill(params); pp.rpc(\"load\", params);",
        "const params = { id }; const other = params; pp.rpc(\"load\", params);",
        // Not a `const`, or not an object literal.
        "let params = { id }; pp.rpc(\"load\", params);",
        "const params = build(); pp.rpc(\"load\", params);",
        "const params = Object.fromEntries(new FormData(form)); pp.rpc(\"load\", params);",
        // A function called with arguments, with two returns, or async.
        "function ids(extra) { return { id }; } pp.rpc(\"load\", ids(1));",
        "function ids() { if (all) return {}; return { id }; } pp.rpc(\"load\", ids());",
        "async function ids() { return { id }; } pp.rpc(\"load\", ids());",
        "function ids() { return { id }; } ids = other; pp.rpc(\"load\", ids());",
        // Not this code's.
        "pp.rpc(\"load\", elsewhere());",
        "pp.rpc(\"load\", pp.props.params);",
    ] {
        assert_eq!(sent(script), None, "{script}");
    }
    // A spread that is not followed leaves the object open, as before.
    for script in [
        "pp.rpc(\"load\", { ...elsewhere(), id });",
        "const params = { a }; params.b = 1; pp.rpc(\"load\", { ...params, id });",
    ] {
        let (keys, open) = sent(script).unwrap();
        assert_eq!(keys, ["id"], "{script}");
        assert!(open, "{script}");
    }
}

#[test]
fn reading_a_field_of_a_payload_does_not_stop_it_being_followed() {
    let (keys, open) =
        sent("const params = { id, page: 1 }; log(params.page); pp.rpc(\"load\", params);")
            .unwrap();
    assert_eq!(keys, ["~id", "~page"]);
    assert!(!open);
}

// ----- server values that are not state -------------------------------------------

#[test]
fn a_script_names_its_server_constants_and_follows_their_uses() {
    let found = state_uses(
        Code::Script,
        "const [open, setOpen] = pp.state(false);\n\
         const projects = __pp_server[0];\n\
         const limit = __pp_server[1];\n\
         let draft = __pp_server[2];\n\
         const picked = __pp_server[index];\n\
         function first() { const inner = __pp_server[3]; return projects[0].name; }\n\
         function shown() { return projects.filter((project) => project.live); }",
    );
    // Only a top-level `const` that is exactly one server value.
    assert_eq!(
        found.server,
        [("projects".to_string(), 0), ("limit".to_string(), 1)]
    );
    let reads = uses_of(&found, "projects");
    let [Use::Read(name), Use::Read(filter)] = reads.as_slice() else {
        panic!("{reads:?}");
    };
    assert_eq!(fields(name), ["[]", "name"]);
    assert_eq!(fields(filter), ["filter"]);
    // And a list that comes from one is followed like one from a state.
    assert_eq!(
        followed(
            "const projects = __pp_server[0];\nfunction shown() { return projects.filter(live); }"
        ),
        ["shown() = same(projects)"]
    );
}

// ----- props -------------------------------------------------------------------

#[test]
fn a_read_of_a_prop_names_it() {
    let found = prop_reads(
        Code::Script,
        "const open = pp.props.open;\n\
         function label() { return (pp.props?.current ?? \"\") + pp.props.name.trim(); }",
    );
    let names: Vec<&str> = found.names.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["open", "current", "name"]);
    assert!(!found.whole);
    // The span is the name's.
    let source = "pp.props.open";
    let (_, span) = &prop_reads(Code::Expression, source).names[0];
    assert_eq!(&source[span.clone()], "open");
    assert_eq!(
        prop_reads(Code::Handler, "pp.props.onPick(event)").names[0].0,
        "onPick"
    );
}

#[test]
fn props_used_whole_may_read_any_prop() {
    for source in [
        "const { open } = pp.props;",
        "show(pp.props);",
        "const value = pp.props[key];",
        "Object.keys(pp.props).length;",
    ] {
        assert!(prop_reads(Code::Script, source).whole, "{source}");
    }
    // Another object's `props`, and code that does not parse, read nothing.
    assert_eq!(
        prop_reads(Code::Script, "other.props.open; props.open;"),
        PropReads::default()
    );
    assert_eq!(
        prop_reads(Code::Script, "pp.props.open = ;"),
        PropReads::default()
    );
}

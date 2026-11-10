//! The shape a block's state holds, worked out from what is put into it.
//!
//! A shape is what a refusal stands on, so each case that gives a state one
//! is paired with the ordinary code that must leave it without, and each
//! refusal with the ordinary reads it must not catch.

use super::*;
use crate::parse::parse;
use proc_macro2::TokenStream;
use std::str::FromStr;

const FILE: &str = "src/app/domains/page.rs";

fn shapes() -> Shapes {
    let text = "\
R\tsrc/app/domains/page.rs\tlist_domains\tDomain[]
R\tsrc/app/domains/page.rs\tadd_domain\tDomain
R\tsrc/app/domains/page.rs\trename_domain\tDomain
R\tsrc/app/domains/page.rs\toverview\tOverview
R\tsrc/app/domains/page.rs\tanything\tunknown
T\tsrc/app/domains/page.rs\tDomain\t{ id: number; name: string; records: DnsRecord[]; owner: Owner | null }
T\tsrc/app/domains/page.rs\tDnsRecord\t{ kind: string; value: string }
T\tsrc/app/domains/page.rs\tOwner\t{ email: string }
T\tsrc/app/domains/page.rs\tOverview\t{ domains: Domain[]; total: number }
";
    Shapes::parse(text, FILE)
}

/// What is found about a block with this markup and this script.
fn found(markup: &str, script: &str) -> Vec<Finding> {
    let src = format!("<div>{markup}<script>r##\"{script}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let parsed = match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    };
    analyze(&parsed.client_code, &shapes())
}

fn state<'a>(findings: &'a [Finding], state: &str) -> &'a Result<String, (&'static str, String)> {
    findings
        .iter()
        .find_map(|finding| match finding {
            Finding::State { name, shape } if name == state => Some(shape),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no state `{state}` in {findings:?}"))
}

fn shape_of(findings: &[Finding], name: &str) -> String {
    match state(findings, name) {
        Ok(shape) => shape.to_string(),
        Err((reason, detail)) => panic!("`{name}` has no shape: {reason} ({detail})"),
    }
}

fn reason(findings: &[Finding], name: &str) -> &'static str {
    match state(findings, name) {
        Ok(shape) => panic!("`{name}` has the shape {shape}"),
        Err((reason, _)) => reason,
    }
}

/// Each read found, as `place expression outcome`.
fn reads(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .filter_map(|finding| match finding {
            Finding::Read {
                place,
                expression,
                outcome,
                ..
            } => Some(format!(
                "{place} {expression} {}",
                match outcome {
                    Outcome::Known => "known",
                    Outcome::Open => "open",
                    Outcome::Missing { .. } => "missing",
                }
            )),
            _ => None,
        })
        .collect()
}

const LOAD: &str = "
    const [domains, setDomains] = pp.state([]);
    async function load() {
        setDomains(await pp.rpc(\"list_domains\", {}));
    }
";

#[test]
fn a_state_given_one_rpcs_response_holds_that_response() {
    let findings = found("", LOAD);
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");
}

#[test]
fn a_named_result_or_a_part_of_one_gives_a_state_its_shape() {
    let findings = found(
        "",
        "
        const [domains, setDomains] = pp.state([]);
        const [total, setTotal] = pp.state(0);
        const [first, setFirst] = pp.state(null);
        async function load() {
            const page = await pp.rpc(\"overview\", {});
            setDomains(page.domains ?? []);
            setFirst(page.domains[0]);
            setDomains([]);
        }
        ",
    );
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");
    assert_eq!(shape_of(&findings, "first"), "Domain");
    assert_eq!(reason(&findings, "total"), "never given an rpc result");
}

#[test]
fn two_rpcs_that_return_the_same_shape_agree() {
    let findings = found(
        "",
        "
        const [domain, setDomain] = pp.state(null);
        async function save(creating) {
            const next = creating
                ? await pp.rpc(\"add_domain\", {})
                : await pp.rpc(\"rename_domain\", {});
            setDomain(next);
        }
        ",
    );
    assert_eq!(shape_of(&findings, "domain"), "Domain");
}

#[test]
fn anything_that_leaves_room_for_doubt_leaves_the_state_without_a_shape() {
    let cases = [
        (
            "setDomains([...domains, await pp.rpc(\"add_domain\", {})]);",
            "given something else",
        ),
        (
            "setDomains((current) => current.slice(1));",
            "given something else",
        ),
        (
            "setDomains(await pp.rpc(\"add_domain\", {}));",
            "given two different shapes",
        ),
        ("load().then(setDomains);", "setter handed to other code"),
        ("domains[0].open = true;", "a field is written in place"),
        (
            "setDomains(await pp.rpc(\"anything\", {}));",
            "given two different shapes",
        ),
    ];
    for (line, expected) in cases {
        let findings = found("", &format!("{LOAD}\nasync function other() {{ {line} }}"));
        assert_eq!(reason(&findings, "domains"), expected, "{line}");
        assert!(reads(&findings).is_empty(), "{line}");
    }

    // A server value read some other way than as one whole value is not
    // followed.
    let findings = found(
        "",
        "const [domains, setDomains] = pp.state(__pp_server[index]);
         async function load() { setDomains(await pp.rpc(\"list_domains\", {})); }",
    );
    assert_eq!(reason(&findings, "domains"), "given something else");
}

#[test]
fn markup_that_sets_or_writes_a_state_counts_too() {
    // A handler clearing the list contradicts nothing.
    let findings = found("<button onclick={setDomains([])}>\"Clear\"</button>", LOAD);
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");

    let findings = found(
        "<button onclick={setDomains(picked)}>\"Use\"</button>",
        LOAD,
    );
    assert_eq!(reason(&findings, "domains"), "given something else");

    let findings = found("<Picker onpick={setDomains} />", LOAD);
    assert_eq!(reason(&findings, "domains"), "setter handed to other code");

    let findings = found(
        "<template pp-for=\"domain in domains\">
            <li key={domain.id} onclick={domain.open = !domain.open}>{domain.name}</li>
        </template>",
        LOAD,
    );
    assert_eq!(
        reason(&findings, "domains"),
        "a row's field is written in place"
    );
}

#[test]
fn reads_in_the_script_and_the_markup_are_followed_through_the_shape() {
    let findings = found(
        "<p hidden={domains.length === 0}>{domains[0].name}</p>
         <p>{domains[0].nmae}</p>
         <p>{domains[0].owner.email}</p>",
        &format!("{LOAD}\nconst first = domains[0]?.records[0].kind;"),
    );
    assert_eq!(
        reads(&findings),
        [
            "script domains[].records[].kind known",
            "binding domains.length open",
            "binding domains[].name known",
            "binding domains[].nmae missing",
            "binding domains[].owner.email known",
        ]
    );
}

#[test]
fn a_loop_row_has_the_shape_of_an_element() {
    let findings = found(
        "<template pp-for=\"(domain, index) in domains\">
            <li key={domain.id}>
                {index.nothing}
                {domain.nmae}
                <template pp-for=\"record in domain.records\">
                    <span key={record.kind}>{record.value}{record.vlaue}{domain.name}</span>
                </template>
            </li>
        </template>",
        LOAD,
    );
    assert_eq!(
        reads(&findings),
        [
            "row domain.id known",
            "row domain.nmae missing",
            "row domain.records known",
            "row record.kind known",
            "row record.value known",
            "row record.vlaue missing",
            "row domain.name known",
        ]
    );
    let loops: Vec<_> = findings
        .iter()
        .filter_map(|finding| match finding {
            Finding::Loop {
                directive, typed, ..
            } => Some((directive.as_str(), *typed)),
            _ => None,
        })
        .collect();
    assert_eq!(
        loops,
        [
            ("(domain, index) in domains", true),
            ("record in domain.records", true)
        ]
    );
}

#[test]
fn a_loop_variable_hides_a_state_of_the_same_name() {
    // Inside the loop `domains` is a row of `groups`, which has no shape.
    let findings = found(
        "<template pp-for=\"domains in groups\">
            <li key={domains.key}>{domains.anything}</li>
        </template>
        <template pp-for=\"domain in domains.map((d) => d.owner)\">
            <li key={domain.id}>{domain.anything}</li>
        </template>",
        LOAD,
    );
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");
    // The mapped loop reads `domains.map`; its rows have no shape.
    assert_eq!(reads(&findings), ["binding domains.map open"]);
}

#[test]
fn fragment_children_of_a_component_read_another_scope() {
    let findings = found("<Card><>{domains.anything}</></Card>", LOAD);
    assert!(reads(&findings).is_empty());
    let findings = found("<Card><p>{domains[0].nmae}</p></Card>", LOAD);
    assert_eq!(reads(&findings), ["binding domains[].nmae missing"]);
}

#[test]
fn a_block_without_one_script_has_no_states() {
    let tokens = TokenStream::from_str("<p>{domains[0].nmae}</p>").unwrap();
    let parsed = parse(tokens).ok().unwrap();
    assert!(analyze(&parsed.client_code, &shapes()).is_empty());
}

// ----- refusals --------------------------------------------------------------

/// What a block with this markup and this script is refused with.
fn refused(markup: &str, script: &str) -> Option<String> {
    let src = format!("<div>{markup}<script>r##\"{script}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let parsed = match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    };
    check_against(&parsed.client_code, &shapes(), None)
        .err()
        .map(|error| error.msg)
}

#[test]
fn a_binding_that_reads_a_field_the_state_does_not_hold_is_refused() {
    let message = refused("<p>{domains[0].nmae}</p>", LOAD).expect("a refusal");
    assert!(message.contains("RAHTI_STATE_FIELD"), "{message}");
    assert!(
        message.contains("`domains` has no field `nmae`"),
        "{message}"
    );
    assert!(
        message.contains("the state `domains` only ever holds what `list_domains` answers"),
        "{message}"
    );
    assert!(
        message.contains("Its fields are `id`, `name`, `owner`, `records`"),
        "{message}"
    );

    // The fields it holds, through `| null` and into an element, and what an
    // array answers to, are read without a word.
    assert_eq!(
        refused(
            "<p hidden={domains.length === 0}>{domains[0].name}</p>
             <p>{domains[0]?.owner?.email}{domains.map((d) => d.anything).join()}</p>",
            LOAD
        ),
        None
    );
}

#[test]
fn a_row_that_reads_a_field_its_state_does_not_hold_is_refused() {
    let message = refused(
        "<template pp-for=\"domain in domains\">
            <li key={domain.id}>
                <template pp-for=\"record in domain.records\">
                    <span key={record.kind}>{record.vlaue}</span>
                </template>
            </li>
        </template>",
        LOAD,
    )
    .expect("a refusal");
    assert!(
        message.contains("`record` has no field `vlaue`"),
        "{message}"
    );
    assert!(
        message.contains("Its fields are `kind`, `value`"),
        "{message}"
    );

    assert_eq!(
        refused(
            "<template pp-for=\"(domain, index) in domains\">
                <li key={domain.id} class={index.anything}>
                    {domain.name}
                    <template pp-for=\"record in domain.records\">
                        <span key={record.kind}>{record.value}</span>
                    </template>
                </li>
            </template>",
            LOAD
        ),
        None
    );
}

#[test]
fn a_script_that_reads_a_field_its_state_does_not_hold_is_refused() {
    let message = refused(
        "",
        &format!("{LOAD}\nfunction first() {{ return domains[0].owner.emial; }}"),
    )
    .expect("a refusal");
    assert!(
        message.contains("`domains.owner` has no field `emial`"),
        "{message}"
    );
    assert_eq!(
        refused(
            "",
            &format!("{LOAD}\nfunction first() {{ return domains[0].owner.email; }}")
        ),
        None
    );
}

#[test]
fn a_handler_that_reads_a_field_its_state_does_not_hold_is_refused() {
    let message = refused(
        "<template pp-for=\"domain in domains\">
            <button key={domain.id} onclick={open(domain.idd)}>\"Open\"</button>
        </template>",
        LOAD,
    )
    .expect("a refusal");
    assert!(message.contains("`domain` has no field `idd`"), "{message}");
}

#[test]
fn a_state_without_a_shape_is_read_freely() {
    // Each of these leaves `domains` holding something this cannot name.
    for line in [
        "setDomains((await pp.rpc(\"list_domains\", {})).map((d) => ({ ...d, open: false })));",
        "setDomains((current) => current.slice(1));",
        "run(setDomains);",
        "setDomains(await pp.rpc(\"add_domain\", {}));",
    ] {
        assert_eq!(
            refused(
                "<template pp-for=\"domain in domains\">
                    <li key={domain.id}>{domain.open}{domains[0].anything}</li>
                </template>",
                &format!("{LOAD}\nasync function other() {{ {line} }}"),
            ),
            None,
            "{line}"
        );
    }
}

#[test]
fn a_field_the_block_assigns_anywhere_is_never_refused() {
    // The rows are given `open` in place before they are stored, through a
    // callback's parameter this cannot follow.
    let script = "
        const [domains, setDomains] = pp.state([]);
        async function load() {
            const list = await pp.rpc(\"list_domains\", {});
            list.forEach((domain) => { domain.open = false; });
            setDomains(list);
        }
    ";
    let markup = "<template pp-for=\"domain in domains\">
        <li key={domain.id}>{domain.open}</li>
    </template>";
    assert_eq!(refused(markup, script), None);
    // A field nothing assigns is still refused in that block.
    assert!(refused("<p>{domains[0].nmae}</p>", script).is_some());

    // Fields given by name this cannot read: nothing is refused.
    assert_eq!(
        refused(
            "<p>{domains[0].anything}</p>",
            &format!("{LOAD}\nfunction mark(row, extra) {{ Object.assign(row, extra); }}")
        ),
        None
    );
}

#[test]
fn names_that_are_not_the_blocks_state_are_left_alone() {
    // A loop variable that hides the state, a callback's parameter of the
    // same name, a row of something that is not state, and the fragment
    // children of a component, which read that component's scope.
    assert_eq!(
        refused(
            "<template pp-for=\"domains in groups\">
                <li key={domains.key}>{domains.anything}</li>
            </template>
            <p>{groups.map((domains) => domains.anything)}</p>
            <template pp-for=\"row in pp.props.rows\">
                <li key={row.id}>{row.anything}</li>
            </template>
            <Card><>{domains.anything}</></Card>",
            LOAD
        ),
        None
    );
}

// ----- a state that starts from a server value ----------------------------------

const SEEDED: &str = "
    const [filter, setFilter] = pp.state(__pp_server[0]);
    const [domains, setDomains] = pp.state(__pp_server[1]);
    const [detail, setDetail] = pp.state(__pp_server[2]);
    const [draft, setDraft] = pp.state(__pp_server[3]);
    async function load() {
        setDomains(await pp.rpc(\"list_domains\", {}));
        const page = await pp.rpc(\"overview\", {});
        setDetail(page.domains[0]);
        setFilter(null);
    }
";

/// The questions a block with this markup and this script leaves the
/// compiler, in a crate whose structs have shapes.
fn asked(markup: &str, script: &str) -> Vec<SeedProbe> {
    let src = format!("<div>{markup}<script>r##\"{script}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let parsed = parse(tokens).ok().expect("the test source parses");
    let crate_wide = crate_shapes_file();
    match check_against(&parsed.client_code, &shapes(), Some(crate_wide.fields())) {
        Ok(probes) => probes,
        Err(error) => panic!("refused outright: {}", error.msg),
    }
}

#[test]
fn a_seeded_state_holds_its_server_value_and_what_it_is_then_given() {
    let findings = found("", SEEDED);
    // Given a response, whole or in part: the response's shape.
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");
    assert_eq!(shape_of(&findings, "detail"), "Domain");
    // Given nothing else, or only an empty value: the server value itself.
    assert_eq!(
        shape_of(&findings, "filter"),
        "the server value it starts from"
    );
    assert_eq!(
        shape_of(&findings, "draft"),
        "the server value it starts from"
    );

    // Given anything else, it holds something this cannot name.
    let findings = found(
        "<input oninput={setDraft(event.currentTarget.value)} />",
        SEEDED,
    );
    assert_eq!(reason(&findings, "draft"), "never given an rpc result");
}

#[test]
fn a_field_the_response_lacks_is_asked_of_the_server_value() {
    // After a reload the state holds the response, which has no `nmae`.
    // Before it, the state holds the server value, which the macro cannot
    // see: so the compiler is asked, about that one step.
    let probes = asked(
        "<template pp-for=\"domain in domains\">
            <li key={domain.id}>{domain.owner.emial}</li>
        </template>",
        SEEDED,
    );
    let [probe] = probes.as_slice() else {
        panic!("{} questions", probes.len());
    };
    assert_eq!(probe.index, 1);
    let tokens = probe.tokens.to_string();
    assert!(
        tokens.contains(
            "rahti[RAHTI_STATE_FIELD]: `domain.owner` has no field `emial`: the state `domains` \
             holds the server value it starts from or what `list_domains` answers, and neither \
             has such a field"
        ),
        "{tokens}"
    );
    // `owner` is not in question: the response has it.
    assert!(!tokens.contains("has no field `owner`"), "{tokens}");
    // A brace in the advice is doubled, as the attribute requires.
    assert!(tokens.contains("({{ ...row, open: false }})"), "{tokens}");
}

#[test]
fn a_field_the_response_has_asks_nothing() {
    assert!(
        asked(
            "<p hidden={domains.length === 0}>{domains[0].name}{detail.name}</p>
             <template pp-for=\"domain in domains\">
                <li key={domain.id}>{domain.owner?.email}</li>
            </template>",
            SEEDED
        )
        .is_empty()
    );
    // And a state that is not seeded is still refused outright.
    assert!(refused("<p>{domains[0].nmae}</p>", LOAD).is_some());
}

#[test]
fn a_state_that_holds_only_its_server_value_is_read_as_that_value() {
    let probes = asked("<p>{filter.status}{draft.title.length}</p>", SEEDED);
    let questions: Vec<String> = probes
        .iter()
        .map(|probe| probe.tokens.to_string())
        .collect();
    assert_eq!(probes[0].index, 0);
    assert!(
        questions[0].contains(
            "rahti[RAHTI_STATE_FIELD]: `filter` has no field `status`: the state `filter` only \
             ever holds the server value it starts from, whose Rust struct has no such field"
        ),
        "{questions:?}"
    );
    // Every field step is in question, as for a server constant.
    assert!(
        questions
            .iter()
            .any(|tokens| tokens.contains("`draft` has no field `title`")),
        "{questions:?}"
    );
    assert!(probes.iter().any(|probe| probe.index == 3));
}

// ----- lists that come from a state ------------------------------------------

const DERIVED: &str = "
    const [domains, setDomains] = pp.state([]);
    const [detail, setDetail] = pp.state(null);
    const [query, setQuery] = pp.state(\"\");
    async function load() {
        setDomains(await pp.rpc(\"list_domains\", {}));
        setDetail(await pp.rpc(\"add_domain\", {}));
    }
    function shown() {
        const all = domains || [];
        if (!query) return all;
        return all.filter((domain) => domain.name.includes(query));
    }
    const sorted = () => shown().toSorted(byName);
    const firstOwner = domains[0]?.owner;
    function labels() { return domains.map((domain) => ({ label: domain.name })); }
    function records(of) { return of.records; }
";

/// A row template over `collection`, reading `field` off each row.
fn row_of(collection: &str, field: &str) -> String {
    format!(
        "<template pp-for=\"row in {collection}\"><li key={{row.{field}}}>\"x\"</li></template>"
    )
}

#[test]
fn a_row_of_a_list_that_comes_from_a_state_has_that_states_rows() {
    for collection in [
        "domains || []",
        "(domains ?? [])",
        "domains.filter((domain) => domain.id > 1)",
        "domains.slice(0, 4).reverse()",
        "(query ? domains.filter(matches) : domains)",
        "shown()",
        "sorted()",
        "shown().slice(0, 3)",
    ] {
        let message = refused(&row_of(collection, "nmae"), DERIVED)
            .unwrap_or_else(|| panic!("`{collection}` rows were not followed"));
        assert!(message.contains("`row` has no field `nmae`"), "{message}");
        assert!(message.contains("the state `domains`"), "{message}");
        // And the fields the rows have are read without a word.
        assert_eq!(
            refused(&row_of(collection, "name"), DERIVED),
            None,
            "{collection}"
        );
    }

    // A part of a state, with a fallback, and a part of a row.
    let records = refused(&row_of("detail?.records || []", "vlaue"), DERIVED).expect("a refusal");
    assert!(
        records.contains("Its fields are `kind`, `value`"),
        "{records}"
    );
    assert_eq!(
        refused(
            "<template pp-for=\"domain in shown()\">
                <li key={domain.id}>
                    <template pp-for=\"record in domain.records || []\">
                        <span key={record.kind}>{record.value}</span>
                    </template>
                </li>
            </template>",
            DERIVED
        ),
        None
    );
}

#[test]
fn a_list_built_some_other_way_has_rows_this_cannot_name() {
    for collection in [
        // Built, not picked.
        "labels()",
        "domains.map((domain) => domain.owner)",
        "[...domains, extra]",
        "Object.values(drafts)",
        // A function given arguments, and one this script does not declare.
        "records(detail)",
        "elsewhere()",
        // Either of two different things.
        "(query ? domains : labels())",
        "(query ? domains : detail.records)",
        // A name that is not a list of this script's.
        "pp.props.rows || []",
    ] {
        assert_eq!(
            refused(&row_of(collection, "anything"), DERIVED),
            None,
            "{collection}"
        );
    }
}

#[test]
fn a_script_const_that_comes_from_a_state_is_followed() {
    let message = refused("<p>{firstOwner.emial}</p>", DERIVED);
    // A `const` is not a state: its reads in a binding are not followed, only
    // the rows of a loop over it.
    assert_eq!(message, None);
    let script = format!("{DERIVED}\nconst recent = domains.slice(0, 5);");
    assert!(refused(&row_of("recent", "nmae"), &script).is_some());
    assert_eq!(refused(&row_of("recent", "name"), &script), None);
}

#[test]
fn a_loop_variable_or_a_state_hides_a_function_of_the_same_name() {
    // `shown` here is the outer loop's row, not the script's function.
    assert_eq!(
        refused(
            "<template pp-for=\"shown in groups\">
                <template pp-for=\"row in shown()\"><li key={row.anything}>\"x\"</li></template>
            </template>",
            DERIVED
        ),
        None
    );
}

#[test]
fn a_row_written_through_a_derived_list_leaves_its_state_without_a_shape() {
    let findings = found(
        "<template pp-for=\"domain in shown()\">
            <li key={domain.id} onclick={domain.open = !domain.open}>{domain.name}</li>
        </template>",
        DERIVED,
    );
    assert_eq!(
        reason(&findings, "domains"),
        "a row's field is written in place"
    );
}

// ----- a setter handed to `.then` ---------------------------------------------

#[test]
fn a_state_set_by_then_holds_the_response() {
    let script = "
        const [domains, setDomains] = pp.state([]);
        const [detail, setDetail] = pp.state(null);
        function load() {
            pp.rpc(\"list_domains\", {}).then(setDomains).catch(() => setDomains([]));
            pp.rpc(\"add_domain\", {}).then((made) => setDetail(made));
        }
    ";
    let findings = found("", script);
    assert_eq!(shape_of(&findings, "domains"), "Domain[]");
    assert_eq!(shape_of(&findings, "detail"), "Domain");
    assert!(refused(&row_of("domains", "nmae"), script).is_some());
    assert_eq!(refused(&row_of("domains", "name"), script), None);

    // A second `.then` is handed what the first returned, which is unknown.
    let findings = found(
        "",
        "const [domains, setDomains] = pp.state([]);
         pp.rpc(\"list_domains\", {}).then(pick).then(setDomains);",
    );
    assert_eq!(reason(&findings, "domains"), "setter handed to other code");
}

// ----- server values that are not state ---------------------------------------

const SERVER: &str = "
    const projects = __pp_server[0];
    const limit = __pp_server[1];
    function live() { return projects.filter((project) => project.live); }
    const first = projects[0].name;
";

/// The shapes a build writes for a crate with `Project`, `Database`, `Owner`
/// and a struct with more fields than a name can spell.
fn crate_shapes_file() -> Shapes {
    let text = "\
G\t*\tcrate::services::projects::Project\tProject
G\t*\tcrate::routes::p_projects::Database\tDatabase
G\t*\tcrate::models::wide::Model\tWide
T\t*\tProject\t{ id: number; name: string; databases: Database[]; owner: Owner | null; parent: Project | null; \"content-type\": string }
T\t*\tDatabase\t{ id: number; engine: string }
T\t*\tOwner\t{ email: string }
T\t*\tWide\t{ first_long_field_name: string; second_long_field_name: string; third_long_field_name: string; fourth_long_field_name: string; fifth: string; sixth: string }
T\t*\tKind\t\"a\" | \"b\"
";
    Shapes::parse(text, crate::rpc_shapes::CRATE_WIDE)
}

/// The questions a block with this markup and the [`SERVER`] script leaves
/// the compiler, with the crate's shapes present or not.
fn server_asked(markup: &str, with_shapes: bool) -> Vec<String> {
    let src = format!("<div>{markup}<script>r##\"{SERVER}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let parsed = parse(tokens).ok().expect("the test source parses");
    let crate_wide = crate_shapes_file();
    let fields = with_shapes.then(|| crate_wide.fields());
    match check_against(&parsed.client_code, &shapes(), fields) {
        Ok(probes) => {
            // Every question is beside server value 0, `projects`, or 1.
            assert!(probes.iter().all(|probe| probe.index < 2));
            probes
                .iter()
                .map(|probe| probe.tokens.to_string())
                .collect()
        }
        Err(error) => panic!("refused outright: {}", error.msg),
    }
}

/// Each read of a server value found, as `place expression = path`.
fn server_reads(markup: &str) -> Vec<String> {
    let src = format!("<div>{markup}<script>r##\"{SERVER}\"##</script></div>");
    let tokens = TokenStream::from_str(&src).expect("test source does not tokenize");
    let parsed = parse(tokens).ok().expect("the test source parses");
    analyze(&parsed.client_code, &shapes())
        .iter()
        .filter_map(|finding| match finding {
            Finding::ServerRead {
                place,
                expression,
                from: From::Constant(name),
                path,
                ..
            } => Some(format!("{place} {expression} = {name}{}", spelled(path))),
            _ => None,
        })
        .collect()
}

#[test]
fn the_crates_structs_are_written_once_as_shapes() {
    let module = crate_shapes(&crate_shapes_file()).to_string();
    assert!(module.contains("pub mod __rahti_shapes"), "{module}");
    // A struct by its path, and the object it is made of by its fields.
    assert!(
        module.contains(
            "impl :: rahti :: shape :: Shape for crate :: services :: projects :: Project { \
             type Node = Project__id__name__databases__owner__and_2_more ; }"
        ),
        "{module}"
    );
    assert!(
        module.contains("impl :: rahti :: shape :: Shape for crate :: routes :: p_projects :: Database { type Node = Database__id__engine ; }"),
        "{module}"
    );
    // A nested struct nobody listed still has a marker: `Owner`.
    assert!(module.contains("pub struct Owner__email ;"), "{module}");
    // One trait per field name, for the objects that have the field; a
    // struct that contains itself names its own marker.
    assert!(
        module.contains("impl F_owner for Project__id__name__databases__owner__and_2_more { type Out = :: rahti :: shape :: Maybe < Owner__email > ; }"),
        "{module}"
    );
    assert!(
        module.contains("impl F_parent for Project__id__name__databases__owner__and_2_more { type Out = :: rahti :: shape :: Maybe < Project__id__name__databases__owner__and_2_more > ; }"),
        "{module}"
    );
    assert!(
        module.contains(
            "impl F_id for Database__id__engine { type Out = :: rahti :: shape :: Open ; }"
        ),
        "{module}"
    );
    // A name that is not an identifier is spelled out.
    assert!(module.contains("pub trait F_content_2d_type"), "{module}");
    // A struct with many fields names the first and counts the rest.
    assert!(
        module.contains(
            "pub struct Wide__first_long_field_name__second_long_field_name__and_4_more ;"
        ),
        "{module}"
    );
}

#[test]
fn reads_of_a_server_value_are_followed_from_the_constant() {
    assert_eq!(
        server_reads(
            "<p hidden={projects.length === 0}>{projects[0].owner?.email}</p>
             <template pp-for=\"project in live()\">
                <li key={project.id}>
                    {project.name}{limit.anything}
                    <template pp-for=\"database in project.databases || []\">
                        <span key={database.id}>{database.engine}</span>
                    </template>
                </li>
            </template>"
        ),
        [
            // The script's own reads.
            "script projects.filter = projects.filter",
            "script projects[].name = projects[].name",
            "binding projects.length = projects.length",
            "binding projects[].owner.email = projects[].owner.email",
            "row project.id = projects[].id",
            "row project.name = projects[].name",
            // A plain value is asked about too: it has no shape to refuse by.
            "binding limit.anything = limit.anything",
            "row project.databases = projects[].databases",
            "row database.id = projects[].databases[].id",
            "row database.engine = projects[].databases[].engine",
        ]
    );
}

#[test]
fn each_field_step_of_a_read_is_a_question_of_its_own() {
    let asked = server_asked(
        "<template pp-for=\"project in projects\">
            <li key={project.id}>{project.owner.emial}</li>
        </template>",
        true,
    );
    let read = asked
        .iter()
        .find(|tokens| tokens.contains("emial"))
        .expect("questions about `project.owner.emial`");
    // `owner` is a field some struct has: its own trait, in the generated
    // module. `emial` is nobody's: the trait only an open shape answers to.
    assert!(
        read.contains(": crate :: routes :: __rahti_shapes :: F_owner"),
        "{read}"
    );
    assert!(read.contains(": :: rahti :: shape :: NoField"), "{read}");
    // Two questions, each with the message of its own step.
    assert!(
        read.contains(
            "rahti[RAHTI_SERVER_VALUE_FIELD]: `project` has no field `owner`: it comes from \
             the server value `projects`, whose Rust struct has no such field"
        ),
        "{read}"
    );
    assert!(
        read.contains("rahti[RAHTI_SERVER_VALUE_FIELD]: `project.owner` has no field `emial`"),
        "{read}"
    );
    // The second is asked only of a value that reaches `owner`.
    assert!(
        read.contains(
            "impl < S : :: rahti :: shape :: Shape + ? Sized > __RahtiLacks < < < < S as :: rahti :: shape :: Shape > :: Node as :: rahti :: shape :: Element > :: Out as crate :: routes :: __rahti_shapes :: F_owner > :: Out > for && :: rahti :: shape :: Known < S >"
        ),
        "{read}"
    );
}

#[test]
fn nothing_is_asked_where_nothing_could_be_refused() {
    // A crate with no generated shapes has nothing to compare a value with.
    assert!(server_asked("<p>{projects[0].anything}</p>", false).is_empty());

    // What an array or an object answers to by itself is not a field.
    let asked = server_asked("<p>{projects.length}{projects[0].toString()}</p>", true);
    assert!(
        !asked.iter().any(|tokens| tokens.contains("toString")),
        "{asked:?}"
    );
}

#[test]
fn a_field_the_block_assigns_is_not_asked_about() {
    let src = "<div><template pp-for=\"project in projects\">\
        <li key={project.id} onclick={project.open = !project.open}>{project.open}{project.name}</li>\
        </template><script>r##\"const projects = __pp_server[0];\"##</script></div>";
    let tokens = TokenStream::from_str(src).unwrap();
    let parsed = parse(tokens).ok().unwrap();
    let crate_wide = crate_shapes_file();
    let asked: Vec<String> =
        check_against(&parsed.client_code, &shapes(), Some(crate_wide.fields()))
            .ok()
            .unwrap()
            .iter()
            .map(|probe| probe.tokens.to_string())
            .collect();
    // `open` is given to rows in place, so it may be there; `name` is asked.
    assert!(
        !asked
            .iter()
            .any(|tokens| tokens.contains("no field `open`")),
        "{asked:?}"
    );
    assert!(
        asked
            .iter()
            .any(|tokens| tokens.contains("no field `name`")),
        "{asked:?}"
    );
}

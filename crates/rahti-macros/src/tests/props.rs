//! The props a component is passed, and the props it reads.
//!
//! Each refusal is paired with the ordinary code it must not catch: a prop
//! name is only a mistake when nothing could read it, or nothing could give
//! it.

use super::*;
use crate::parse::{Parsed, parse};
use proc_macro2::TokenStream;
use std::str::FromStr;

const FILE: &str = "src/app/projects/page.rs";

/// The shapes file of a page that can call three components: one that reads
/// four props, one that reads none, and one that may read any.
fn shapes() -> Shapes {
    let text = "\
C\tsrc/app/projects/page.rs\tMigrationDialog\tcurrent onPick onlineNow open
C\tsrc/app/projects/page.rs\tDivider\t
C\tsrc/app/projects/page.rs\tButton\t*
C\tsrc/app/other/page.rs\tElsewhere\topen
";
    Shapes::parse(text, FILE)
}

fn parsed(src: &str) -> Parsed {
    let tokens = TokenStream::from_str(src).expect("test source does not tokenize");
    match parse(tokens) {
        Ok(parsed) => parsed,
        Err(err) => panic!("expected `{src}` to parse, got:\n{}", err.msg),
    }
}

/// What a block with this markup is refused with, for a prop it passes.
fn passed(markup: &str) -> Option<String> {
    check_passed(&parsed(markup).ops, &shapes())
        .err()
        .map(|error| error.msg)
}

/// What a component with this template is refused with, for a prop it reads.
fn read(template: &str) -> Option<String> {
    let parsed = parsed(&format!("@component \"widget\" {template}"));
    assert!(parsed.forced_component.is_some());
    check_read(
        &parsed.client_code,
        parsed.root_attributes.as_deref(),
        |source, _| (source.span, String::new()),
    )
    .err()
    .map(|error| error.msg)
}

#[test]
fn an_attribute_name_is_read_in_camel_case() {
    assert_eq!(camel("user-name"), "userName");
    assert_eq!(camel("aria-labelledby"), "ariaLabelledby");
    assert_eq!(camel("open"), "open");
    assert_eq!(camel("data-state-open"), "dataStateOpen");
}

// ----- at the tag ---------------------------------------------------------------

#[test]
fn a_prop_the_component_never_reads_is_refused_at_the_tag() {
    let message =
        passed("<div><MigrationDialog curent={id} open={shown} /></div>").expect("a refusal");
    assert!(message.contains("RAHTI_PROP_NOT_READ"), "{message}");
    assert!(
        message.contains(
            "`<MigrationDialog>` is passed `curent={…}`, and `MigrationDialog` never reads \
             `pp.props.curent`"
        ),
        "{message}"
    );
    assert!(
        message.contains("It reads `current`, `onPick`, `onlineNow`, `open`"),
        "{message}"
    );
    assert!(message.contains("or `data-curent`"), "{message}");

    // Between another component's tags, too.
    assert!(passed("<div><Button><MigrationDialog curent={id} /></Button></div>").is_some());
}

#[test]
fn a_prop_the_component_reads_is_passed_by_either_spelling() {
    assert_eq!(
        passed(
            "<div><MigrationDialog open={shown} current={id} online-now={live} \
             onlineNow={live} onPick={choose} /></div>"
        ),
        None
    );
}

#[test]
fn a_name_the_root_element_uses_for_itself_is_never_refused() {
    // An attribute, a handler, and the prefixes nothing reads as a prop.
    assert_eq!(
        passed(
            "<div><Divider class={tone} hidden={!shown} disabled={busy} value={draft} \
             onclick={close()} oninput={setDraft(event.target.value)} \
             data-state={state} aria-label={label} pp-ref={host} key={row.id} \
             style={css} title={hint} /></div>"
        ),
        None
    );
}

#[test]
fn a_component_that_may_read_any_prop_is_passed_anything() {
    // The build could not see every way `Button` reads a prop.
    assert_eq!(passed("<div><Button anything={x} /></div>"), None);
    // A component this file's shapes do not list: another crate's, or one
    // the build did not find. And one another file can call is not this
    // file's to check.
    assert_eq!(passed("<div><Unlisted anything={x} /></div>"), None);
    assert_eq!(passed("<div><Elsewhere anything={x} /></div>"), None);
    // A component that reads no prop of its own is a wrapper: what it is
    // passed goes to its root, where code written elsewhere may read it.
    assert_eq!(passed("<div><Divider thick={wide} /></div>"), None);
    // A server prop is a Rust argument, which the compiler checks itself.
    assert_eq!(
        passed("<div><Divider thick=@{wide} label=\"x\" /></div>"),
        None
    );
}

// ----- in the component ---------------------------------------------------------

#[test]
fn a_prop_the_root_is_never_given_is_refused_in_the_component() {
    let message = read(
        "<form user-name=@{user_name} class=\"card\">\
            <b>{pp.props.userNmae}</b>\
         </form>",
    )
    .expect("a refusal");
    assert!(message.contains("RAHTI_PROP_NEVER_GIVEN"), "{message}");
    assert!(
        message.contains("this component reads `pp.props.userNmae`, a prop it is never given"),
        "{message}"
    );
    assert!(
        message.contains("Its root element gives it `userName`, `class`"),
        "{message}"
    );

    // In a handler and in the script too.
    assert!(
        read("<form><button onclick={pick(pp.props.missing)}>\"Go\"</button></form>").is_some()
    );
    let message = read(
        "<form><script>r##\"const [name, setName] = pp.state(pp.props.userName ?? '');\"##</script></form>",
    )
    .expect("a refusal");
    assert!(
        message.contains("Its root element is written with no attributes"),
        "{message}"
    );
}

#[test]
fn a_prop_the_root_is_written_with_is_read_freely() {
    assert_eq!(
        read(
            "<form user-name=@{user_name} class=\"card\" data-state=\"open\">\
                <b>{pp.props.userName}</b>\
                <p hidden={pp.props.dataState !== \"open\"}>{pp.props.class}</p>\
                <script>r##\"const [name, setName] = pp.state(pp.props.userName ?? '');\"##</script>\
             </form>"
        ),
        None
    );
}

#[test]
fn a_root_that_takes_a_spread_can_be_given_any_prop() {
    // The caller's `{…}` props arrive through `bindings`; an `Attrs` set may
    // carry anything.
    assert_eq!(
        read("<div @{..bindings}><b>{pp.props.anything}</b></div>"),
        None
    );
    assert_eq!(
        read("<div class=\"x\" @{..attrs}><b>{pp.props.anything}</b></div>"),
        None
    );
}

#[test]
fn props_used_whole_or_read_in_another_scope_are_left_alone() {
    // Destructured: which props are read is not written here.
    assert_eq!(
        read("<form><script>r##\"const { anything } = pp.props;\"##</script></form>"),
        None
    );
    // Fragment children between another component's tags read that
    // component's props.
    assert_eq!(
        read("<form><Card><>{pp.props.anything}</></Card></form>"),
        None
    );
    // A root that is another component, or a fragment, is not an element
    // whose attributes are all there to read.
    assert_eq!(read("<Card>{pp.props.anything}</Card>"), None);
    assert_eq!(read("<><b>{pp.props.anything}</b></>"), None);
}

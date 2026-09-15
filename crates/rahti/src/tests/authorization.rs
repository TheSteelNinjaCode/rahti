use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Deserialize)]
struct User {
    id: i32,
    admin: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    owner_id: i32,
}

fn user(id: i32, admin: bool) -> Value {
    serde_json::json!({ "id": id, "admin": admin })
}

#[test]
fn a_typed_session_rule_allows_and_denies() {
    let mut gate = Gate::new();
    gate.define::<User, _>("reports.view", |user| user.id == 7);

    assert_eq!(
        gate.inspect_value(Some(&user(7, false)), "reports.view", None),
        Decision::Allowed
    );
    assert_eq!(
        gate.inspect_value(Some(&user(8, false)), "reports.view", None),
        Decision::Denied
    );
}

#[test]
fn a_resource_policy_can_enforce_ownership() {
    let mut gate = Gate::new();
    gate.define_resource::<User, Record, _>("record.update", |user, record| {
        user.id == record.owner_id
    });
    let mine = serde_json::to_value(Record { owner_id: 7 }).unwrap();
    let theirs = serde_json::to_value(Record { owner_id: 8 }).unwrap();

    assert_eq!(
        gate.inspect_value(Some(&user(7, false)), "record.update", Some(&mine)),
        Decision::Allowed
    );
    assert_eq!(
        gate.inspect_value(Some(&user(7, false)), "record.update", Some(&theirs)),
        Decision::Denied
    );
}

#[test]
fn a_before_hook_can_grant_every_defined_ability_to_an_admin() {
    let mut gate = Gate::new();
    gate.define::<User, _>("reports.delete", |_| false);
    gate.before::<User, _>(|user, _| user.admin.then_some(true));

    assert_eq!(
        gate.inspect_value(Some(&user(1, true)), "reports.delete", None),
        Decision::Allowed
    );
}

#[test]
fn an_undefined_ability_fails_closed_before_the_admin_hook() {
    let mut gate = Gate::new();
    gate.before::<User, _>(|user, _| user.admin.then_some(true));

    assert_eq!(
        gate.inspect_value(Some(&user(1, true)), "misspelled.ability", None),
        Decision::Undefined
    );
}

#[test]
fn no_session_is_distinct_from_a_forbidden_session() {
    let mut gate = Gate::new();
    gate.define::<User, _>("reports.view", |_| false);

    assert_eq!(
        gate.inspect_value(None, "reports.view", None),
        Decision::Unauthenticated
    );
    assert_eq!(
        gate.inspect_value(Some(&user(1, false)), "reports.view", None),
        Decision::Denied
    );
}

#[test]
fn the_wrong_session_shape_is_denied_instead_of_panicking() {
    let mut gate = Gate::new();
    gate.define::<User, _>("reports.view", |_| true);

    assert_eq!(
        gate.inspect_value(
            Some(&serde_json::json!({ "unexpected": true })),
            "reports.view",
            None
        ),
        Decision::Denied
    );
}

#[test]
#[should_panic(expected = "defined more than once")]
fn duplicate_abilities_are_a_startup_error() {
    let mut gate = Gate::new();
    gate.define::<User, _>("reports.view", |_| true)
        .define::<User, _>("reports.view", |_| false);
}

#[test]
fn authorization_outside_a_request_is_unauthenticated() {
    let error = authorize("reports.view").unwrap_err();
    assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
}

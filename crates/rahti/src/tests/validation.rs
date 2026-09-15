use axum::body::to_bytes;

use super::*;
use crate::rpc_failed;

#[test]
fn several_fields_and_messages_are_collected_in_stable_order() {
    let mut validator = Validator::new();
    validator
        .required("email", "")
        .email("email", "not mail")
        .min_chars("password", "short", 10)
        .same("confirmation", "different", "password", "secret");

    let errors = validator.finish().expect_err("invalid");
    assert_eq!(errors.len(), 3);
    assert_eq!(errors.field("email").unwrap().len(), 2);
    assert_eq!(
        errors.first("confirmation"),
        Some("The confirmation field must match password.")
    );
    assert!(errors.to_string().starts_with("The confirmation field"));
}

#[test]
fn content_rules_leave_an_optional_empty_value_alone() {
    let mut validator = Validator::new();
    validator
        .email("email", "")
        .min_chars("nickname", "", 3)
        .max_chars("nickname", "", 20);
    assert!(validator.finish().is_ok());
}

#[test]
fn lengths_count_people_facing_characters_instead_of_utf8_bytes() {
    let mut validator = Validator::new();
    validator
        .min_chars("name", "José", 4)
        .max_chars("name", "José", 4);
    assert!(validator.finish().is_ok());
}

#[test]
fn typed_and_custom_rules_join_the_same_error_bag() {
    let mut validator = Validator::new();
    validator
        .required_option("plan", &None::<u8>)
        .one_of("role", &"owner", &["reader", "writer"])
        .min_value("age", &15, 18)
        .max_value("seats", &101, 100)
        .check("slug", false, "The slug is already taken.");

    let errors = validator.finish().expect_err("invalid");
    for field in ["age", "plan", "role", "seats", "slug"] {
        assert!(errors.has(field), "missing {field}");
    }
}

#[test]
fn validated_returns_the_original_value_after_success() {
    let mut validator = Validator::new();
    validator.required("name", "Ada");
    assert_eq!(validator.validated(42).unwrap(), 42);
}

#[tokio::test]
async fn validation_converts_to_a_structured_422_rpc_failure() {
    let mut errors = ValidationErrors::new();
    errors
        .add("email", "The email field is required.")
        .add("email", "The email field must be valid.");
    let response = rpc_failed(Error::from(errors));

    assert_eq!(response.status(), VALIDATION_STATUS);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body["error"],
        "The email field is required. The email field must be valid."
    );
    assert_eq!(body["errors"]["email"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_api_route_can_return_the_same_structured_validation_contract() {
    let mut errors = ValidationErrors::new();
    errors.add("name", "The name field is required.");
    let response = errors.into_response();

    assert_eq!(response.status(), VALIDATION_STATUS);
    assert!(response.extensions().get::<crate::RpcBody>().is_none());
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["errors"]["name"][0], "The name field is required.");
}

#[test]
#[should_panic(expected = "field names")]
fn an_empty_field_name_is_a_programming_error() {
    ValidationErrors::new().add(" ", "Wrong.");
}

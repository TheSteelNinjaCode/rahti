//! Structured application-input validation.
//!
//! Rust and serde establish the input's shape. This module handles business
//! rules after deserialization: required text, character lengths, plausible
//! email syntax, comparisons, allowed values, and application-defined checks.

use std::collections::BTreeMap;
use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::Error;

#[cfg(test)]
#[path = "tests/validation.rs"]
mod tests;

/// The status used when typed input is readable but violates business rules.
pub const VALIDATION_STATUS: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

/// Field names mapped to one or more user-facing messages.
///
/// A `BTreeMap` makes the JSON and display order deterministic. One field can
/// carry several messages so callers can render all failures or only `first`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ValidationErrors {
    fields: BTreeMap<String, Vec<String>>,
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one failure. Dotted and indexed field names are allowed.
    pub fn add(&mut self, field: impl Into<String>, message: impl Into<String>) -> &mut Self {
        let field = field.into();
        assert!(
            valid_field(&field),
            "validation field names cannot be empty or contain a NUL byte"
        );
        let message = message.into();
        assert!(
            !message.trim().is_empty(),
            "validation messages cannot be empty"
        );
        self.fields.entry(field).or_default().push(message);
        self
    }

    pub fn has(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }

    pub fn field(&self, field: &str) -> Option<&[String]> {
        self.fields.get(field).map(Vec::as_slice)
    }

    pub fn first(&self, field: &str) -> Option<&str> {
        self.field(field)
            .and_then(|messages| messages.first())
            .map(String::as_str)
    }

    pub fn fields(&self) -> &BTreeMap<String, Vec<String>> {
        &self.fields
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Number of fields with at least one failure.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Return success when empty, or the collected failures otherwise.
    pub fn into_result(self) -> ValidationResult {
        if self.is_empty() { Ok(()) } else { Err(self) }
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut messages = self.fields.values().flatten();
        if let Some(first) = messages.next() {
            formatter.write_str(first)?;
            for message in messages {
                write!(formatter, " {message}")?;
            }
            Ok(())
        } else {
            formatter.write_str("The given data was invalid.")
        }
    }
}

/// Ordinary `route.rs` handlers can return `Result<T, ValidationErrors>` and
/// receive the same 422 JSON contract as an RPC.
impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        let message = self.to_string();
        (
            VALIDATION_STATUS,
            Json(serde_json::json!({
                "error": message,
                "errors": self.fields,
            })),
        )
            .into_response()
    }
}

/// A validation operation's result.
pub type ValidationResult<T = ()> = std::result::Result<T, ValidationErrors>;

/// Implement on an application input type when its rules belong with the
/// type and are reused by more than one handler.
pub trait Validate {
    fn validate(&self) -> ValidationResult;
}

/// Collect several field-rule failures in one pass.
#[derive(Debug, Default)]
pub struct Validator {
    errors: ValidationErrors,
}

impl Validator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn errors(&self) -> &ValidationErrors {
        &self.errors
    }

    pub fn errors_mut(&mut self) -> &mut ValidationErrors {
        &mut self.errors
    }

    pub fn required(&mut self, field: &str, value: &str) -> &mut Self {
        if value.trim().is_empty() {
            self.errors
                .add(field, format!("The {field} field is required."));
        }
        self
    }

    pub fn required_option<T>(&mut self, field: &str, value: &Option<T>) -> &mut Self {
        if value.is_none() {
            self.errors
                .add(field, format!("The {field} field is required."));
        }
        self
    }

    /// Character length, not UTF-8 byte length. Empty values are optional;
    /// combine this with [`Self::required`] when they are not.
    pub fn min_chars(&mut self, field: &str, value: &str, minimum: usize) -> &mut Self {
        if !value.is_empty() && value.chars().count() < minimum {
            self.errors.add(
                field,
                format!("The {field} field must be at least {minimum} characters."),
            );
        }
        self
    }

    pub fn max_chars(&mut self, field: &str, value: &str, maximum: usize) -> &mut Self {
        if !value.is_empty() && value.chars().count() > maximum {
            self.errors.add(
                field,
                format!("The {field} field must not exceed {maximum} characters."),
            );
        }
        self
    }

    /// A deliberately modest syntax check, not proof that mail can arrive.
    /// Empty values are left to [`Self::required`].
    pub fn email(&mut self, field: &str, value: &str) -> &mut Self {
        let value = value.trim();
        let plausible = value.is_empty()
            || (value.len() >= 3
                && value.matches('@').count() == 1
                && !value.starts_with('@')
                && !value.ends_with('@')
                && !value.contains(char::is_whitespace));
        if !plausible {
            self.errors.add(
                field,
                format!("The {field} field must be a valid email address."),
            );
        }
        self
    }

    pub fn same(&mut self, field: &str, value: &str, other: &str, expected: &str) -> &mut Self {
        if value != expected {
            self.errors
                .add(field, format!("The {field} field must match {other}."));
        }
        self
    }

    pub fn one_of<T: PartialEq>(&mut self, field: &str, value: &T, allowed: &[T]) -> &mut Self {
        if !allowed.contains(value) {
            self.errors.add(
                field,
                format!("The {field} field contains an unsupported value."),
            );
        }
        self
    }

    pub fn min_value<T: PartialOrd + fmt::Display>(
        &mut self,
        field: &str,
        value: &T,
        minimum: T,
    ) -> &mut Self {
        if value < &minimum {
            self.errors.add(
                field,
                format!("The {field} field must be at least {minimum}."),
            );
        }
        self
    }

    pub fn max_value<T: PartialOrd + fmt::Display>(
        &mut self,
        field: &str,
        value: &T,
        maximum: T,
    ) -> &mut Self {
        if value > &maximum {
            self.errors.add(
                field,
                format!("The {field} field must not exceed {maximum}."),
            );
        }
        self
    }

    /// Add an application-specific rule without leaving the collection flow.
    pub fn check(&mut self, field: &str, passes: bool, message: impl Into<String>) -> &mut Self {
        if !passes {
            self.errors.add(field, message);
        }
        self
    }

    pub fn finish(self) -> ValidationResult {
        self.errors.into_result()
    }

    pub fn validated<T>(self, value: T) -> ValidationResult<T> {
        self.finish().map(|()| value)
    }
}

impl From<ValidationErrors> for Error {
    fn from(errors: ValidationErrors) -> Self {
        let message = errors.to_string();
        Error::new(VALIDATION_STATUS, message)
            .with_details(serde_json::json!({ "errors": errors.fields }))
    }
}

fn valid_field(field: &str) -> bool {
    !field.trim().is_empty() && !field.contains('\0')
}

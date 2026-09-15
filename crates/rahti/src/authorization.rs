//! Application-defined authorization gates.
//!
//! Authentication establishes the signed session. Authorization evaluates a
//! named ability against that session payload and, optionally, a resource.
//! Rahti supplies the mechanism; the application defines every ability.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

use axum::http::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Error, Result};

#[cfg(test)]
#[path = "tests/authorization.rs"]
mod tests;

type Rule = Arc<dyn Fn(&Value, Option<&Value>) -> std::result::Result<bool, String> + Send + Sync>;
type Before = Arc<
    dyn Fn(&Value, &str, Option<&Value>) -> std::result::Result<Option<bool>, String> + Send + Sync,
>;

/// The result of inspecting an ability without turning it into an HTTP error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Unauthenticated,
    Denied,
    Undefined,
}

impl Decision {
    pub fn allowed(self) -> bool {
        self == Self::Allowed
    }
}

/// A collection of named authorization rules.
#[derive(Clone, Default)]
pub struct Gate {
    rules: BTreeMap<String, Rule>,
    before: Vec<Before>,
}

impl fmt::Debug for Gate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Gate")
            .field("abilities", &self.rules.keys().collect::<Vec<_>>())
            .field("before_callbacks", &self.before.len())
            .finish()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Define an ability that depends only on the typed session payload.
    pub fn define<User, F>(&mut self, ability: impl Into<String>, check: F) -> &mut Self
    where
        User: DeserializeOwned,
        F: Fn(&User) -> bool + Send + Sync + 'static,
    {
        self.insert(
            ability,
            Arc::new(move |user, _| {
                let user = decode::<User>(user, "session payload")?;
                Ok(check(&user))
            }),
        )
    }

    /// Define an ability against both the typed session payload and a typed
    /// resource supplied by the handler.
    pub fn define_resource<User, Resource, F>(
        &mut self,
        ability: impl Into<String>,
        check: F,
    ) -> &mut Self
    where
        User: DeserializeOwned,
        Resource: DeserializeOwned,
        F: Fn(&User, &Resource) -> bool + Send + Sync + 'static,
    {
        self.insert(
            ability,
            Arc::new(move |user, resource| {
                let user = decode::<User>(user, "session payload")?;
                let resource = resource.ok_or_else(|| "no resource was supplied".to_string())?;
                let resource = decode::<Resource>(resource, "authorization resource")?;
                Ok(check(&user, &resource))
            }),
        )
    }

    /// Low-level rule for applications whose session/resource shapes are
    /// intentionally dynamic JSON.
    pub fn define_value<F>(&mut self, ability: impl Into<String>, check: F) -> &mut Self
    where
        F: Fn(&Value, Option<&Value>) -> bool + Send + Sync + 'static,
    {
        self.insert(
            ability,
            Arc::new(move |user, resource| Ok(check(user, resource))),
        )
    }

    /// Run before every *defined* ability. Returning `Some(true)` grants and
    /// `Some(false)` denies; `None` continues to the ability rule. Undefined
    /// ability names never reach this hook, so a typo cannot become an admin
    /// bypass.
    pub fn before<User, F>(&mut self, check: F) -> &mut Self
    where
        User: DeserializeOwned,
        F: Fn(&User, &str) -> Option<bool> + Send + Sync + 'static,
    {
        self.before.push(Arc::new(move |user, ability, _| {
            let user = decode::<User>(user, "session payload")?;
            Ok(check(&user, ability))
        }));
        self
    }

    pub fn abilities(&self) -> impl Iterator<Item = &str> {
        self.rules.keys().map(String::as_str)
    }

    pub fn contains(&self, ability: &str) -> bool {
        self.rules.contains_key(ability)
    }

    fn insert(&mut self, ability: impl Into<String>, rule: Rule) -> &mut Self {
        let ability = ability.into();
        assert!(
            valid_ability(&ability),
            "authorization abilities cannot be empty, contain whitespace, or contain a NUL byte"
        );
        assert!(
            !self.rules.contains_key(&ability),
            "authorization ability `{ability}` is defined more than once"
        );
        self.rules.insert(ability, rule);
        self
    }

    fn inspect_value(
        &self,
        user: Option<&Value>,
        ability: &str,
        resource: Option<&Value>,
    ) -> Decision {
        let Some(user) = user else {
            return Decision::Unauthenticated;
        };
        let Some(rule) = self.rules.get(ability) else {
            return Decision::Undefined;
        };

        for before in &self.before {
            match before(user, ability, resource) {
                Ok(Some(true)) => return Decision::Allowed,
                Ok(Some(false)) => return Decision::Denied,
                Ok(None) => {}
                Err(error) => {
                    policy_warning(ability, &error);
                    return Decision::Denied;
                }
            }
        }

        match rule(user, resource) {
            Ok(true) => Decision::Allowed,
            Ok(false) => Decision::Denied,
            Err(error) => {
                policy_warning(ability, &error);
                Decision::Denied
            }
        }
    }
}

fn configured() -> &'static RwLock<Arc<Gate>> {
    static CONFIGURED: OnceLock<RwLock<Arc<Gate>>> = OnceLock::new();
    CONFIGURED.get_or_init(|| RwLock::new(Arc::new(Gate::new())))
}

/// Install the application's complete gate. Call once during startup before
/// building the router. A later call replaces it, which keeps in-process test
/// setup deterministic.
pub fn configure(gate: Gate) {
    match configured().write() {
        Ok(mut installed) => *installed = Arc::new(gate),
        Err(poisoned) => *poisoned.into_inner() = Arc::new(gate),
    }
}

/// The installed gate, as an immutable snapshot.
pub fn gate() -> Arc<Gate> {
    configured()
        .read()
        .map(|gate| gate.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// Inspect a session-only ability.
pub fn inspect(ability: &str) -> Decision {
    inspect_value(ability, None)
}

/// Inspect an ability against a serializable resource.
pub fn inspect_resource<Resource: Serialize>(ability: &str, resource: &Resource) -> Decision {
    let resource = match serde_json::to_value(resource) {
        Ok(resource) => resource,
        Err(error) => {
            policy_warning(
                ability,
                &format!("resource could not be serialized: {error}"),
            );
            return if crate::auth::is_authenticated() {
                Decision::Denied
            } else {
                Decision::Unauthenticated
            };
        }
    };
    inspect_value(ability, Some(&resource))
}

pub fn allows(ability: &str) -> bool {
    inspect(ability).allowed()
}

pub fn allows_resource<Resource: Serialize>(ability: &str, resource: &Resource) -> bool {
    inspect_resource(ability, resource).allowed()
}

/// Require an ability, returning 401 without a session and 403 for every
/// authenticated denial (including an undefined ability).
pub fn authorize(ability: &str) -> Result<()> {
    decision_error(ability, inspect(ability))
}

pub fn authorize_resource<Resource: Serialize>(ability: &str, resource: &Resource) -> Result<()> {
    decision_error(ability, inspect_resource(ability, resource))
}

fn inspect_value(ability: &str, resource: Option<&Value>) -> Decision {
    if !valid_ability(ability) {
        policy_warning(ability, "the ability name is empty or malformed");
        return if crate::auth::is_authenticated() {
            Decision::Undefined
        } else {
            Decision::Unauthenticated
        };
    }
    let session = crate::auth::session();
    gate().inspect_value(
        session.as_ref().map(crate::auth::Session::user),
        ability,
        resource,
    )
}

fn decision_error(ability: &str, decision: Decision) -> Result<()> {
    match decision {
        Decision::Allowed => Ok(()),
        Decision::Unauthenticated => Err(Error::new(
            StatusCode::UNAUTHORIZED,
            "This action requires an authenticated session.",
        )),
        Decision::Denied => Err(Error::new(
            StatusCode::FORBIDDEN,
            "You are not authorized to perform this action.",
        )),
        Decision::Undefined => {
            policy_warning(ability, "no policy defines this ability");
            Err(Error::new(
                StatusCode::FORBIDDEN,
                "You are not authorized to perform this action.",
            ))
        }
    }
}

fn decode<T: DeserializeOwned>(value: &Value, what: &str) -> std::result::Result<T, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("{what} has the wrong shape: {error}"))
}

fn valid_ability(ability: &str) -> bool {
    !ability.is_empty() && !ability.contains('\0') && !ability.chars().any(char::is_whitespace)
}

fn policy_warning(ability: &str, problem: &str) {
    crate::dev::record_server(
        "warn",
        "authorization",
        &format!("rahti: authorization for `{ability}` failed closed — {problem}."),
    );
}

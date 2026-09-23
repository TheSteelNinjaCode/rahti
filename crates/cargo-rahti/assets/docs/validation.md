# Validation

## Boundary

Serde and Rust types validate the payload's shape before an RPC runs. An
`i32` parameter does not receive text, and a required parameter does not
receive a missing value. `rahti::Validator` handles the next layer: readable,
typed input that violates application rules.

Validation belongs at the handler boundary, before a database query, external
call, cache write, or other side effect. Database constraints remain the final
authority for invariants such as uniqueness when requests race.

## Collecting Field Errors

```rust
use crate::rahti::{Result, Validator, rpc};

#[rpc]
pub async fn register(
    email: String,
    password: String,
    confirmation: String,
) -> Result<Account> {
    let email = email.trim().to_lowercase();

    let mut validator = Validator::new();
    validator
        .required("email", &email)
        .email("email", &email)
        .max_chars("email", &email, 254)
        .required("password", &password)
        .min_chars("password", &password, 10)
        .same("confirmation", &confirmation, "password", &password);
    validator.finish()?;

    create_account(email, password).await
}
```

Rules run in the order they are called and collect every failure. Messages are
grouped by field in deterministic field-name order. Content rules such as
`email`, `min_chars`, and `max_chars` skip an empty string; combine them with
`required` when empty input is invalid. Character-length rules count Unicode
characters, not UTF-8 bytes.

Built-in rules are:

| Rule | Purpose |
| --- | --- |
| `required(field, text)` | Reject blank or whitespace-only text |
| `required_option(field, &value)` | Reject `None` |
| `min_chars` / `max_chars` | Bound user-facing character length |
| `email` | Check plausible email syntax, not mailbox ownership |
| `same` | Require two text fields to match |
| `one_of` | Require a typed value to be in an allowed slice |
| `min_value` / `max_value` | Bound a typed comparable number or value |
| `check(field, passes, message)` | Add an application-defined synchronous rule |

Use `errors_mut().add(field, message)` when a rule needs several steps. An
asynchronous rule is checked outside the fluent chain, then added to the same
bag:

```rust
let mut validator = Validator::new();
validator.required("slug", &slug);

if slug_exists(&slug).await? {
    validator
        .errors_mut()
        .add("slug", "That slug is already in use.");
}

validator.finish()?;
```

Do not treat a pre-query uniqueness check as sufficient. Keep the database
unique constraint and translate a constraint race into the appropriate
application error.

## Reusable Input Types

Implement `Validate` when one input's rules are used in several handlers:

```rust
use crate::rahti::{Validate, ValidationResult, Validator};

struct ProfileInput {
    name: String,
}

impl Validate for ProfileInput {
    fn validate(&self) -> ValidationResult {
        let mut validator = Validator::new();
        validator
            .required("name", &self.name)
            .max_chars("name", &self.name, 80);
        validator.finish()
    }
}
```

`Validator::validated(value)` is the compact alternative when validation and
construction happen together. `ValidationErrors` can also be built directly;
it provides `add`, `has`, `field`, `first`, `fields`, `len`, `is_empty`, and
`into_result`.

## Error Contract

`ValidationErrors` converts into `rahti::Error`, so `validator.finish()?`
works inside the ordinary `rahti::Result<T>` alongside database and other
errors. No second RPC result type is required.

An RPC validation failure returns HTTP 422 with a readable summary and every
field message:

```json
{
  "error": "The email field is required. The password field must be at least 10 characters.",
  "errors": {
    "email": ["The email field is required."],
    "password": ["The password field must be at least 10 characters."]
  }
}
```

PulsePoint rejects with a `RpcError`. Its `message` is the `error` summary,
`status` is `422`, and `errors` is the field map. Read a field directly in
the caller:

```javascript
try {
    await pp.rpc("save_profile", { email });
} catch (error) {
    if (error.status === 422) {
        setEmailError(error.errors.email?.[0] ?? "Check the email field.");
    } else {
        setStatus(error.message);
    }
}
```

The same shape is available for file uploads made with `onUploadProgress`.
Never place secrets, password values, tokens, query text, or database
diagnostics in validation messages.

Malformed JSON and values that cannot deserialize remain HTTP 400 RPC wire
errors. HTTP 422 means the payload was structurally readable and business
validation ran.

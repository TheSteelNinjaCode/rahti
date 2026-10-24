//! What `html!` checks a `pp.rpc` call against, at compile time.
//!
//! `#[rpc]` writes a [`Signature`] beside the function it marks, named after
//! it. For each `pp.rpc("name", { … })` a template's client code makes,
//! `html!` refers to that constant by name — so a name no `#[rpc]` in the
//! module declares fails to resolve, with rustc's own error and suggestions —
//! and evaluates the functions below in a `const`, so a key that is not a
//! parameter, or a required parameter left out, fails `cargo check` on the
//! token that wrote it.
//!
//! None of this exists at runtime, and none of it is a security boundary: a
//! payload is still read and validated by the server whoever sent it, and a
//! call whose keys are only known in the browser — `FormData`,
//! `Object.fromEntries(…)`, a variable — is checked by name alone.
//!
//! A literal shape mismatch is rejected by the expanded template:
//! ```compile_fail,E0080
//! use rahti::{html, rpc, Html};
//! #[rpc]
//! async fn greet(name: String) -> String { name }
//! fn page() -> Html {
//!     html! { <button onclick={pp.rpc("greet", { name: 12 })}>"Send"</button> }
//! }
//! ```
//! The same call with a string compiles:
//! ```
//! use rahti::{html, rpc, Html};
//! #[rpc]
//! async fn greet(name: String) -> String { name }
//! fn page() -> Html {
//!     html! { <button onclick={pp.rpc("greet", { name: "Ada" })}>"Send"</button> }
//! }
//! ```

/// An `#[rpc]`'s parameters, as the payload has to supply them.
#[derive(Debug, Clone, Copy)]
pub struct Signature {
    /// The name the browser calls it by.
    pub name: &'static str,
    pub params: &'static [Param],
    /// Whether a parameter is a file, so that the payload carries a `File`
    /// and `pp.rpc` posts it as `FormData`. An object literal holding the file
    /// does that by itself, so no call is refused for it.
    pub upload: bool,
}

/// One parameter: the payload key, and whether a call may leave it out. An
/// `Option<T>`, an optional file and a file list may be; everything else is
/// required.
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub key: &'static str,
    pub required: bool,
    pub rust_type: &'static str,
    pub wire: Wire,
    pub nullable: bool,
}

/// Conservative wire shapes. Named application types have no inferred schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Opaque,
    String,
    Number,
    Boolean,
    Array,
    Object,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Literal {
    String,
    Number,
    Boolean,
    Null,
    Array,
    Object,
}

/// Refuse a proven shape mismatch. Strings may contain JSON text, as form fields do.
pub const fn value(signature: &Signature, key: &str, literal: Literal, at: &str) {
    // Multipart encodes ordinary fields as text, including numbers and booleans.
    if signature.upload {
        return;
    }
    let mut i = 0;
    while i < signature.params.len() {
        let param = &signature.params[i];
        if eq(param.key, key) {
            let valid = match (param.wire, literal) {
                (Wire::Opaque, _) | (_, Literal::String) => true,
                (_, Literal::Null) => param.nullable,
                (Wire::Number, Literal::Number)
                | (Wire::Boolean, Literal::Boolean)
                | (Wire::Array, Literal::Array)
                | (Wire::Object, Literal::Object) => true,
                _ => false,
            };
            if !valid {
                let mut message = Message::new();
                message.push("rahti[RAHTI_RPC_ARGUMENT_TYPE]: `");
                message.push(signature.name);
                message.push("` argument `");
                message.push(key);
                message.push("` expects ");
                message.push(param.rust_type);
                message.push(", received a ");
                message.push(match literal {
                    Literal::String => "string",
                    Literal::Number => "number",
                    Literal::Boolean => "boolean",
                    Literal::Null => "null",
                    Literal::Array => "array",
                    Literal::Object => "object",
                });
                message.push(" literal");
                message.push(at);
                message.push(".\n  Send a value matching the Rust parameter. Dynamic values are validated by the server.");
                panic!("{}", message.as_str());
            }
            return;
        }
        i += 1;
    }
}

/// `pp.rpc(…, { key })`: refused unless `key` is a parameter.
///
/// `at` says where the call is when its span cannot — `" (line 12)"` for a
/// call in a `<script>` body, which is one literal — and is empty otherwise.
pub const fn key(signature: &Signature, key: &str, at: &str) {
    let mut i = 0;
    while i < signature.params.len() {
        if eq(signature.params[i].key, key) {
            return;
        }
        i += 1;
    }
    let mut message = Message::new();
    message.push("rahti[RAHTI_RPC_UNKNOWN_ARGUMENT]: `");
    message.push(signature.name);
    message.push("` has no parameter `");
    message.push(key);
    message.push("`");
    message.push(at);
    message.push(".\n  ");
    parameters(&mut message, signature);
    message.push(" A payload key is the parameter's own name.");
    panic!("{}", message.as_str());
}

/// `pp.rpc(…, { … })` with every key written out: refused when a required
/// parameter is not among `keys`.
pub const fn complete(signature: &Signature, keys: &[&str], at: &str) {
    let mut i = 0;
    while i < signature.params.len() {
        let param = signature.params[i];
        if param.required && !contains(keys, param.key) {
            let mut message = Message::new();
            message.push("rahti[RAHTI_RPC_MISSING_ARGUMENT]: this call to `");
            message.push(signature.name);
            message.push("`");
            message.push(at);
            message.push(" does not send `");
            message.push(param.key);
            message.push("`, which it requires.\n  ");
            parameters(&mut message, signature);
            message.push(" Only an `Option<T>`, an optional file or a file list may be left out.");
            panic!("{}", message.as_str());
        }
        i += 1;
    }
}

/// `pp.rpc("name")`, with no payload: refused when a parameter is required.
pub const fn no_payload(signature: &Signature, at: &str) {
    let mut i = 0;
    while i < signature.params.len() {
        if signature.params[i].required {
            let mut message = Message::new();
            message.push("rahti[RAHTI_RPC_MISSING_ARGUMENT]: this call to `");
            message.push(signature.name);
            message.push("`");
            message.push(at);
            message.push(" sends no arguments, and it requires `");
            message.push(signature.params[i].key);
            message.push("`.\n  ");
            parameters(&mut message, signature);
            message.push(" Send them by name: `pp.rpc(\"");
            message.push(signature.name);
            message.push("\", { … })`.");
            panic!("{}", message.as_str());
        }
        i += 1;
    }
}

/// "Its parameters are `id`, `text` (optional)." — or that it has none.
const fn parameters(message: &mut Message, signature: &Signature) {
    if signature.params.is_empty() {
        message.push("It takes no arguments: send `{}`.");
        return;
    }
    message.push("Its parameters are ");
    let mut i = 0;
    while i < signature.params.len() {
        if i > 0 {
            message.push(", ");
        }
        message.push("`");
        message.push(signature.params[i].key);
        message.push("`");
        if !signature.params[i].required {
            message.push(" (optional)");
        }
        i += 1;
    }
    message.push(".");
}

const fn contains(keys: &[&str], key: &str) -> bool {
    let mut i = 0;
    while i < keys.len() {
        if eq(keys[i], key) {
            return true;
        }
        i += 1;
    }
    false
}

const fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// An error message assembled during constant evaluation, where `format!`
/// is not available: `panic!` takes one `{}` of a `&str`, so the pieces are
/// copied into a buffer first.
struct Message {
    bytes: [u8; 1024],
    len: usize,
}

impl Message {
    const fn new() -> Self {
        Message {
            bytes: [0; 1024],
            len: 0,
        }
    }

    /// Whole pieces only, so the buffer always holds valid UTF-8.
    const fn push(&mut self, piece: &str) {
        let piece = piece.as_bytes();
        if self.len + piece.len() > self.bytes.len() {
            return;
        }
        let mut i = 0;
        while i < piece.len() {
            self.bytes[self.len + i] = piece[i];
            i += 1;
        }
        self.len += piece.len();
    }

    const fn as_str(&self) -> &str {
        let (written, _) = self.bytes.split_at(self.len);
        match core::str::from_utf8(written) {
            Ok(message) => message,
            Err(_) => "rahti: a `pp.rpc` call does not match its `#[rpc]`.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RENAME: Signature = Signature {
        name: "rename",
        params: &[
            Param {
                key: "id",
                required: true,
                rust_type: "i32",
                wire: Wire::Number,
                nullable: false,
            },
            Param {
                key: "note",
                required: false,
                rust_type: "Option<String>",
                wire: Wire::String,
                nullable: true,
            },
        ],
        upload: false,
    };

    // Evaluated at compile time: a regression here fails the build.
    const _: () = key(&RENAME, "id", "");
    const _: () = key(&RENAME, "note", "");
    const _: () = complete(&RENAME, &["id"], "");
    const _: () = complete(&RENAME, &["note", "id"], "");
    const _: () = value(&RENAME, "id", Literal::Number, "");
    const _: () = value(&RENAME, "id", Literal::String, "");
    const _: () = value(&RENAME, "note", Literal::Null, "");

    fn panic_message(check: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let payload = std::panic::catch_unwind(check).unwrap_err();
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap()
    }

    #[test]
    fn an_unknown_key_names_it_and_lists_the_parameters() {
        let message = panic_message(|| key(&RENAME, "nmae", ""));
        assert!(
            message.contains("`rename` has no parameter `nmae`"),
            "{message}"
        );
        assert!(message.contains("`id`, `note` (optional)"), "{message}");
    }

    #[test]
    fn a_missing_required_parameter_is_named() {
        let message = panic_message(|| complete(&RENAME, &["note"], " (line 7)"));
        assert!(
            message.contains("`rename` (line 7) does not send `id`"),
            "{message}"
        );
        let message = panic_message(|| no_payload(&RENAME, ""));
        assert!(message.contains("requires `id`"), "{message}");
    }

    #[test]
    fn a_function_without_parameters_says_to_send_nothing() {
        let none = Signature {
            name: "list",
            params: &[],
            upload: false,
        };
        no_payload(&none, "");
        let message = panic_message(move || key(&none, "id", ""));
        assert!(message.contains("It takes no arguments"), "{message}");
    }

    #[test]
    fn a_shape_mismatch_names_the_contract_without_printing_values() {
        let message = panic_message(|| value(&RENAME, "note", Literal::Number, " (line 8)"));
        assert!(message.contains("RAHTI_RPC_ARGUMENT_TYPE"), "{message}");
        assert!(
            message.contains("`rename` argument `note` expects Option<String>"),
            "{message}"
        );
        assert!(message.contains("number literal (line 8)"), "{message}");
        assert!(
            panic_message(|| value(&RENAME, "id", Literal::Null, ""))
                .contains("RAHTI_RPC_ARGUMENT_TYPE")
        );
    }

    #[test]
    fn custom_deserializers_are_not_guessed() {
        let signature = Signature {
            name: "custom",
            params: &[Param {
                key: "value",
                required: true,
                rust_type: "Custom",
                wire: Wire::Opaque,
                nullable: false,
            }],
            upload: false,
        };
        for literal in [
            Literal::String,
            Literal::Number,
            Literal::Boolean,
            Literal::Null,
            Literal::Array,
            Literal::Object,
        ] {
            value(&signature, "value", literal, "");
        }
    }

    #[test]
    fn multipart_fields_are_text_even_when_the_caller_supplies_a_number() {
        let signature = Signature {
            upload: true,
            ..RENAME
        };
        value(&signature, "note", Literal::Number, "");
    }
}

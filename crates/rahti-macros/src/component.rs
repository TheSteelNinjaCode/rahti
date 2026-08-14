//! Naming a block for the client runtime.
//!
//! Every convention file under `app/` gets a `pp-component` identifier of the
//! form `<kind>_<hash>`, where the hash covers the file's path below the app
//! directory:
//!
//! ```text
//! src/app/page.rs                  page_9f2b41c7
//! src/app/users/page.rs            page_a004a681
//! src/app/customers/[id]/page.rs   page_1c8d5e30
//! src/app/(marketing)/layout.rs    layout_6b0f2a94
//! src/app/not-found.rs             not_found_44e1c8b2
//! ```
//!
//! The value is used verbatim as a JavaScript variable name, so it has to be
//! a valid identifier: `[A-Za-z_$][A-Za-z0-9_$]*` and not a reserved word.
//! Deriving it from a hash rather than from the path segments is what
//! guarantees that — directory names like `[[...slug]]` and `(marketing)`
//! carry characters no identifier may contain.
//!
//! `rahti-build`'s `route.rs` reimplements this and must stay byte-identical;
//! its `scanner.rs` proves at build time that no two files collide.
//!
//! Components are named the other way round — after their function rather
//! than their file, since one file may hold several. See
//! [`function_identifier`].

/// Set by `rahti-build` via `cargo:rustc-env`, so a project that moves
/// its app directory keeps working. rustc passes its own environment to
/// proc macros, which run in-process during expansion.
const APP_DIR_ENV: &str = "RAHTI_APP_DIR";

const DEFAULT_APP_DIR: &str = "src/app";

#[cfg(test)]
#[path = "tests/component.rs"]
mod tests;

/// The identifier for the file an `html!` block was written in.
///
/// `None` for anything that is not a convention file under the app
/// directory — a helper module has no route to be identified by.
pub fn identifier(file: &str) -> Option<String> {
    let path = file.replace('\\', "/");
    let app_dir = std::env::var(APP_DIR_ENV)
        .unwrap_or_else(|_| DEFAULT_APP_DIR.to_string())
        .replace('\\', "/");
    let marker = format!("{}/", app_dir.trim_end_matches('/'));

    // Spans report the path rustc was given, which may be absolute.
    let (_, relative) = path.rsplit_once(&marker)?;
    let name = relative.rsplit_once('/').map_or(relative, |(_, name)| name);

    let kind = match name {
        "page.rs" => "page",
        "layout.rs" => "layout",
        "error.rs" => "error",
        // Underscored: `not-found` is not a legal identifier.
        "not-found.rs" => "not_found",
        _ => return None,
    };

    Some(format!("{kind}_{:08x}", fnv1a32(relative)))
}

/// The identifier for a `#[component]` function: `UserCard` in
/// `src/components/user_card.rs` becomes `user_card_3b91f0ae`.
///
/// Unlike a convention file, a component is named by its function rather than
/// by its path — one file may hold several. The hash covers both, so moving a
/// component between files renames it and two components never collide.
pub fn function_identifier(file: &str, function: &str) -> String {
    let name = snake_case(function);
    let id = format!(
        "{name}_{:08x}",
        fnv1a32(&format!("{}#{function}", below_src(file)))
    );

    // Rust accepts identifiers this ASCII-only check does not, so fall back
    // to a name that is legal by construction rather than shipping one the
    // client runtime cannot bind.
    if is_valid_js_identifier(&id) {
        id
    } else {
        format!(
            "component_{:08x}",
            fnv1a32(&format!("{}#{function}", below_src(file)))
        )
    }
}

/// The part of a path below the crate's `src/`, forward slashes.
///
/// Spans report whatever path rustc was given, which is absolute on some
/// invocations. Hashing that would give a different name on every machine,
/// and these names ship inside checked-in markup.
fn below_src(file: &str) -> String {
    let path = file.replace('\\', "/");
    match path.rsplit_once("src/") {
        Some((_, relative)) => relative.to_string(),
        None => path,
    }
}

/// `UserCard` -> `user_card`, `Button` -> `button`, `HTMLBadge` -> `html_badge`.
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);

    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev_was_lower = chars[i - 1].is_lowercase() || chars[i - 1].is_numeric();
            // The last capital of a run belongs to the word it starts:
            // `HTMLBadge` breaks before `B`, not before `L`.
            let starts_word = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if (prev_was_lower || starts_word) && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// FNV-1a, 32-bit.
///
/// Chosen over `DefaultHasher`, whose output std explicitly does not promise
/// to keep stable: these names end up in checked-in markup and in client
/// code, so the same path must hash the same on every machine and release.
pub fn fnv1a32(input: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in input.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Words JavaScript will not accept as a variable name.
const RESERVED: [&str; 40] = [
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "eval",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "package",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
];

/// Can this name be used as a JavaScript variable?
///
/// ASCII-only on purpose. JavaScript allows a much wider set, but these
/// identifiers travel through HTML attributes, generated JS and hand-written
/// selectors, and the wider set buys nothing here.
pub fn is_valid_js_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
        return false;
    }
    !RESERVED.contains(&name)
}

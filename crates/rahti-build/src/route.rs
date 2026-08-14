//! Route model: how a path on disk becomes a URL.

#[cfg(test)]
#[path = "tests/route.rs"]
mod tests;

/// What a file in `app/` contributes to the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `page.rs` — a GET route.
    Page,
    /// `route.rs` — one or more method handlers.
    Route,
    /// `not-found.rs` — the fallback.
    NotFound,
    /// `layout.rs` — wraps every page in its subtree.
    Layout,
    /// `error.rs` — error boundary. Recorded, not yet wired.
    Error,
    /// `loading.rs` — the markup shown while the next page is fetched.
    Loading,
}

impl Kind {
    pub fn from_file_name(name: &str) -> Option<Self> {
        match name {
            "page.rs" => Some(Kind::Page),
            "route.rs" => Some(Kind::Route),
            "not-found.rs" => Some(Kind::NotFound),
            "layout.rs" => Some(Kind::Layout),
            "error.rs" => Some(Kind::Error),
            "loading.rs" => Some(Kind::Loading),
            _ => None,
        }
    }

    /// Does this file produce URLs?
    pub fn is_routable(self) -> bool {
        matches!(self, Kind::Page | Kind::Route)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Page => "page",
            Kind::Route => "route",
            Kind::NotFound => "not-found",
            Kind::Layout => "layout",
            Kind::Error => "error",
            Kind::Loading => "loading",
        }
    }
}

/// One directory name, interpreted.
pub enum Segment {
    /// `(marketing)` — contributes no URL segment.
    Group,
    /// `users`
    Literal(String),
    /// `[id]` -> `{id}`
    Dynamic(String),
    /// `[...id]` -> `{*id}`, one or more segments.
    CatchAll(String),
    /// `[[...slug]]` -> zero or more, so it yields two routes.
    OptionalCatchAll(String),
}

pub fn parse_segment(raw: &str) -> Segment {
    // Order matters: `[[...x]]` also matches the `[x]` test.
    if raw.starts_with('(') && raw.ends_with(')') {
        Segment::Group
    } else if raw.starts_with("[[...") && raw.ends_with("]]") {
        Segment::OptionalCatchAll(raw[5..raw.len() - 2].to_string())
    } else if raw.starts_with("[...") && raw.ends_with(']') {
        Segment::CatchAll(raw[4..raw.len() - 1].to_string())
    } else if raw.starts_with('[') && raw.ends_with(']') {
        Segment::Dynamic(raw[1..raw.len() - 1].to_string())
    } else {
        Segment::Literal(raw.to_string())
    }
}

/// The `pp-component` name the `html!` macro stamps on this file's root
/// element: `customers/[id]/page.rs` -> `page_1c8d5e30`.
///
/// `relative` is the file's path below the app directory, forward slashes,
/// no leading slash — exactly the string the macro hashes.
///
/// Mirrors `crates/rahti-macros/src/component.rs` and must stay
/// byte-identical to it: the macro derives its copy from the source path at
/// expansion time, which is all it can see, while this one lets the manifest
/// report the same names and lets `scanner::validate` prove they are unique.
pub fn component_id(kind: Kind, relative: &str) -> String {
    let prefix = match kind {
        Kind::Page => "page",
        Kind::Layout => "layout",
        Kind::Error => "error",
        Kind::Loading => "loading",
        // Underscored: `not-found` is not a legal JavaScript identifier.
        Kind::NotFound => "not_found",
        Kind::Route => "route",
    };
    format!("{prefix}_{:08x}", fnv1a32(relative))
}

/// FNV-1a, 32-bit.
///
/// Chosen over `DefaultHasher`, whose output std explicitly does not promise
/// to keep stable: these names end up in checked-in markup and in client
/// code, so the same path must hash the same on every machine and release.
fn fnv1a32(input: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in input.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Does `dirs` sit at or below `ancestor`?
///
/// Compared as raw directory names, so a layout inside `(marketing)` covers
/// its subtree even though the group contributes no URL segment.
pub fn covers(ancestor: &[String], dirs: &[String]) -> bool {
    ancestor.len() <= dirs.len() && ancestor.iter().zip(dirs).all(|(a, b)| a == b)
}

/// Matching specificity, used only to order the generated file readably.
/// Axum's matcher ranks internally, so this is cosmetic.
pub fn rank(url: &str) -> u8 {
    if url.contains("{*") {
        2
    } else if url.contains('{') {
        1
    } else {
        0
    }
}

/// Turn the directory chain into one or more URLs.
///
/// Returns more than one only for an optional catch-all, which answers both
/// its parent path and everything below it.
pub fn to_urls(dirs: &[String]) -> Vec<String> {
    let mut urls = vec![String::new()];

    for dir in dirs {
        match parse_segment(dir) {
            Segment::Group => {}
            Segment::Literal(name) => {
                for url in &mut urls {
                    url.push('/');
                    url.push_str(&name);
                }
            }
            Segment::Dynamic(name) => {
                for url in &mut urls {
                    url.push_str(&format!("/{{{name}}}"));
                }
            }
            Segment::CatchAll(name) => {
                for url in &mut urls {
                    url.push_str(&format!("/{{*{name}}}"));
                }
            }
            Segment::OptionalCatchAll(name) => {
                // The bare parent path, plus the wildcard form.
                let mut expanded = Vec::with_capacity(urls.len() * 2);
                for url in &urls {
                    expanded.push(url.clone());
                    expanded.push(format!("{url}/{{*{name}}}"));
                }
                urls = expanded;
            }
        }
    }

    for url in &mut urls {
        if url.is_empty() {
            url.push('/');
        }
    }
    urls
}

/// A unique, valid Rust identifier for the module that will point at `dirs`.
pub fn module_name(kind: Kind, dirs: &[String], used: &mut Vec<String>) -> String {
    let prefix = match kind {
        Kind::Page => "p",
        Kind::Route => "r",
        Kind::NotFound => "nf",
        Kind::Layout => "l",
        Kind::Error => "e",
        Kind::Loading => "ld",
    };

    let mut parts: Vec<String> = Vec::new();
    for dir in dirs {
        let cleaned: String = dir
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    if parts.is_empty() {
        parts.push("root".to_string());
    }

    let base = format!("{prefix}_{}", parts.join("_"));

    // Distinct paths can still sanitize to the same identifier (`[id]` and `id`).
    let mut name = base.clone();
    let mut n = 2;
    while used.contains(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    used.push(name.clone());
    name
}

//! A button that arrives with the crate.
//!
//! Note the import: `rahti::` rather than `crate::rahti::`. A page in the app
//! can write either, because the app re-exports the runtime at its own root —
//! a library has no such root to borrow, which is why `html!` expands to
//! absolute `::rahti::` paths.

use rahti::{Html, component, html};

/// A link that reads as a button, in the library's own colours.
///
/// `bg-fuchsia-600` and `ring-lime-400` are written nowhere in the app. If
/// they reach `public/css/styles.css`, they got there because Tailwind was
/// pointed at this crate.
#[component]
pub fn Button(label: &str, href: &str) -> Html {
    html! {
        <a class="inline-flex items-center justify-center rounded-lg bg-fuchsia-600 px-4 py-2 text-sm font-medium text-white shadow-sm transition-colors hover:bg-fuchsia-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-lime-400"
                href=@{href}>@{label}</a>
    }
}

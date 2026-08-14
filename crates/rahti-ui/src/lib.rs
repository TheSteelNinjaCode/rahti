//! Components an app installs rather than writes.
//!
//! The only thing separating this from a crate on crates.io is that the
//! dependency on `rahti` is a path. There is no generated `mod.rs` here and no
//! scan: the app's `src/components/` is walked by `rahti-build` because a file
//! dropped in it should not need a `mod` line, but a library is an ordinary
//! crate and says what it exports in the ordinary way.

pub mod button;
pub mod counter;

// Flat re-exports, so a page writes `use rahti_ui::Button;` without repeating
// the module name.
pub use button::Button;
pub use counter::Counter;

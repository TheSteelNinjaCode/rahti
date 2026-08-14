//! Serving `public/`.

use std::path::PathBuf;

#[cfg(test)]
#[path = "tests/statics.rs"]
mod tests;

/// Environment override for the asset root.
///
/// `ServeDir` resolves relative paths against the process working directory,
/// not the binary, so a release copied anywhere but the crate root would 404
/// on every asset without this.
pub const PUBLIC_DIR_ENV: &str = "RAHTI_PUBLIC_DIR";

/// Where static files live, `default` coming from `rahti.config.json`.
pub fn public_dir(default: &str) -> PathBuf {
    std::env::var_os(PUBLIC_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

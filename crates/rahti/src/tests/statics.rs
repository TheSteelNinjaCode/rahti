//! Where static files are read from.

use super::*;

/// `rahti.config.json` decides, and the generated router passes it in.
#[test]
fn the_configured_directory_is_used() {
    // The override is a process-wide environment variable, so this asserts
    // the path every test run takes: nothing set, the default honoured.
    assert!(
        std::env::var_os(PUBLIC_DIR_ENV).is_none(),
        "{PUBLIC_DIR_ENV} is set in this shell"
    );
    assert_eq!(public_dir("public"), PathBuf::from("public"));
    assert_eq!(public_dir("assets/static"), PathBuf::from("assets/static"));
}

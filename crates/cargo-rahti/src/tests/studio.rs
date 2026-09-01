//! Unit tests for the Prisma Studio bridge. No test spawns npm or Prisma.

use super::*;

#[test]
fn dotenv_database_url_accepts_quotes_and_export() {
    assert_eq!(
        parse_database_url("OTHER=value\nexport DATABASE_URL='sqlite://./tasks.db?mode=rwc'\n"),
        Some("sqlite://./tasks.db?mode=rwc".to_string())
    );
}

#[test]
fn exported_database_url_wins_over_dotenv() {
    let root = std::env::temp_dir().join(format!("cargo-rahti-studio-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".env"), "DATABASE_URL=sqlite://./from-file.db").unwrap();
    assert_eq!(
        database_url(&root, Some("sqlite://./from-environment.db".to_string())).unwrap(),
        "sqlite://./from-environment.db"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn seaorm_urls_become_prisma_urls() {
    assert_eq!(
        prisma_url("sqlite://./app.db?mode=rwc").unwrap(),
        "file:./app.db"
    );
    assert_eq!(
        prisma_url("postgres://localhost/tasks").unwrap(),
        "postgresql://localhost/tasks"
    );
    assert_eq!(
        prisma_url("mysql://localhost/tasks").unwrap(),
        "mysql://localhost/tasks"
    );
}

#[test]
fn studio_invocation_does_not_let_npx_install_prisma() {
    let invocation = Invocation::new(Path::new("."), "file:./app.db".to_string(), Some(5556));
    assert_eq!(invocation.program, "npx");
    assert_eq!(
        invocation.args,
        [
            "--no-install",
            "prisma",
            "studio",
            "--url",
            "file:./app.db",
            "--port",
            "5556"
        ]
    );
}

#[test]
fn zero_is_not_a_studio_port() {
    assert!(Options::parse(&["--port", "0"]).is_err());
}

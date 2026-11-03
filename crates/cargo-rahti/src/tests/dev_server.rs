use super::*;

#[test]
fn a_stopped_server_says_what_to_do_next() {
    let out = describe(&json!({
        "running": false,
        "next": "Start the server with `cargo rahti restart`.",
    }));
    assert!(out.contains("Server: not running"), "{out}");
    assert!(out.contains("cargo rahti restart"), "{out}");
}

#[test]
fn a_stale_server_names_the_files_that_changed() {
    let out = describe(&json!({
        "running": true,
        "url": "http://localhost:3000",
        "pid": 7,
        "boot": "b",
        "changedFiles": ["src/app/page.rs"],
        "browser": { "routes": [], "errors": 0, "warnings": 0, "problems": [] },
        "jobs": null,
        "next": "Run restart.",
    }));
    assert!(out.contains("STALE"), "{out}");
    assert!(out.contains("src/app/page.rs"), "{out}");
    assert!(out.contains("nothing is verified"), "{out}");
    assert!(!out.contains("Jobs:"), "{out}");
}

#[test]
fn browser_routes_and_failed_jobs_are_spelled_out() {
    let out = describe(&json!({
        "running": true,
        "url": "http://localhost:3000",
        "pid": 7,
        "boot": "b",
        "changedFiles": [],
        "browser": {
            "routes": [{ "route": "/todo", "loads": 2, "errors": 1, "warnings": 0 }],
            "errors": 1,
            "warnings": 0,
            "problems": [{ "level": "uncaught", "route": "/todo", "message": "boom" }],
        },
        "jobs": {
            "durable": true,
            "handlers": ["send_receipt"],
            "metrics": { "succeeded": 3, "failed": 1 },
            "stored": { "pending": 2, "running": 0 },
            "storeError": null,
            "recentFailures": [{
                "name": "send_receipt",
                "outcome": "failed",
                "attempt": 3,
                "error": "smtp refused",
            }],
        },
        "next": "Load the routes.",
    }));
    assert!(out.contains("Source: current"), "{out}");
    assert!(out.contains("/todo loads=2 errors=1"), "{out}");
    assert!(out.contains("uncaught on /todo: boom"), "{out}");
    assert!(
        out.contains("1 handler(s), stored, 3 succeeded, 1 failed"),
        "{out}"
    );
    assert!(out.contains("2 pending"), "{out}");
    assert!(
        out.contains("send_receipt failed after 3 attempt(s): smtp refused"),
        "{out}"
    );
}

#[test]
fn the_binary_is_read_from_cargos_own_report() {
    let binary = json!({
        "reason": "compiler-artifact",
        "target": { "kind": ["bin"] },
        "executable": "/project/target/debug/app",
    });
    assert_eq!(
        artifact_executable(&binary.to_string()),
        Some(PathBuf::from("/project/target/debug/app"))
    );

    let library = json!({
        "reason": "compiler-artifact",
        "target": { "kind": ["lib"] },
        "executable": null,
    });
    assert_eq!(artifact_executable(&library.to_string()), None);
    assert_eq!(artifact_executable("not json"), None);
}

#[test]
fn a_timeout_option_is_read_and_anything_else_is_refused() {
    assert_eq!(
        timeout_from(&["--timeout", "5"]).unwrap(),
        Duration::from_secs(5)
    );
    assert!(timeout_from(&["--timeout"]).is_err());
    assert!(timeout_from(&["--force"]).is_err());
}

#[test]
fn an_mcp_command_that_is_not_installed_is_named_with_its_install() {
    let out = describe(&json!({
        "running": false,
        "next": "Start the server with `cargo rahti restart`.",
        "mcpMissing": ["rahti-mcp"],
    }));
    assert!(out.contains("`rahti-mcp`, which is not installed"), "{out}");
    assert!(out.contains("cargo install rahti-mcp --version"), "{out}");
}

#[test]
fn a_command_is_found_on_path_or_as_a_path() {
    let dir = std::env::temp_dir().join(format!("rahti-mcp-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = if cfg!(windows) { "tool.exe" } else { "tool" };
    std::fs::write(dir.join(file), "").unwrap();
    let path = std::env::join_paths([&dir]).unwrap();

    assert!(installed("tool", &path));
    assert!(!installed("rahti-mcp-nowhere", &path));
    assert!(installed(dir.join(file).to_str().unwrap(), &path));

    std::fs::write(
        dir.join(".mcp.json"),
        r#"{ "mcpServers": { "rahti": { "command": "rahti-mcp-nowhere", "args": [] } } }"#,
    )
    .unwrap();
    assert_eq!(
        missing_mcp_commands(&dir),
        vec!["rahti-mcp-nowhere".to_string()]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

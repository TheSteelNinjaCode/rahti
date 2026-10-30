use super::*;

const LIB: &str = "pub mod routes;\n\npub async fn initialize_application() {\n    Ok(ApplicationRuntime {\n        router,\n    })\n}\n";

fn project(name: &str, config: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("rahti-generate-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/migrations")).unwrap();
    fs::create_dir_all(root.join("src/models")).unwrap();
    fs::write(root.join("rahti.config.json"), config).unwrap();
    fs::write(root.join("src/lib.rs"), LIB).unwrap();
    root
}

#[test]
fn a_first_job_creates_the_module_and_starts_the_workers() {
    let root = project("first", "{}");
    job(&root, "send_receipt", &["--params", "order_id:i64,note"]).unwrap();

    let jobs = fs::read_to_string(root.join(JOBS_FILE)).unwrap();
    assert!(jobs.contains("use rahti::job;"), "{jobs}");
    assert!(jobs.contains("#[job]"), "{jobs}");
    assert!(
        jobs.contains("pub async fn send_receipt(order_id: i64, note: String)"),
        "{jobs}"
    );

    let lib = fs::read_to_string(root.join(LIB_FILE)).unwrap();
    assert!(lib.contains("pub mod jobs;"), "{lib}");
    assert!(lib.contains("rahti::jobs::Jobs::new().install();"), "{lib}");
    assert!(
        lib.find("install()").unwrap() < lib.find("Ok(ApplicationRuntime").unwrap(),
        "{lib}"
    );
}

#[test]
fn a_second_job_appends_and_changes_nothing_else() {
    let root = project("second", "{}");
    job(&root, "first_job", &[]).unwrap();
    let lib = fs::read_to_string(root.join(LIB_FILE)).unwrap();

    job(&root, "second_job", &[]).unwrap();
    let jobs = fs::read_to_string(root.join(JOBS_FILE)).unwrap();
    assert!(jobs.contains("pub async fn first_job()"), "{jobs}");
    assert!(jobs.contains("pub async fn second_job()"), "{jobs}");
    assert_eq!(jobs.matches("use rahti::job;").count(), 1, "{jobs}");
    assert_eq!(fs::read_to_string(root.join(LIB_FILE)).unwrap(), lib);
}

#[test]
fn an_existing_job_is_refused_rather_than_duplicated() {
    let root = project("duplicate", "{}");
    job(&root, "send", &[]).unwrap();
    let message = job(&root, "send", &[]).unwrap_err();
    assert!(message.contains("already defines `send`"), "{message}");
}

#[test]
fn a_store_needs_a_database() {
    let root = project("no-db", "{}");
    let message = job_store(&root).unwrap_err();
    assert!(message.contains("cargo rahti upgrade --db"), "{message}");
}

#[test]
fn a_store_writes_its_table_and_points_the_queue_at_it() {
    let root = project("store", r#"{"db":{"backend":"sqlite"}}"#);
    job_store(&root).unwrap();

    assert!(root.join(STORE_FILE).is_file());
    assert!(root.join("src/models/job.rs").is_file());
    let migrations = fs::read_dir(root.join("src/migrations")).unwrap().count();
    assert_eq!(migrations, 1);

    let lib = fs::read_to_string(root.join(LIB_FILE)).unwrap();
    assert!(lib.contains("pub mod job_store;"), "{lib}");
    assert!(lib.contains(".store(job_store::SeaJobStore)"), "{lib}");

    let message = job_store(&root).unwrap_err();
    assert!(message.contains("already exists"), "{message}");
}

#[test]
fn a_store_replaces_nothing_a_project_already_chose() {
    let chosen = "rahti::jobs::Jobs::new().store(MyStore).install();";
    assert_eq!(with_store(chosen), None);
    assert_eq!(with_install(chosen), None);
}

#[test]
fn the_shipped_store_is_the_one_this_workspace_proves() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The demonstration application exists only in the development workspace.
    if !root.join("src/job_store.rs").is_file() {
        return;
    }
    for (file, template) in [
        ("src/job_store.rs", templates::JOB_STORE_RS),
        ("src/models/job.rs", templates::MODEL_JOB_RS),
        (
            "src/migrations/m20261030_000001_create_job.rs",
            templates::MIGRATION_JOB,
        ),
    ] {
        let proven = fs::read_to_string(root.join(file))
            .unwrap()
            .replace('\r', "");
        assert_eq!(proven, template.replace('\r', ""), "{file} has drifted");
    }
}

#[test]
fn parameters_keep_generic_types_whole() {
    assert_eq!(
        parse_params("id:i64, tags:Vec<String>, pairs:Vec<(i32,i32)>, note").unwrap(),
        vec![
            ("id".to_string(), "i64".to_string()),
            ("tags".to_string(), "Vec<String>".to_string()),
            ("pairs".to_string(), "Vec<(i32,i32)>".to_string()),
            ("note".to_string(), "String".to_string()),
        ]
    );
    assert!(parse_params("name:&str").unwrap_err().contains("by value"));
    assert!(parse_params("Bad:i64").is_err());
}

#[test]
fn a_name_has_to_be_a_function_name() {
    for name in ["send_receipt", "_private", "job2"] {
        check_identifier(name).unwrap();
    }
    for name in ["SendReceipt", "send-receipt", "fn", "", "_", "2fast"] {
        assert!(check_identifier(name).is_err(), "{name}");
    }
}

#[test]
fn a_definition_is_found_by_its_exact_name() {
    let source = "#[job]\npub async fn send_receipt(id: i64) {}\n";
    assert!(defines(source, "send_receipt"));
    assert!(!defines(source, "send"));
}

#[test]
fn migration_dates_are_the_utc_calendar() {
    assert_eq!(civil_date(0), (1970, 1, 1));
    assert_eq!(civil_date(951_782_400), (2000, 2, 29));
    assert_eq!(civil_date(1_793_318_400), (2026, 10, 30));
}

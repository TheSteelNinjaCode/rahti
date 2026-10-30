use super::*;

fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("rahti-jobs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/jobs")).unwrap();
    root
}

#[test]
fn a_job_is_described_for_a_reader_that_runs_nothing() {
    let root = project("describe");
    fs::write(
        root.join("src/jobs.rs"),
        r#"
use rahti::job;

/// Not a job.
pub async fn helper() {}

#[job(name = "billing.receipt", attempts = 5, timeout_secs = 20)]
pub async fn send_receipt(order_id: i64, note: Option<String>) -> Result<(), JobError> {
    Ok(())
}
"#,
    )
    .unwrap();

    let scan = scan(&root);
    assert_eq!(scan.jobs.len(), 1);
    let job = &scan.jobs[0];
    assert_eq!(job["name"], "billing.receipt");
    assert_eq!(job["function"], "send_receipt");
    assert_eq!(job["file"], "src/jobs.rs");
    assert_eq!(job["line"], 8);
    assert_eq!(job["attempts"], 5);
    assert_eq!(job["timeoutSecs"], 20);
    assert!(job["backoffMs"].is_null(), "left to the queue's policy");
    assert_eq!(job["parameters"][0]["name"], "order_id");
    assert_eq!(job["parameters"][0]["rustType"], "i64");
    assert_eq!(
        job["enqueue"],
        "crate::jobs::send_receipt::enqueue(order_id, note).await"
    );
    assert!(scan.watched.contains(&"src/jobs.rs".to_string()));
}

#[test]
fn a_directory_of_jobs_is_read_with_its_module_paths() {
    let root = project("directory");
    fs::create_dir_all(root.join("src/jobs/mail")).unwrap();
    fs::write(
        root.join("src/jobs/mail/digest.rs"),
        "#[job] pub async fn digest() {}",
    )
    .unwrap();

    let scan = scan(&root);
    assert_eq!(
        scan.jobs[0]["enqueue"],
        "crate::jobs::mail::digest::digest::enqueue().await"
    );
    assert!(scan.watched.contains(&"src/jobs".to_string()));
}

#[test]
fn a_project_with_no_jobs_has_nothing_to_watch() {
    let root = std::env::temp_dir().join(format!("rahti-jobs-none-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    let scan = scan(&root);
    assert!(scan.jobs.is_empty());
    assert!(scan.watched.is_empty());
}

#[test]
fn source_the_compiler_will_refuse_is_left_to_the_compiler() {
    let root = project("broken");
    fs::write(root.join("src/jobs.rs"), "fn {").unwrap();
    assert!(scan(&root).jobs.is_empty());
}

#[test]
#[should_panic(expected = "two jobs are named `send`")]
fn two_jobs_cannot_share_a_stored_name() {
    let root = project("duplicate");
    fs::write(
        root.join("src/jobs.rs"),
        r#"
#[job] pub async fn send() {}
#[job(name = "send")] pub async fn other() {}
"#,
    )
    .unwrap();
    check_names(&scan(&root).jobs);
}

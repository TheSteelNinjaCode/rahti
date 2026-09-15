use super::*;
use axum::response::{Html as AxumHtml, Redirect};
use axum::routing::get;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

struct Tree(PathBuf);

impl Tree {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rahti-static-{label}-{}-{now}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn fixed_and_declared_dynamic_pages_are_exported_with_assets() {
    let tree = Tree::new("pages");
    let public = tree.0.join("public");
    let output = tree.0.join("dist");
    fs::create_dir_all(public.join("css")).unwrap();
    fs::write(public.join("css/app.css"), "body{}").unwrap();
    let router = Router::new()
        .route("/", get(|| async { AxumHtml("home") }))
        .route("/posts/{id}", get(|| async { AxumHtml("post") }));

    let report = export_static(
        router,
        vec![
            StaticRoute::fixed("/"),
            StaticRoute::dynamic(&["/posts/{id}"], ["/posts/1", "/posts/2"]),
        ],
        &public,
        &output,
    )
    .await
    .unwrap();

    assert_eq!(report.exported, ["/", "/posts/1", "/posts/2"]);
    assert_eq!(
        fs::read_to_string(output.join("index.html")).unwrap(),
        "home"
    );
    assert_eq!(
        fs::read_to_string(output.join("posts/1/index.html")).unwrap(),
        "post"
    );
    assert!(output.join("css/app.css").is_file());
    assert!(output.join(OUTPUT_MARKER).is_file());
}

#[tokio::test]
async fn unsafe_or_unavailable_routes_are_reported() {
    let tree = Tree::new("skips");
    let router = Router::new()
        .route("/private", get(|| async { Redirect::to("/signin") }))
        .route("/api", get(|| async { "plain" }));
    let report = export_static(
        router,
        vec![
            StaticRoute::fixed("/private"),
            StaticRoute::fixed("/api"),
            StaticRoute::undeclared(&["/posts/{id}"]),
            StaticRoute::dynamic(&["/users/{id}"], ["/elsewhere"]),
            StaticRoute::dynamic(&["/files/{id}"], ["/files/a:b"]),
        ],
        tree.0.join("public"),
        tree.0.join("dist"),
    )
    .await
    .unwrap();

    assert_eq!(report.exported.len(), 0);
    assert_eq!(report.skipped.len(), 5);
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.reason.contains("redirected"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.reason.contains("non-HTML"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.reason.contains("static_paths"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.reason.contains("does not match"))
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|skip| skip.reason.contains("safely"))
    );
}

#[tokio::test]
async fn a_non_rahti_directory_is_never_cleared() {
    let tree = Tree::new("ownership");
    let output = tree.0.join("dist");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("mine.txt"), "keep").unwrap();

    let error = export_static(Router::new(), Vec::new(), tree.0.join("public"), &output)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("refusing to clear"));
    assert_eq!(fs::read_to_string(output.join("mine.txt")).unwrap(), "keep");
}

#[test]
fn route_matching_understands_single_and_catch_all_segments() {
    assert!(matches_route("/posts/{id}", "/posts/42"));
    assert!(!matches_route("/posts/{id}", "/posts/42/edit"));
    assert!(matches_route("/docs/{*slug}", "/docs/one/two"));
    assert!(!matches_route("/docs/{*slug}", "/docs"));
}

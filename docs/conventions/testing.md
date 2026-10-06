# Testing Conventions

Every Rahti application starts with a working, area-organized test suite:
`cargo rahti new` writes a collector, a shared request harness, and the first
page and RPC test modules under `src/tests/`. Grow that suite with the
application — a feature is not finished until a test drives it the way a
browser would.

Run it with:

```text
cargo test
```

## Where Tests Live

- `src/tests/` holds the application's tests, grouped by cohesive area rather
  than one file per individual test. `src/lib.rs` includes one collector:

  ```rust
  #[cfg(test)]
  #[path = "tests/mod.rs"]
  mod app_tests;
  ```

  `src/tests/mod.rs` declares the area modules, while `support.rs` owns only
  the setup and request helpers genuinely shared by several areas:

  ```text
  src/tests/
  ├── mod.rs
  ├── support.rs
  ├── pages.rs
  ├── rpc.rs
  └── database.rs  # only when the project has a database
  ```

  The files remain modules in one library test binary. They retain
  crate-private access, share one-time setup, and do not relink the
  application once per area. Keep feature-specific helpers in their area
  file so `support.rs` does not become another oversized test file.

- A small unit test may sit beside the code it tests in an ordinary
  `#[cfg(test)] mod tests` block, as anywhere in Rust.

`src/tests/` rather than a root `tests/` directory, on purpose. The
application's startup lives in `src/lib.rs` so an optional native package
can call it, and its tests need crate-internal access — `crate::db`,
`crate::auth::settings()`, modules the library does not export. A root
`tests/` directory sees only public API, and every file there is a separate
test binary that links the application again. One module inside the library
sees everything and builds once.

`src/tests/` is not a route segment: it sits outside `src/app/`, so the
router never sees it.

## Running a Focused Area

The collector gives every test a module-qualified name. Run one area without
running the rest of the application suite:

```text
cargo test --lib app_tests::rpc
```

Run one test exactly when diagnosing a single behavior:

```text
cargo test --lib app_tests::rpc::the_rename_rpc_answers -- --exact
```

Splitting files organizes source and produces stable area filters; it does
not create process isolation. The modules still share globals, database
connections, and parallel test execution inside the same binary.

## Driving the Router

No server starts and no port binds. `crate::routes::router()` is the same
`axum::Router` the binary serves, and `oneshot` sends one request through
it — a request in, the finished page out:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get(url: &str) -> (StatusCode, String) {
    setup().await;

    let res = crate::routes::router()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .expect("the router answered");

    let status = res.status();
    let bytes = res.into_body().collect().await.expect("a body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}
```

The scaffolded `src/tests/support.rs` carries this helper, and the
dev-dependencies it needs — `tower` with the `util` feature, and
`http-body-util` — are already in the scaffolded `Cargo.toml`.

`setup()` is the suite's one-time installation, guarded by a
`tokio::sync::OnceCell` so the first request performs it and the rest wait.
Call it from the request helpers rather than from each test: a page is
often reached by tests that are not about what the setup installs, and a
handler that panicked for want of it would fail them for the wrong reason.

## Testing a Page

Assert the status and a fragment the page must render, not the whole
document — a test that snapshots every byte fails on every unrelated edit:

```rust
#[tokio::test]
async fn the_dashboard_renders() {
    let (status, body) = get("/dashboard").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<h1"), "{body}");
}
```

A URL no `page.rs` claims answers with the 404 page and
`StatusCode::NOT_FOUND`; that is worth one test so a routing mistake is
caught as a wrong status rather than a missing page.

## Testing an RPC

An `#[rpc]` is called on the URL of the page that declares it, and the call
is CSRF-protected: the page's response hands out a token cookie, and the
POST must send it back twice — as the cookie, and as the `x-csrf-token`
header. The scaffolded `rpc` helper performs the browser's flow exactly:
GET the page, keep the cookie, POST to the same URL with the function's
name in `x-pp-function`.

```rust
#[tokio::test]
async fn the_rename_rpc_answers() {
    let (status, body) = rpc("/settings", "rename", r#"{"name":"Ada"}"#).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Ada"), "{body}");
}
```

Every RPC deserves at least two tests: the success, and a malformed
payload — a missing field, a wrong type — asserting the call is refused
rather than answered. A function the page does not declare answers
`StatusCode::NOT_FOUND`, and a POST that carries no token pair answers
`StatusCode::FORBIDDEN`; both are framework behavior, so one test of each
per suite is plenty.

## Testing Auth

The guard the generated router installs reads the policy installed by
`rahti::auth::configure(...)`. `initialize_application` installs it in the
running application; the test suite must install it too, once, from
`setup()`:

```rust
rahti::auth::configure(crate::auth::settings());
```

A suite that skips this line exercises private routes that nothing
protects — every test passes, and the protection was never tested. The
scaffolded `setup()` marks where the line goes.

With the policy installed, a private route with no session answers
`StatusCode::SEE_OTHER` with a `location` header pointing at the sign-in
page and carrying the original destination:

```rust
#[tokio::test]
async fn a_private_route_redirects_a_signed_out_visitor() {
    let (status, body) = get("/account").await;

    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
}
```

An `#[rpc(auth)]` with no session is refused with
`StatusCode::UNAUTHORIZED`. Test both halves for anything private: the
guard decides who may load the page, the attribute decides who may make
the call, and neither substitutes for the other.

## Testing Against the Database

A project with a database tests against a real one: SQLite in a file, with
the application's own migrations run against it, connected once for the
whole test binary from `setup()`:

```rust
let path = std::env::temp_dir().join("my-app-tests.db");
let _ = std::fs::remove_file(&path);

let url = format!(
    "sqlite://{}?mode=rwc",
    path.display().to_string().replace('\\', "/")
);
crate::db::connect_to(&url).await.expect("a test database");
crate::db::migrate().await.expect("the migrations");
```

Two rules here are load-bearing; `database.md` explains both in full:

- **A file, not `:memory:`.** The connection is a process-wide static, but
  every `#[tokio::test]` builds its own runtime and drops it when the test
  ends. An in-memory database exists only while something is connected to
  it, so it vanishes with the first test's runtime and every later test
  fails with `no such table`. The symptom is tests that pass alone and
  fail together.
- **Delete the file on the way in, not on the way out.** A test binary has
  no reliable teardown, and starting from nothing is what makes a run
  repeatable.

Because the migrations run in `setup()`, any test that touches an entity
also proves the migration creates what the entity expects — a disagreement
between the two fails on the first query.

## Browser Tests

The router tests prove what the server sends. What PulsePoint then does with
it — state, bindings, effects, the DOM a library builds, what a click or a key
press changes — happens in a browser. `rahti::browser`, behind the
`browser-test` feature, drives one: it launches the installed Chrome,
Chromium or Edge headless, speaks the DevTools protocol to it, and needs no
WebDriver, Node, or browser download. Turn it on for tests only:

```toml
[dev-dependencies]
rahti = { version = "…", features = ["browser-test"] }
```

```rust
use rahti::browser::{Browser, serve};

#[tokio::test(flavor = "multi_thread")]
async fn the_counter_counts() {
    setup().await;
    let addr = serve(crate::routes::router()).await;
    let Some(mut browser) = Browser::launch().await.unwrap() else {
        return; // no browser installed: skipped, with a note on stderr
    };
    browser.goto(&format!("http://{addr}/")).await.unwrap();
    browser.click("button").await.unwrap();
    browser.wait_for("document.querySelector('p').textContent === '1'").await.unwrap();
    assert!(browser.errors().is_empty(), "{:?}", browser.errors());
}
```

- `goto`, `click`, `double_click`, `click_at`, `move_mouse`, `press`,
  `key_down`/`key_up` (hold a key), and `type_text` produce trusted input,
  the same kind a person does.
- `eval::<T>(js)` reads a value back; `wait_for(js)` polls a condition for up
  to 30 seconds and, when it gives up, names the condition and any page
  errors.
- `errors()` is every console error and uncaught exception so far. End a
  journey by asserting it is empty.
- `RAHTI_BROWSER` names a browser executable when the search does not find
  one. Without a browser, `launch` answers `None` and the test returns early,
  so the suite still passes on a machine that has none.

Test a journey — sign in, act, check what the page shows — and use the
browser for what only a browser can show; keep rules and data in the router
tests, which are faster and exact.

## What a Feature's Tests Cover

Adding a feature means adding its tests in the same change:

- a new page: it renders, and what it renders reflects its data;
- a new `#[rpc]`: the success, and a malformed payload refused;
- anything private: the route redirects without a session, and the
  `#[rpc(auth)]` refuses without one;
- a new table: an insert and a query through the entity, which exercises
  the migration for free;
- a bug fix: a test that fails on the old behavior;
- browser behavior a person would notice — a binding that must update, a
  window that must open and close, a library's DOM surviving a re-render:
  a browser test (see "Browser Tests" above).

Read `.rahti/dev.log` when something misbehaves in the browser; see
`diagnostics.md`.

//! The browser driver, against a small page served for the purpose.
//!
//! Every test skips (passes, with a note on stderr) when no Chrome, Chromium
//! or Edge is installed, so the suite still runs on a machine without one.

use axum::Router;
use axum::response::Html;
use axum::routing::get;

use super::*;

const PAGE: &str = r#"<!doctype html>
<html><body>
<button id="count" onclick="this.textContent = String(Number(this.textContent) + 1)">0</button>
<p id="key"></p>
<p id="moved">0</p>
<p id="trusted"></p>
<script>
  document.addEventListener('keydown', (e) => {
    document.getElementById('key').textContent = e.key + ':' + e.code;
  });
  document.addEventListener('mousemove', (e) => {
    const p = document.getElementById('moved');
    p.textContent = String(Number(p.textContent) + Math.abs(e.movementX));
  });
  document.getElementById('count').addEventListener('click', (e) => {
    document.getElementById('trusted').textContent = String(e.isTrusted);
  });
  console.error('a deliberate error');
</script>
</body></html>"#;

async fn page() -> Option<(Browser, String)> {
    let app = Router::new().route("/", get(|| async { Html(PAGE) }));
    let addr = serve(app).await;
    let browser = Browser::launch().await.expect("the browser starts")?;
    Some((browser, format!("http://{addr}/")))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_click_is_a_trusted_click_that_changes_the_page() {
    let Some((mut browser, url)) = page().await else { return };
    browser.goto(&url).await.unwrap();

    browser.click("#count").await.unwrap();
    browser.click("#count").await.unwrap();
    browser
        .wait_for("document.getElementById('count').textContent === '2'")
        .await
        .unwrap();
    let trusted: String = browser
        .eval("document.getElementById('trusted').textContent")
        .await
        .unwrap();
    assert_eq!(trusted, "true");
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_and_mouse_movement_reach_the_page() {
    let Some((mut browser, url)) = page().await else { return };
    browser.goto(&url).await.unwrap();

    browser.press("Escape").await.unwrap();
    browser
        .wait_for("document.getElementById('key').textContent === 'Escape:Escape'")
        .await
        .unwrap();
    browser.press("e").await.unwrap();
    browser
        .wait_for("document.getElementById('key').textContent === 'e:KeyE'")
        .await
        .unwrap();

    browser.move_mouse(100.0, 100.0).await.unwrap();
    browser.move_mouse(160.0, 100.0).await.unwrap();
    browser
        .wait_for("Number(document.getElementById('moved').textContent) >= 60")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn console_errors_are_collected() {
    let Some((mut browser, url)) = page().await else { return };
    browser.goto(&url).await.unwrap();
    // An evaluation after load, so the error's event has arrived.
    let _: bool = browser.eval("true").await.unwrap();
    assert!(
        browser.errors().iter().any(|e| e.text.contains("a deliberate error")),
        "{:?}",
        browser.console()
    );

    let _: Value = browser
        .eval("setTimeout(() => { throw new Error('uncaught here'); }); null")
        .await
        .unwrap();
    browser.pause(Duration::from_millis(200)).await;
    let _: bool = browser.eval("true").await.unwrap();
    assert!(
        browser
            .errors()
            .iter()
            .any(|e| e.level == "exception" && e.text.contains("uncaught here")),
        "{:?}",
        browser.console()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_say_what_went_wrong() {
    let Some((mut browser, url)) = page().await else { return };
    browser.goto(&url).await.unwrap();

    let thrown = browser.eval::<Value>("missingFunction()").await.unwrap_err();
    assert!(thrown.0.contains("missingFunction"), "{thrown}");

    let missing = browser.click("#nothing-here").await.unwrap_err();
    assert!(missing.0.contains("#nothing-here"), "{missing}");

    let waited = browser
        .wait_for_within("false", Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(waited.0.contains("never became true"), "{waited}");

    assert!(Key::of("NotAKey").is_err());
}

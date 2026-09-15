use super::*;
use crate::Html;
use axum::body::to_bytes;

#[test]
fn child_scalars_override_and_maps_merge() {
    let mut metadata = Metadata::new()
        .title("Home")
        .description("Default")
        .open_graph("site_name", "Rahti")
        .open_graph("title", "Home");
    metadata.merge(
        Metadata::new()
            .title("Docs")
            .open_graph("title", "Documentation"),
    );

    assert_eq!(metadata.rendered_title().as_deref(), Some("Docs"));
    assert_eq!(metadata.description.as_deref(), Some("Default"));
    assert_eq!(metadata.open_graph.get("site_name").unwrap(), "Rahti");
    assert_eq!(metadata.open_graph.get("title").unwrap(), "Documentation");
}

#[test]
fn inherited_templates_apply_except_to_absolute_titles() {
    let mut inherited = Metadata::new().title_template("{title} | Rahti");
    inherited.merge(Metadata::new().title("Docs"));
    assert_eq!(inherited.rendered_title().as_deref(), Some("Docs | Rahti"));

    inherited.merge(Metadata::new().absolute_title("Rahti status"));
    assert_eq!(inherited.rendered_title().as_deref(), Some("Rahti status"));
}

#[test]
fn a_default_title_does_not_apply_its_own_template() {
    let metadata = Metadata::new()
        .default_title("Rahti")
        .title_template("{title} | Rahti");
    assert_eq!(metadata.rendered_title().as_deref(), Some("Rahti"));
}

#[test]
fn metadata_is_escaped_and_uses_the_right_social_attributes() {
    let html = inject(
        "<html><head></head><body>x</body></html>".into(),
        Metadata::new()
            .title("A < B")
            .description("A \"quote\"")
            .canonical("https://example.test/?a=1&b=2")
            .robots(Robots::new().index(false).follow(false))
            .open_graph("title", "A & B")
            .twitter("card", "summary_large_image"),
    );

    assert!(html.contains("<title data-rahti-metadata=\"\">A &lt; B</title>"));
    assert!(html.contains("content=\"A &quot;quote&quot;\""));
    assert!(html.contains("href=\"https://example.test/?a=1&amp;b=2\""));
    assert!(html.contains("content=\"noindex, nofollow\""));
    assert!(html.contains("property=\"og:title\" content=\"A &amp; B\""));
    assert!(html.contains("name=\"twitter:card\""));
    assert!(html.contains("<template id=\"rahti-metadata\">"));
}

#[tokio::test]
async fn dynamic_metadata_overrides_static_metadata() {
    let dynamic = Page::new(
        Html::from_raw("<p>post</p>".into()),
        Metadata::new().title("Post 42"),
    )
    .into_response();
    let response = apply_metadata(
        Metadata::new().title("Blog").description("Latest posts"),
        dynamic,
    )
    .await;
    let resolved = &response.extensions().get::<ResolvedMetadata>().unwrap().0;

    assert_eq!(resolved.title.as_deref(), Some("Post 42"));
    assert_eq!(resolved.description.as_deref(), Some("Latest posts"));
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"<p>post</p>");
}

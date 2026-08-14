//! The session wire, and the policy that reads a path.
//!
//! The guard itself is exercised end-to-end against a real router in
//! `tests/end_to_end/router.rs`; what is here is the two halves it is built
//! from, where a failure says which half.

use super::*;

fn settings() -> AuthSettings {
    AuthSettings {
        secret: "test-secret".to_string(),
        cookie: "rahti_session".to_string(),
        validity: Duration::from_secs(3600),
        sliding: false,
        all_private: false,
        private_routes: Vec::new(),
        public_routes: vec!["/".to_string()],
        auth_routes: vec!["/signin".to_string()],
        signin: "/signin".to_string(),
        after_signin: "/dashboard".to_string(),
        after_signout: "/".to_string(),
    }
}

fn session(user: Value, in_secs: i64) -> Session {
    Session {
        user,
        expires: (now() as i64 + in_secs) as u64,
    }
}

// ------------------------------------------------------------------ cookie

#[test]
fn round_trips_a_payload() {
    let original = session(
        serde_json::json!({ "id": 7, "email": "ada@example.test" }),
        3600,
    );
    let cookie = encode(&original, "test-secret").expect("a cookie");

    let read = decode(&cookie, "test-secret").expect("a session");
    assert_eq!(read, original);
    assert_eq!(read.user()["email"], "ada@example.test");
}

#[test]
fn refuses_a_payload_edited_in_the_browser() {
    let cookie = encode(
        &session(serde_json::json!({ "id": 1 }), 3600),
        "test-secret",
    )
    .unwrap();
    let (body, tag) = cookie.split_once('.').unwrap();

    // The whole point of signing: the payload is readable, so someone will
    // read it, change the id and send it back.
    let forged: Value = serde_json::from_slice(&B64.decode(body).unwrap()).unwrap();
    let mut forged = forged.clone();
    forged["user"]["id"] = serde_json::json!(2);
    let body = B64.encode(serde_json::to_vec(&forged).unwrap());

    assert!(decode(&format!("{body}.{tag}"), "test-secret").is_none());
}

#[test]
fn refuses_a_cookie_from_another_secret() {
    let cookie = encode(&session(serde_json::json!({ "id": 1 }), 3600), "one").unwrap();
    assert!(decode(&cookie, "another").is_none());
}

#[test]
fn refuses_a_cookie_that_is_not_a_cookie() {
    for raw in ["", ".", "nonsense", "a.b", "%%%.%%%"] {
        assert!(decode(raw, "test-secret").is_none(), "{raw} decoded");
    }
}

#[test]
fn an_expired_session_knows_it() {
    assert!(session(serde_json::json!(1), -1).expired());
    assert!(!session(serde_json::json!(1), 60).expired());
}

#[test]
fn a_cleared_cookie_expires_immediately() {
    assert!(cleared_cookie_line("rahti_session").contains("Max-Age=0"));
    assert!(cleared_cookie_line("rahti_session").contains("HttpOnly"));
}

#[test]
fn the_session_cookie_is_not_readable_by_a_script() {
    // Unlike the CSRF cookie, which the client has to read. A script that
    // could read this one could take the session with it.
    let line = cookie_line("rahti_session", "value", now() + 60);
    assert!(line.contains("HttpOnly"));
    assert!(line.contains("SameSite=Lax"));
}

#[test]
fn reads_one_cookie_out_of_several() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("pp_csrf=abc; rahti_session=xyz; other=1"),
    );
    assert_eq!(
        read_cookie(&headers, "rahti_session").as_deref(),
        Some("xyz")
    );
    assert_eq!(read_cookie(&headers, "absent"), None);
}

// ------------------------------------------------------------ environment

/// The rule the production refusal is built on, tested without an environment
/// variable and without a process that exits.
#[test]
fn a_secret_that_is_not_a_secret_is_named_as_such() {
    assert_eq!(check_secret(""), Err(SecretProblem::Missing));
    assert_eq!(check_secret("   "), Err(SecretProblem::Missing));

    // The value `.env.example` ships, and the ones people reach for.
    for placeholder in ["change-me", "changeme", "CHANGE-ME", "Secret", "replace-me"] {
        assert_eq!(
            check_secret(placeholder),
            Err(SecretProblem::Placeholder),
            "{placeholder}"
        );
    }

    assert_eq!(check_secret("abc123"), Err(SecretProblem::TooShort));

    // What `cargo rahti new` writes: 32 random bytes as base64.
    assert!(check_secret("j0it2PS/rKI8Idz/JQgkH+TVqaJTHjv28gOpBhe2XnD5=").is_ok());
}

/// A placeholder long enough to clear the length floor is still a placeholder:
/// the two checks are independent, and the order matters.
#[test]
fn a_long_placeholder_is_still_refused() {
    assert_eq!(
        check_secret("your-secret-here"),
        Err(SecretProblem::Placeholder)
    );
}

#[test]
fn session_lifetime_is_whole_hours_greater_than_zero() {
    assert_eq!(parse_hours("7"), Some(7));
    assert_eq!(parse_hours("1"), Some(1));

    // A zero-hour session expires before the response carrying it lands, so
    // it is a typo rather than a way to turn sessions off.
    assert_eq!(parse_hours("0"), None);
    // The unit is the variable's name, not part of its value.
    assert_eq!(parse_hours("24h"), None);
    assert_eq!(parse_hours("-1"), None);
    assert_eq!(parse_hours("1.5"), None);
    assert_eq!(parse_hours(""), None);
    // Far enough out that the duration arithmetic stops meaning anything.
    assert_eq!(parse_hours("999999999999"), None);
}

/// A cookie name goes verbatim into `Set-Cookie`, and comes back out of a
/// `Cookie` header that is split on `;` and `=`. A name containing either can
/// never be read back, so the session silently never persists — and a name
/// containing `;` also injects whatever follows it as a cookie attribute.
///
/// Both failures are quiet, which is what makes this worth refusing rather
/// than passing through.
#[test]
fn a_cookie_name_that_is_not_a_token_is_refused() {
    for bad in [
        "my session",                   // space: not a token
        "sess;Path=/;Domain=evil.test", // attribute injection
        "sess=x",                       // splits where the value should
        "sess\tid",
        "sess,id",
        "sess\"id",
        "sess\\id",
        "sess@id",
        "sess\nid",
    ] {
        assert!(check_cookie_name(bad).is_err(), "`{bad}` was accepted");
    }

    // What the scaffold writes, and the framework's own default.
    assert!(check_cookie_name("8369362cd0046069").is_ok());
    assert!(check_cookie_name(DEFAULT_COOKIE).is_ok());
    // The characters a token may legitimately carry.
    assert!(check_cookie_name("my-app_session.v2").is_ok());
}

/// The names are the wire between `cargo rahti new`, `.env`, and this module.
/// They match Caspian's, so a project moving across reads the same way.
#[test]
fn the_environment_names_are_the_ones_the_scaffold_writes() {
    assert_eq!(SECRET_ENV, "AUTH_SECRET");
    assert_eq!(COOKIE_ENV, "AUTH_COOKIE_NAME");
    assert_eq!(LIFETIME_ENV, "SESSION_LIFETIME_HOURS");
}

// ----------------------------------------------------------------- require

/// What `#[rpc(auth)]` expands to, on its own.
///
/// Worth checking apart from the guard: the two refusals are independent, and
/// a call to an `#[rpc(auth)]` on a *public* page has only this one to stop it.
/// Outside a request there is no session, which is the case the shim sees when
/// the browser carries no cookie.
#[test]
fn require_refuses_when_there_is_no_session() {
    let refusal = require().expect_err("nobody is signed in here");
    assert_eq!(refusal.status(), StatusCode::UNAUTHORIZED);
    // JSON, not a redirect: `fetch` would follow a redirect and hand the
    // sign-in page's HTML to a caller expecting `{"error": …}`.
    assert!(refusal.headers().get(header::LOCATION).is_none());
    assert_eq!(
        refusal
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
}

/// And the session is read from the task, so asking outside a request answers
/// "nobody" rather than panicking — a 404 page renders below the router's
/// fallback, where there is no scope.
#[test]
fn the_session_outside_a_request_is_nobody() {
    // `super::` because the helper at the top of this file shadows the name.
    assert!(!is_authenticated());
    assert!(super::session().is_none());
    assert!(user::<Value>().is_none());
}

// ------------------------------------------------------------------ policy

#[test]
fn a_private_route_covers_its_subtree() {
    let policy = AuthSettings {
        private_routes: vec!["/account".to_string()],
        ..settings()
    };

    assert!(policy.is_private_route("/account"));
    assert!(policy.is_private_route("/account/billing"));
    assert!(policy.is_private_route("/account/billing/invoices"));
    assert!(!policy.is_private_route("/accounts"));
    assert!(!policy.is_private_route("/"));
}

#[test]
fn all_private_protects_everything_but_the_exceptions() {
    let policy = AuthSettings {
        all_private: true,
        public_routes: vec!["/".to_string(), "/pricing".to_string()],
        ..settings()
    };

    assert!(policy.is_private_route("/dashboard"));
    assert!(!policy.is_private_route("/"));
    assert!(!policy.is_private_route("/pricing"));
    assert!(!policy.is_private_route("/pricing/enterprise"));
    // The sign-in page, whichever way round the policy is.
    assert!(!policy.is_private_route("/signin"));
}

#[test]
fn root_as_a_public_route_does_not_make_everything_public() {
    let policy = AuthSettings {
        all_private: true,
        public_routes: vec!["/".to_string()],
        ..settings()
    };
    assert!(policy.is_private_route("/dashboard"));
}

#[test]
fn dynamic_segments_are_written_the_way_the_file_tree_writes_them() {
    for pattern in ["/customers/[id]", "/customers/{id}"] {
        assert!(pattern_matches(pattern, "/customers/42"));
        assert!(!pattern_matches(pattern, "/customers"));
        assert!(!pattern_matches(pattern, "/customers/42/orders"));
    }

    for pattern in ["/posts/[...id]", "/posts/{*id}"] {
        assert!(pattern_matches(pattern, "/posts/a"));
        assert!(pattern_matches(pattern, "/posts/a/b/c"));
        assert!(!pattern_matches(pattern, "/posts"));
    }

    // The optional catch-all matches the bare segment too.
    assert!(pattern_matches("/docs/[[...slug]]", "/docs"));
    assert!(pattern_matches("/docs/[[...slug]]", "/docs/a/b"));
    assert!(!pattern_matches("/docs/[[...slug]]", "/guide"));
}

#[test]
fn a_trailing_slash_is_the_same_route() {
    let policy = AuthSettings {
        private_routes: vec!["/account/".to_string()],
        ..settings()
    };
    assert!(policy.is_private_route("/account"));
    assert!(policy.is_private_route("/account/"));
}

// ---------------------------------------------------------------- redirects

#[test]
fn next_cannot_leave_the_origin() {
    // The classic open redirect: `?next=` is written by whoever sends the
    // link, so a sign-in that followed it anywhere would be a phishing hop.
    for hostile in [
        "https://evil.test/steal",
        "//evil.test/steal",
        "\\\\evil.test",
        "javascript:alert(1)",
        "",
    ] {
        assert_eq!(
            safe_target(hostile, "/dashboard"),
            "/dashboard",
            "{hostile}"
        );
    }
}

#[test]
fn next_keeps_a_path_that_is_ours() {
    assert_eq!(safe_target("/account/billing", "/"), "/account/billing");
    assert_eq!(safe_target("/search?q=ada", "/"), "/search?q=ada");
    assert_eq!(safe_target("/account/", "/"), "/account");
}

#[test]
fn a_path_survives_being_a_query_value() {
    let path = "/account/billing?tab=plan";
    let encoded = encode_query(path);
    assert!(!encoded.contains('?'));
    assert_eq!(
        query_param(Some(&format!("next={encoded}")), "next").as_deref(),
        Some(path)
    );
}

#[test]
fn an_absent_query_parameter_is_none() {
    assert_eq!(query_param(None, "next"), None);
    assert_eq!(query_param(Some("other=1"), "next"), None);
}

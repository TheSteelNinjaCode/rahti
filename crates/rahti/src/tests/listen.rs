//! The environment override, and the port fallback: forward to a free port,
//! and the end of the scan.

use super::*;

/// A platform names the port through the environment; a value is a value
/// however it is spaced.
#[test]
fn a_set_variable_is_the_value_it_carries() {
    assert_eq!(present(Some("8080".to_string())), Some("8080".to_string()));
    assert_eq!(
        present(Some("  8080 ".to_string())),
        Some("8080".to_string())
    );
}

/// A variable exported empty has not chosen anything, so the configured value
/// stands rather than the server trying to bind `host:`.
#[test]
fn a_blank_variable_is_unset() {
    assert_eq!(present(None), None);
    assert_eq!(present(Some(String::new())), None);
    assert_eq!(present(Some("   ".to_string())), None);
}

/// `PORT` is read as a number, `0` included — that asks the OS for a free
/// port rather than being rejected as out of range.
#[test]
fn a_port_variable_parses_as_a_port() {
    assert_eq!(parse_port("8080"), 8080);
    assert_eq!(parse_port("0"), 0);
    assert_eq!(parse_port("65535"), 65535);
}

/// The conflict message names where the port it is complaining about lives,
/// because the two answers are in different places.
#[test]
fn a_conflict_names_the_source_of_the_port() {
    assert_eq!(
        PortSource::Config.describe(),
        "`server.port` in rahti.config.json"
    );
    assert_eq!(
        PortSource::Environment.describe(),
        "the PORT environment variable"
    );
}

/// `0.0.0.0` is a sentence about listening, not a place to send a request —
/// a browser handed it refuses outright. The line a developer clicks has to
/// name the loopback the same server answers on.
#[test]
fn a_wildcard_bind_is_printed_as_localhost() {
    assert_eq!(
        url_of("0.0.0.0:3000".parse().unwrap()),
        "http://localhost:3000"
    );
    assert_eq!(
        url_of("[::]:8080".parse().unwrap()),
        "http://localhost:8080"
    );
}

/// Any other address is already a destination, and saying something else
/// about it would be a guess. IPv6 keeps the brackets a URL needs.
#[test]
fn a_real_address_is_printed_as_it_is() {
    assert_eq!(
        url_of("127.0.0.1:3000".parse().unwrap()),
        "http://127.0.0.1:3000"
    );
    assert_eq!(
        url_of("192.168.1.10:80".parse().unwrap()),
        "http://192.168.1.10:80"
    );
    assert_eq!(url_of("[::1]:3000".parse().unwrap()), "http://[::1]:3000");
}

/// The port is the one that actually bound, which after a dev-mode fallback
/// is not the one that was configured.
#[tokio::test]
async fn the_url_carries_the_port_that_bound() {
    let listener = TcpListener::bind("0.0.0.0:0")
        .await
        .expect("an OS-assigned port");
    let port = listener.local_addr().unwrap().port();

    assert_eq!(local_url(&listener), format!("http://localhost:{port}"));
}

/// A busy port falls forward to a later free one, never backward and never
/// to the busy port itself.
#[tokio::test]
async fn a_busy_port_falls_forward_to_a_free_one() {
    // An OS-assigned port, held so the scan starts from a genuinely busy one.
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();

    let fallback = next_free("127.0.0.1", port)
        .await
        .expect("a later port is free");
    let moved = fallback.local_addr().unwrap().port();

    assert!(moved > port, "{moved} is not past {port}");
}

/// The scan holds every listener it hands out, so two servers falling off the
/// same port cannot be given the same answer.
#[tokio::test]
async fn two_fallbacks_from_one_port_do_not_collide() {
    let busy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = busy.local_addr().unwrap().port();

    let first = next_free("127.0.0.1", port).await.expect("a free port");
    let second = next_free("127.0.0.1", port)
        .await
        .expect("another free port");

    assert_ne!(
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port()
    );
}

/// Ports end at 65535; the scan ends with them rather than wrapping around.
#[tokio::test]
async fn the_scan_stops_at_the_end_of_the_port_range() {
    assert!(next_free("127.0.0.1", u16::MAX).await.is_none());
}

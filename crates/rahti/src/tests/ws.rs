//! The socket wire, from both ends.
//!
//! The unit half exercises what needs no connection: argument reading, name
//! collisions, the origin rule. The end-to-end half serves [`routes`] on a
//! real listener and dials it with a real WebSocket client, because an
//! upgrade cannot be faked through `tower::Service` — the connection it
//! hands over only exists on a live socket.

use std::future::Future;
use std::pin::Pin;

use axum::http::HeaderMap;
use axum::http::header;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;

use super::*;

// ------------------------------------------------------------------- units

fn entry(name: &'static str) -> SocketEntry {
    SocketEntry {
        name,
        source: "here",
        auth: false,
        handler: |_, _| Box::pin(async {}),
    }
}

#[test]
fn duplicate_names_are_found() {
    let a = entry("feed");
    let b = entry("other");
    let c = entry("feed");

    assert!(duplicate(&[&a, &b]).is_none());
    let (first, second) = duplicate(&[&a, &b, &c]).expect("the pair");
    assert_eq!(first.name, "feed");
    assert_eq!(second.name, "feed");
}

#[test]
fn arg_reads_by_name() {
    let mut args = Map::new();
    args.insert("room".into(), json!("lobby"));
    args.insert("limit".into(), json!(3));

    assert_eq!(arg::<String>(&args, "chat", "room").unwrap(), "lobby");
    assert_eq!(arg::<u32>(&args, "chat", "limit").unwrap(), 3);
    // Absent reads as JSON null, so an `Option` parameter is optional.
    assert_eq!(arg::<Option<String>>(&args, "chat", "topic").unwrap(), None);
}

#[test]
fn arg_error_names_the_function_and_parameter() {
    let args = Map::new();
    let message = arg::<String>(&args, "chat", "room").unwrap_err();
    assert!(message.contains("`chat`"), "{message}");
    assert!(message.contains("`room`"), "{message}");
    assert!(message.contains("pp.socket"), "{message}");
}

#[test]
fn only_single_key_pp_objects_are_control_frames() {
    assert_eq!(control_frame(r#"{"__pp":"ping"}"#), Some(true));
    assert_eq!(control_frame(r#"{"__pp":"pong"}"#), Some(false));
    assert_eq!(control_frame(r#"{"__pp":17}"#), Some(false));
    assert_eq!(control_frame(r#"{"__pp":"ping","text":"hi"}"#), None);
    assert_eq!(control_frame(r#""{\"__pp\":\"ping\"}""#), None);
}

fn headers(origin: Option<&str>, host: Option<&str>) -> HeaderMap {
    let mut map = HeaderMap::new();
    if let Some(origin) = origin {
        map.insert(header::ORIGIN, origin.parse().unwrap());
    }
    if let Some(host) = host {
        map.insert(header::HOST, host.parse().unwrap());
    }
    map
}

#[test]
fn origin_must_match_host() {
    // No Origin is a non-browser client, which carries nothing to forge.
    assert!(check_origin(&headers(None, Some("localhost:3000"))).is_ok());

    // The scheme is not compared — a TLS-terminating proxy rewrites it.
    let ok = [
        ("http://localhost:3000", "localhost:3000"),
        ("https://localhost:3000", "localhost:3000"),
        ("http://LOCALHOST:3000", "localhost:3000"),
    ];
    for (origin, host) in ok {
        assert!(
            check_origin(&headers(Some(origin), Some(host))).is_ok(),
            "{origin} should be allowed on {host}"
        );
    }

    let refused = [
        ("http://evil.example", "localhost:3000"),
        ("http://localhost:4000", "localhost:3000"),
        ("http://localhost", "localhost:3000"),
    ];
    for (origin, host) in refused {
        assert!(
            check_origin(&headers(Some(origin), Some(host))).is_err(),
            "{origin} should be refused on {host}"
        );
    }

    // A request with an Origin but no Host says nothing about who it is —
    // refused rather than guessed.
    assert!(check_origin(&headers(Some("http://localhost:3000"), None)).is_err());
}

// ------------------------------------------------------------- end to end

/// What `#[socket]` would have written for
/// `async fn echo(label: String, mut socket: Socket)`.
fn echo_shim(args: Map<String, Value>, socket: Socket) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        let label = match arg::<String>(&args, "test_ws_echo", "label") {
            Ok(value) => value,
            Err(message) => return refuse(socket, message).await,
        };
        let mut socket = socket;
        while let Ok(Some(text)) = socket.recv::<String>().await {
            if !socket.send(&format!("{label}: {text}")).await {
                break;
            }
        }
    })
}

inventory::submit! {
    SocketEntry {
        name: "test_ws_echo",
        source: "src/tests/ws.rs",
        auth: false,
        handler: echo_shim,
    }
}

inventory::submit! {
    SocketEntry {
        name: "test_ws_private",
        source: "src/tests/ws.rs",
        auth: true,
        handler: |_, _| Box::pin(async {}),
    }
}

/// Serve [`routes`] on an ephemeral port and hand back its address.
async fn serve() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, routes()).await.unwrap();
    });
    addr
}

type Frame = tungstenite::Message;

#[tokio::test]
async fn echo_roundtrip() {
    let addr = serve().await;
    let (mut ws, _) = connect_async(format!("ws://{addr}{WS_PATH}?name=test_ws_echo"))
        .await
        .expect("handshake");

    // The first frame is the arguments.
    ws.send(Frame::text(r#"{"label":"you"}"#)).await.unwrap();
    ws.send(Frame::text(r#""hello""#)).await.unwrap();

    let frame = ws.next().await.unwrap().unwrap();
    assert_eq!(frame.into_text().unwrap().as_str(), r#""you: hello""#);

    // Closing from the browser side ends the conversation cleanly.
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn heartbeat_is_answered_without_reaching_the_typed_handler() {
    let addr = serve().await;
    let (mut ws, _) = connect_async(format!("ws://{addr}{WS_PATH}?name=test_ws_echo"))
        .await
        .expect("handshake");

    ws.send(Frame::text(r#"{"label":"you"}"#)).await.unwrap();
    ws.send(Frame::text(r#"{"__pp":"ping"}"#)).await.unwrap();
    ws.send(Frame::text(r#""hello""#)).await.unwrap();

    let pong = ws.next().await.unwrap().unwrap();
    assert_eq!(pong.into_text().unwrap().as_str(), r#"{"__pp":"pong"}"#);
    let echo = ws.next().await.unwrap().unwrap();
    assert_eq!(echo.into_text().unwrap().as_str(), r#""you: hello""#);
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn unknown_name_is_refused_before_upgrade() {
    let addr = serve().await;
    let error = connect_async(format!("ws://{addr}{WS_PATH}?name=nowhere"))
        .await
        .expect_err("no such socket");

    match error {
        tungstenite::Error::Http(response) => assert_eq!(response.status(), 404),
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn auth_socket_needs_a_session() {
    let addr = serve().await;
    let error = connect_async(format!("ws://{addr}{WS_PATH}?name=test_ws_private"))
        .await
        .expect_err("no session");

    match error {
        tungstenite::Error::Http(response) => assert_eq!(response.status(), 401),
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_first_frame_gets_the_error_shape() {
    let addr = serve().await;
    let (mut ws, _) = connect_async(format!("ws://{addr}{WS_PATH}?name=test_ws_echo"))
        .await
        .expect("handshake");

    // An array is not an argument object.
    ws.send(Frame::text("[1,2]")).await.unwrap();

    let frame = ws.next().await.unwrap().unwrap();
    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert!(
        value.get("error").and_then(Value::as_str).is_some(),
        "expected the error frame, got {value}"
    );
}

#[tokio::test]
async fn missing_argument_gets_the_error_shape() {
    let addr = serve().await;
    let (mut ws, _) = connect_async(format!("ws://{addr}{WS_PATH}?name=test_ws_echo"))
        .await
        .expect("handshake");

    ws.send(Frame::text("{}")).await.unwrap();

    let frame = ws.next().await.unwrap().unwrap();
    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    let message = value
        .get("error")
        .and_then(Value::as_str)
        .expect("the error frame");
    assert!(message.contains("`label`"), "{message}");
}

inventory::submit! {
    SocketEntry {
        name: "test_ws_refuses",
        source: "src/tests/ws.rs",
        auth: false,
        handler: |_, socket| Box::pin(async move {
            let sender = socket.sender();
            failed(
                &sender,
                crate::Error::new(axum::http::StatusCode::NOT_FOUND, "This worker is not running."),
            )
            .await;
        }),
    }
}

inventory::submit! {
    SocketEntry {
        name: "test_ws_breaks",
        source: "src/tests/ws.rs",
        auth: false,
        handler: |_, socket| Box::pin(async move {
            let sender = socket.sender();
            failed(&sender, crate::Error::internal("database password rejected")).await;
        }),
    }
}

async fn first_error_frame(name: &str) -> String {
    let addr = serve().await;
    let (mut ws, _) = connect_async(format!("ws://{addr}{WS_PATH}?name={name}"))
        .await
        .expect("handshake");
    ws.send(Frame::text("{}")).await.unwrap();
    let frame = ws.next().await.unwrap().unwrap();
    let value: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    value["error"].as_str().expect("the error frame").to_string()
}

/// An expected refusal reaches the browser with its public message.
#[tokio::test]
async fn a_handler_refusal_sends_its_public_message() {
    assert_eq!(first_error_frame("test_ws_refuses").await, "This worker is not running.");
}

/// An unexpected failure is redacted outside development, as an rpc's is.
#[tokio::test]
async fn an_unexpected_handler_failure_is_redacted_outside_development() {
    let message = first_error_frame("test_ws_breaks").await;
    if crate::dev::enabled() {
        assert!(message.contains("database password rejected"), "{message}");
    } else {
        assert!(!message.contains("database password"), "{message}");
    }
}

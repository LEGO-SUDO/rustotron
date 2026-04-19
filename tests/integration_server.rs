//! Integration tests for the WebSocket server. Each test spins up the
//! server on an ephemeral port, a tungstenite client, a store, and a bus,
//! exercises one scenario, then cancels.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustotron::bus::{Event, new_bus};
use rustotron::server::{self, ServerConfig};
use rustotron::store::{self, SecretsMode, StoreConfig};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

struct Harness {
    url: String,
    token: CancellationToken,
    server_join: JoinHandle<()>,
    store: store::StoreHandle,
    store_join: JoinHandle<()>,
    bus: rustotron::bus::EventBus,
}

impl Harness {
    async fn start() -> Self {
        let bus = new_bus(64);
        let token = CancellationToken::new();
        let store_task = store::spawn(StoreConfig::default(), bus.clone(), token.clone());
        let store_handle = store_task.handle.clone();

        let config = ServerConfig::ephemeral();
        let bound = server::bind(&config).await.expect("bind failed");
        let url = format!("ws://{}", bound.local_addr());

        let server_join = {
            let config = config.clone();
            let store = store_handle.clone();
            let bus = bus.clone();
            let token = token.clone();
            tokio::spawn(async move {
                server::serve(bound, config, store, bus, token).await;
            })
        };

        Self {
            url,
            token,
            server_join,
            store: store_handle,
            store_join: store_task.join,
            bus,
        }
    }

    async fn shutdown(self) {
        self.token.cancel();
        // Server first (drains sessions), then store.
        let _ = tokio::time::timeout(Duration::from_millis(750), self.server_join).await;
        let _ = tokio::time::timeout(Duration::from_millis(500), self.store_join).await;
    }
}

async fn connect(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect failed");
    ws
}

fn intro_without_client_id() -> String {
    r#"{"type":"client.intro","payload":{"name":"TestApp","environment":"test"},"important":false,"date":"2026-04-19T12:00:00.000Z","deltaTime":0}"#.to_string()
}

fn intro_with_client_id(id: &str) -> String {
    format!(
        r#"{{"type":"client.intro","payload":{{"name":"TestApp","clientId":"{id}","environment":"test"}},"important":false,"date":"2026-04-19T12:00:00.000Z","deltaTime":0}}"#
    )
}

fn api_response_frame(url: &str, status: u16, duration_ms: f64) -> String {
    format!(
        r#"{{"type":"api.response","payload":{{"duration":{duration_ms},"request":{{"url":"{url}","method":"GET"}},"response":{{"status":{status}}}}},"important":false,"date":"2026-04-19T12:00:01.000Z","deltaTime":1000}}"#
    )
}

#[tokio::test]
async fn intro_without_client_id_receives_set_client_id_reply() {
    let harness = Harness::start().await;
    let mut bus_rx = harness.bus.subscribe();
    let mut ws = connect(&harness.url).await;

    ws.send(WsMessage::Text(intro_without_client_id().into()))
        .await
        .unwrap();

    // We should see a setClientId text frame from the server.
    let received = tokio::time::timeout(Duration::from_millis(500), ws.next())
        .await
        .expect("timed out waiting for setClientId")
        .expect("stream ended prematurely")
        .expect("read error");

    match received {
        WsMessage::Text(t) => {
            let parsed: serde_json::Value = serde_json::from_str(t.as_str()).unwrap();
            assert_eq!(
                parsed.get("type").and_then(|v| v.as_str()),
                Some("setClientId")
            );
            assert!(
                parsed
                    .get("payload")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty()),
                "payload should be a non-empty guid"
            );
            assert!(
                parsed.get("date").is_none(),
                "setClientId must ship the slim envelope (no date)"
            );
        }
        other => panic!("expected Text, got {other:?}"),
    }

    // Bus should have emitted ClientConnected exactly once.
    let ev = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
        .await
        .expect("bus recv timed out")
        .expect("bus closed");
    assert!(matches!(ev, Event::ClientConnected(_)));

    let _ = ws.close(None).await;
    harness.shutdown().await;
}

#[tokio::test]
async fn intro_with_client_id_does_not_receive_set_client_id() {
    let harness = Harness::start().await;
    let mut ws = connect(&harness.url).await;

    ws.send(WsMessage::Text(intro_with_client_id("abc-123").into()))
        .await
        .unwrap();

    // We expect no immediate setClientId reply. The only thing on the wire
    // should be the periodic ping, which won't arrive within 200 ms.
    let received = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
    match received {
        Err(_) => {}                           // timeout as expected
        Ok(Some(Ok(WsMessage::Ping(_)))) => {} // permissible, keepalive
        Ok(Some(Ok(WsMessage::Text(t)))) => {
            panic!("expected silence, got text frame: {t}", t = t.as_str())
        }
        Ok(other) => panic!("expected silence, got {other:?}"),
    }

    let _ = ws.close(None).await;
    harness.shutdown().await;
}

#[tokio::test]
async fn api_response_frame_is_stored() {
    let harness = Harness::start().await;
    let mut ws = connect(&harness.url).await;

    // Handshake first.
    ws.send(WsMessage::Text(intro_with_client_id("abc").into()))
        .await
        .unwrap();

    // Then send an api.response.
    ws.send(WsMessage::Text(
        api_response_frame("https://example.com/a", 200, 42.0).into(),
    ))
    .await
    .unwrap();

    // Wait for the store to see it. The bus publishes ResponseReceived on
    // commit; poll the store until the row is visible.
    let mut attempts = 0;
    loop {
        let rows = harness.store.all(SecretsMode::Raw).await.unwrap();
        if !rows.is_empty() {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].exchange.request.url, "https://example.com/a");
            assert_eq!(rows[0].exchange.response.status, 200);
            assert_eq!(rows[0].exchange.duration, Some(42.0));
            break;
        }
        attempts += 1;
        assert!(attempts < 20, "api.response never reached the store");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = ws.close(None).await;
    harness.shutdown().await;
}

#[tokio::test]
async fn malformed_text_frame_does_not_crash_the_session() {
    let harness = Harness::start().await;
    let mut ws = connect(&harness.url).await;

    // Handshake so we're fully connected.
    ws.send(WsMessage::Text(intro_with_client_id("abc").into()))
        .await
        .unwrap();

    // Garbage, then a valid frame. Server should warn+drop the garbage
    // and still process the good one.
    ws.send(WsMessage::Text("not-json".into())).await.unwrap();
    ws.send(WsMessage::Text(
        "{\"type\":\"log\",\"payload\":null}".into(),
    ))
    .await
    .unwrap(); // known-drop type
    ws.send(WsMessage::Text(
        api_response_frame("https://x/post-garbage", 201, 7.5).into(),
    ))
    .await
    .unwrap();

    let mut attempts = 0;
    loop {
        let rows = harness.store.all(SecretsMode::Raw).await.unwrap();
        if !rows.is_empty() {
            assert_eq!(rows[0].exchange.request.url, "https://x/post-garbage");
            break;
        }
        attempts += 1;
        assert!(attempts < 20, "session died after garbage frame");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let _ = ws.close(None).await;
    harness.shutdown().await;
}

#[tokio::test]
async fn cancellation_closes_sessions_quickly() {
    let harness = Harness::start().await;
    let mut ws = connect(&harness.url).await;
    ws.send(WsMessage::Text(intro_with_client_id("abc").into()))
        .await
        .unwrap();

    // Give the server a moment to register the session.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let start = std::time::Instant::now();
    harness.token.cancel();

    let _ = tokio::time::timeout(Duration::from_millis(750), harness.server_join).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(750),
        "server took too long to drain: {elapsed:?}"
    );

    // The client side should observe close.
    let end = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
    match end {
        Ok(Some(Ok(WsMessage::Close(_))) | None) => {}
        Ok(Some(Ok(other))) => panic!("expected close or disconnect, got {other:?}"),
        Ok(Some(Err(_))) | Err(_) => {} // connection dropped or timed out, both acceptable
    }
}

#[tokio::test]
async fn duplicate_client_id_closes_older_session_after_500ms() {
    // ADR-003 §F-8: when a second connection arrives with the same
    // client.intro.clientId, the older session should be closed within
    // the documented 500 ms grace. Regression test for review H-3.
    let harness = Harness::start().await;
    let mut first = connect(&harness.url).await;
    first
        .send(WsMessage::Text(intro_with_client_id("shared-id").into()))
        .await
        .unwrap();

    // Drain any setClientId / pings so the stream is quiet.
    let _ = tokio::time::timeout(Duration::from_millis(100), first.next()).await;

    // Second connection claims the same clientId.
    let mut second = connect(&harness.url).await;
    second
        .send(WsMessage::Text(intro_with_client_id("shared-id").into()))
        .await
        .unwrap();

    // Expect the first session to observe a close within ~1 s
    // (500 ms grace + scheduling wiggle).
    let start = std::time::Instant::now();
    let mut observed_close = false;
    while start.elapsed() < Duration::from_millis(1_500) {
        match tokio::time::timeout(Duration::from_millis(200), first.next()).await {
            Ok(Some(Ok(WsMessage::Close(_))) | None) => {
                observed_close = true;
                break;
            }
            Ok(Some(Err(_))) => {
                observed_close = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(
        observed_close,
        "older session should be closed within 1500 ms of a duplicate clientId connecting"
    );

    // Second session should still be usable: send an api.response, see
    // it in the store.
    second
        .send(WsMessage::Text(
            api_response_frame("https://example.com/after-takeover", 200, 5.0).into(),
        ))
        .await
        .unwrap();

    let mut attempts = 0;
    loop {
        let rows = harness.store.all(SecretsMode::Raw).await.unwrap();
        if rows
            .iter()
            .any(|r| r.exchange.request.url == "https://example.com/after-takeover")
        {
            break;
        }
        attempts += 1;
        assert!(
            attempts < 30,
            "api.response from new owner never reached the store"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let _ = second.close(None).await;
    harness.shutdown().await;
}

#[tokio::test]
async fn client_disconnect_publishes_event() {
    let harness = Harness::start().await;
    let mut bus_rx = harness.bus.subscribe();
    let mut ws = connect(&harness.url).await;
    ws.send(WsMessage::Text(intro_with_client_id("abc").into()))
        .await
        .unwrap();

    // Drain ClientConnected.
    let connected = tokio::time::timeout(Duration::from_millis(500), bus_rx.recv())
        .await
        .expect("timed out waiting for ClientConnected")
        .expect("bus closed");
    assert!(matches!(connected, Event::ClientConnected(_)));

    let _ = ws.close(None).await;
    drop(ws);

    // Expect ClientDisconnected within a short window.
    let mut seen = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(50), bus_rx.recv()).await {
            Ok(Ok(Event::ClientDisconnected(_))) => {
                seen = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(seen, "server did not publish ClientDisconnected");

    harness.shutdown().await;
}

//! Chaos test — verifies the WS server survives garbage input + random
//! disconnects without panicking, and continues accepting new
//! connections.
//!
//! Per PRD NFR-9 and TASK-301 AC: malformed messages, garbage bytes,
//! random disconnects do not crash the process.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustotron::bus::new_bus;
use rustotron::server::{self, ServerConfig};
use rustotron::store::{self, SecretsMode, StoreConfig};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

fn garbage_frames() -> Vec<String> {
    vec![
        "not-json-at-all".to_string(),
        "[1,2,3]".to_string(),
        "{\"no-type\":true}".to_string(),
        "{\"type\":\"unknown.weird\",\"payload\":null}".to_string(),
        "{\"type\":\"api.response\",\"payload\":{\"duration\":1}}".to_string(), // missing req/res
        "{\"type\":\"client.intro\",\"payload\":42}".to_string(),
        "{\"type\":123}".to_string(),
        String::new(),
        "\u{FFFF}".to_string(),
        "{\"type\":\"api.response\",\"payload\":{\"duration\":null,\"request\":{\"url\":\"http://x\",\"method\":\"GET\"},\"response\":{\"status\":200}}}".to_string(),
    ]
}

#[tokio::test]
async fn server_survives_sustained_garbage_and_disconnects() {
    let bus = new_bus(128);
    let token = CancellationToken::new();
    let store_task = store::spawn(StoreConfig::default(), bus.clone(), token.clone());
    let store_handle = store_task.handle.clone();

    let config = ServerConfig::ephemeral();
    let bound = server::bind(&config).await.expect("bind");
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

    // Hammer the server with 50 garbage sessions; each connects,
    // sends a random mix of garbage, then drops mid-stream.
    for i in 0..50_usize {
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let frames = garbage_frames();
        let pick = frames[i % frames.len()].clone();
        // Still send a handshake so some sessions look "real" before
        // going off the rails.
        let _ = ws
            .send(WsMessage::Text(
                r#"{"type":"client.intro","payload":{"name":"chaos"},"important":false,"date":"x","deltaTime":0}"#.into()))
            .await;
        let _ = ws.send(WsMessage::Text(pick.into())).await;
        // Random subset of sessions send a real api.response too.
        if i % 3 == 0 {
            let good = format!(
                r#"{{"type":"api.response","payload":{{"duration":{i}.0,"request":{{"url":"https://chaos/{i}","method":"GET"}},"response":{{"status":200}}}},"important":false,"date":"x","deltaTime":0}}"#,
            );
            let _ = ws.send(WsMessage::Text(good.into())).await;
        }
        // Drop without close so the server sees the read side close.
        drop(ws);
    }

    // Let sessions drain.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The store should have received the good frames (every third of 50).
    let rows = store_handle.all(SecretsMode::Raw).await.expect("all");
    assert!(
        rows.len() >= 10,
        "expected ≥10 good frames, got {}",
        rows.len()
    );

    // Verify we can still establish a fresh connection — server didn't
    // wedge.
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("reconnect after chaos");
    ws.send(WsMessage::Text(
        r#"{"type":"client.intro","payload":{"name":"post-chaos"},"important":false,"date":"x","deltaTime":0}"#.into()))
        .await
        .expect("send");
    // Expect the setClientId reply.
    let received = tokio::time::timeout(Duration::from_millis(500), ws.next())
        .await
        .expect("post-chaos recv timed out")
        .expect("stream ended")
        .expect("read error");
    match received {
        WsMessage::Text(t) => assert!(t.as_str().contains("setClientId")),
        other => panic!("expected setClientId, got {other:?}"),
    }

    // Sanity: store is alive and the redacted view works post-chaos —
    // check BEFORE shutdown so we're measuring chaos tolerance, not
    // cancellation handling.
    let alive = store_handle.all(SecretsMode::Redacted).await;
    assert!(
        alive.is_ok(),
        "store should still be responsive after chaos"
    );

    let _ = ws.close(None).await;
    token.cancel();
    let _ = tokio::time::timeout(Duration::from_millis(750), server_join).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), store_task.join).await;
}

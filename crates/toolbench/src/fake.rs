//! A canned OpenAI-compatible server for tests: answers `POST
//! /v1/chat/completions` with the next queued (status, body) and records
//! every request; `GET /v1/models` always succeeds.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Shared {
    queue: Arc<Mutex<Vec<(u16, Value)>>>,
    seen: Arc<Mutex<Vec<Value>>>,
    delay: Option<Duration>,
}

pub struct FakeServer {
    addr: SocketAddr,
    shared: Shared,
}

impl FakeServer {
    pub async fn start(mut replies: Vec<(u16, Value)>) -> Self {
        replies.reverse();
        Self::serve(Shared {
            queue: Arc::new(Mutex::new(replies)),
            ..Shared::default()
        })
        .await
    }

    pub async fn start_slow(delay: Duration) -> Self {
        Self::serve(Shared {
            delay: Some(delay),
            ..Shared::default()
        })
        .await
    }

    async fn serve(shared: Shared) -> Self {
        let app = Router::new()
            .route(
                "/v1/models",
                get(|| async { Json(json!({"object": "list", "data": [{"id": "m"}]})) }),
            )
            .route("/v1/chat/completions", post(chat))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { addr, shared }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn requests(&self) -> Vec<Value> {
        self.shared.seen.lock().unwrap().clone()
    }
}

async fn chat(State(s): State<Shared>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    s.seen.lock().unwrap().push(body);
    if let Some(d) = s.delay {
        tokio::time::sleep(d).await;
    }
    let (status, reply) = s
        .queue
        .lock()
        .unwrap()
        .pop()
        .unwrap_or((200, text_reply("ok")));
    (StatusCode::from_u16(status).unwrap(), Json(reply))
}

/// A chat-completion body whose message content is `text`.
pub fn text_reply(text: &str) -> Value {
    json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
           "usage": {"completion_tokens": 3}})
}

/// A chat-completion body with one native tool call.
pub fn call_reply(name: &str, arguments: &str) -> Value {
    json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": null,
           "tool_calls": [{"id": "1", "type": "function", "function": {"name": name, "arguments": arguments}}]},
           "finish_reason": "tool_calls"}], "usage": {"completion_tokens": 7}})
}

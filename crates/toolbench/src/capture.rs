//! `toolbench capture`: a fake OpenAI server. Point a product at it, send
//! one message, and its first request that carries `tools` is saved as a
//! snapshot: exactly the tool list and system prompt the model would see.
//! Every request gets a short "ok" answer (SSE when `stream` is true) so
//! the product finishes its turn normally.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use aivyx_stencil::ToolSpec;
use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::snapshot::Snapshot;

pub fn snapshot_from_request(body: &Value, product: &str, source: &str) -> Result<Snapshot> {
    let Some(tools) = body
        .get("tools")
        .and_then(Value::as_array)
        .filter(|t| !t.is_empty())
    else {
        bail!("request has no tools");
    };
    let tools = tools
        .iter()
        .map(|t| {
            let f = &t["function"];
            Ok(ToolSpec {
                name: f
                    .get("name")
                    .and_then(Value::as_str)
                    .context("tool without a name")?
                    .to_string(),
                description: f
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input_schema: f
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let system_prompt = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|m| m.iter().find(|m| m["role"] == "system"))
        .and_then(|m| m["content"].as_str())
        .map(str::to_string);
    let snap = Snapshot {
        product: product.into(),
        source: source.into(),
        system_prompt,
        file_sha256: String::new(),
        tools,
    };
    snap.validate()?;
    Ok(snap)
}

#[derive(Clone)]
struct Shared {
    product: String,
    source: String,
    out: PathBuf,
    done: Arc<Mutex<bool>>,
}

pub async fn serve(
    listen: SocketAddr,
    product: String,
    source: String,
    out: PathBuf,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    eprintln!(
        "capture: listening on http://{listen}/v1 — point {product} at it and send one message (Ctrl-C to stop)"
    );
    serve_on(listener, product, source, out).await
}

async fn serve_on(
    listener: tokio::net::TcpListener,
    product: String,
    source: String,
    out: PathBuf,
) -> Result<()> {
    let shared = Shared {
        product,
        source,
        out,
        done: Arc::new(Mutex::new(false)),
    };
    let models = || async {
        Json(json!({"object": "list", "data": [{"id": "capture", "object": "model"}]}))
    };
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/models", get(models))
        // llama-server style readiness probe (pa's dev-run preflight needs it).
        .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
        .route("/v1/chat/completions", post(chat))
        .route("/chat/completions", post(chat))
        .with_state(shared);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn chat(State(s): State<Shared>, Json(body): Json<Value>) -> Response {
    let mut done = s.done.lock().await;
    if !*done && let Ok(snap) = snapshot_from_request(&body, &s.product, &s.source) {
        {
            if let Some(parent) = s.out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&s.out, serde_json::to_string_pretty(&snap).unwrap() + "\n") {
                Ok(()) => {
                    eprintln!(
                        "capture: saved {} tools → {}",
                        snap.tools.len(),
                        s.out.display()
                    );
                    *done = true;
                }
                Err(e) => eprintln!("capture: writing {}: {e}", s.out.display()),
            }
        }
    }
    if body["stream"] == json!(true) {
        let chunk = |delta: Value, finish: Value| {
            format!(
                "data: {}\n\n",
                json!({"id": "capture", "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
            )
        };
        let sse = chunk(json!({"role": "assistant", "content": "ok"}), Value::Null)
            + &chunk(json!({}), json!("stop"))
            + "data: [DONE]\n\n";
        ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
    } else {
        Json(json!({"id": "capture", "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}})).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> Value {
        json!({"model": "x", "stream": true, "messages": [
            {"role": "system", "content": "You are Aivyx."},
            {"role": "user", "content": "hi"}],
          "tools": [{"type": "function", "function": {"name": "fs.read", "description": "Read", "parameters": {"type": "object"}}},
                    {"type": "function", "function": {"name": "net.dns"}}]})
    }

    #[test]
    fn builds_a_snapshot_from_a_real_request() {
        let snap = snapshot_from_request(&request(), "pa", "test").unwrap();
        assert_eq!(snap.system_prompt.as_deref(), Some("You are Aivyx."));
        assert_eq!(snap.tools.len(), 2);
        assert_eq!(snap.tools[1].description, "");
        assert_eq!(snap.tools[1].input_schema, json!({"type": "object"}));
        assert!(snapshot_from_request(&json!({"messages": []}), "pa", "t").is_err());
    }

    #[tokio::test]
    async fn the_server_saves_the_first_request_with_tools_and_answers_streams() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pa.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let out2 = out.clone();
        tokio::spawn(async move {
            serve_on(listener, "pa".into(), "test".into(), out2)
                .await
                .unwrap()
        });
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/v1/chat/completions");
        // A request without tools (e.g. a title) is answered but not saved.
        let plain = client
            .post(&url)
            .json(&json!({"messages": [], "stream": false}))
            .send()
            .await
            .unwrap();
        assert!(plain.status().is_success());
        assert!(!out.exists());
        let streamed = client
            .post(&url)
            .json(&request())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(streamed.contains("data: [DONE]"), "{streamed}");
        let snap = Snapshot::load(&out).unwrap();
        assert_eq!(snap.tools[0].name, "fs.read");
        let models = client
            .get(format!("http://{addr}/v1/models"))
            .send()
            .await
            .unwrap();
        assert!(models.status().is_success());
        let health = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .unwrap();
        assert!(health.status().is_success());
    }
}

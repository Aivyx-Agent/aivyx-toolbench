//! `toolbench capture`: a fake OpenAI server. Point a product at it, send
//! one message, and its first request that carries `tools` is saved as a
//! snapshot: exactly the tool list and system prompt the model would see.
//! Every request gets a short "ok" answer (SSE when `stream` is true) so
//! the product finishes its turn normally.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write as _};
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

pub struct Redactor {
    home: Vec<String>,
    user: Option<String>,
}

impl Redactor {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let mut homes = Vec::new();
        if let Some(h) = &home {
            if let Ok(c) = std::fs::canonicalize(h) {
                let c = c.to_string_lossy().into_owned();
                if &c != h {
                    homes.push(c);
                }
            }
            homes.push(h.clone());
        }
        let user = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok())
            .or_else(|| {
                home.as_deref()
                    .and_then(|h| h.rsplit('/').next())
                    .map(str::to_string)
            })
            .filter(|u| u.len() >= 3 && u != "user" && u != "root");
        Self {
            home: homes.into_iter().filter(|h| h.len() > 1).collect(),
            user,
        }
    }

    #[cfg(test)]
    fn for_test(home: &str, user: &str) -> Self {
        Self {
            home: vec![home.to_string()],
            user: Some(user.to_string()),
        }
    }

    /// Replaces private details; returns the text and counts per kind.
    pub fn redact(&self, text: &str) -> (String, BTreeMap<&'static str, usize>) {
        let mut counts = BTreeMap::new();
        let mut s = text.to_string();
        let replace = |counts: &mut BTreeMap<&'static str, usize>,
                       s: &mut String,
                       re: &regex::Regex,
                       with: &str,
                       kind: &'static str| {
            let n = re.find_iter(s).count();
            if n > 0 {
                *s = re.replace_all(s, with).into_owned();
                *counts.entry(kind).or_insert(0) += n;
            }
        };
        for h in &self.home {
            let re = regex::Regex::new(&regex::escape(h)).unwrap();
            replace(&mut counts, &mut s, &re, "/home/user", "home path");
        }
        let url_user = regex::Regex::new(r"([a-z][a-z0-9+.-]*://)[^/\s@]+@").unwrap();
        replace(&mut counts, &mut s, &url_user, "$1", "url credentials");
        let email = regex::Regex::new(r"[\w.+-]+@[\w-]+\.[\w.]+").unwrap();
        // Keep the placeholder itself.
        let n = email
            .find_iter(&s)
            .filter(|m| m.as_str() != "user@example.com")
            .count();
        if n > 0 {
            s = email.replace_all(&s, "user@example.com").into_owned();
            *counts.entry("email").or_insert(0) += n;
        }
        let ip = regex::Regex::new(r"\b(\d{1,3})\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap();
        let keep = |c: &regex::Captures| {
            &c[1] == "127" || c[0].starts_with("192.0.2.") || &c[0] == "0.0.0.0"
        };
        let n = ip.captures_iter(&s).filter(|c| !keep(c)).count();
        if n > 0 {
            s = ip
                .replace_all(&s, |c: &regex::Captures| {
                    if keep(c) {
                        c[0].to_string()
                    } else {
                        "192.0.2.1".to_string()
                    }
                })
                .into_owned();
            *counts.entry("ip address").or_insert(0) += n;
        }
        if let Some(u) = &self.user {
            let re = regex::Regex::new(&format!(r"\b{}\b", regex::escape(u))).unwrap();
            replace(&mut counts, &mut s, &re, "user", "user name");
        }
        (s, counts)
    }
}

/// Lines worth a human look after redaction: token-like strings and
/// personal-data headings. (line number, up to 80 chars of context).
pub fn flags(text: &str) -> Vec<(usize, String)> {
    let token = regex::Regex::new(r"[A-Za-z0-9+/=_-]{32,}").unwrap();
    let personal =
        regex::Regex::new(r"(?i)\b(memory|memories|persona|about the user|learned|preferences)\b")
            .unwrap();
    text.lines()
        .enumerate()
        .filter(|(_, l)| token.is_match(l) || personal.is_match(l))
        .map(|(i, l)| (i + 1, l.chars().take(80).collect()))
        .collect()
}

/// Redacts the prompt and every tool description; counts are merged.
fn redacted_snapshot(
    mut snap: Snapshot,
    r: &Redactor,
) -> (Snapshot, BTreeMap<&'static str, usize>) {
    let mut total = BTreeMap::new();
    let mut merge = |counts: BTreeMap<&'static str, usize>| {
        for (k, n) in counts {
            *total.entry(k).or_insert(0) += n;
        }
    };
    if let Some(p) = snap.system_prompt.take() {
        let (p, c) = r.redact(&p);
        merge(c);
        snap.system_prompt = Some(p);
    }
    for t in &mut snap.tools {
        let (d, c) = r.redact(&t.description);
        merge(c);
        t.description = d;
    }
    if !total.is_empty() {
        let parts: Vec<String> = total
            .iter()
            .map(|(k, n)| format!("{k} \u{d7}{n}"))
            .collect();
        snap.source = format!("{}; redacted: {}", snap.source, parts.join(", "));
    }
    (snap, total)
}

/// Refuses a non-loopback listen address unless explicitly allowed.
pub fn check_listen(addr: SocketAddr, allow_remote: bool) -> Result<()> {
    if !addr.ip().is_loopback() {
        anyhow::ensure!(
            allow_remote,
            "--listen {addr} is not a loopback address; pass --allow-remote to expose the capture server"
        );
        eprintln!("capture: warning: listening on a non-loopback address ({addr})");
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CaptureState {
    Waiting,
    Reviewing,
    Done,
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    pub accept: bool,
    pub redact: bool,
}

#[derive(Clone)]
struct Shared {
    product: String,
    source: String,
    out: PathBuf,
    opts: CaptureOptions,
    state: Arc<Mutex<CaptureState>>,
}

/// Prints the review, decides, and saves or declines one snapshot.
async fn review(s: Shared, snap: Snapshot) {
    let snap = if s.opts.redact {
        let (snap, counts) = redacted_snapshot(snap, &Redactor::from_env());
        let parts: Vec<String> = counts
            .iter()
            .map(|(k, n)| format!("{k} \u{d7}{n}"))
            .collect();
        eprintln!(
            "capture: review: redacted {}",
            if parts.is_empty() {
                "nothing".into()
            } else {
                parts.join(", ")
            }
        );
        snap
    } else {
        eprintln!("capture: warning: --no-redact; private details may be saved");
        snap
    };
    let prompt = snap.system_prompt.as_deref().unwrap_or("");
    eprintln!(
        "capture: review: product {}, {} tools, system prompt {} chars",
        snap.product,
        snap.tools.len(),
        prompt.chars().count()
    );
    let mut flagged = Vec::new();
    for (n, c) in flags(prompt) {
        flagged.push(format!("system prompt line {n}: {c}"));
    }
    for t in &snap.tools {
        for (n, c) in flags(&t.description) {
            flagged.push(format!("{} line {n}: {c}", t.name));
        }
    }
    for f in &flagged {
        eprintln!("capture: review: flagged {f}");
    }
    let save = if s.opts.accept {
        true
    } else if std::io::stdin().is_terminal() {
        tokio::task::spawn_blocking(|| {
            eprint!("save this snapshot? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).is_ok()
                && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
        })
        .await
        .unwrap_or(false)
    } else {
        eprintln!("capture: not saved: re-run with --accept after reviewing");
        false
    };
    let mut state = s.state.lock().await;
    if save {
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
                *state = CaptureState::Done;
                return;
            }
            Err(e) => eprintln!("capture: writing {}: {e}", s.out.display()),
        }
    }
    *state = CaptureState::Waiting;
}

pub async fn serve(
    listen: SocketAddr,
    product: String,
    source: String,
    out: PathBuf,
    opts: CaptureOptions,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    eprintln!(
        "capture: listening on http://{listen}/v1 — point {product} at it and send one message (Ctrl-C to stop)"
    );
    serve_on(listener, product, source, out, opts).await
}

async fn serve_on(
    listener: tokio::net::TcpListener,
    product: String,
    source: String,
    out: PathBuf,
    opts: CaptureOptions,
) -> Result<()> {
    let shared = Shared {
        product,
        source,
        out,
        opts,
        state: Arc::new(Mutex::new(CaptureState::Waiting)),
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
    let snap = {
        let mut state = s.state.lock().await;
        match snapshot_from_request(&body, &s.product, &s.source) {
            Ok(snap) if *state == CaptureState::Waiting => {
                *state = CaptureState::Reviewing;
                Some(snap)
            }
            _ => None,
        }
    };
    if let Some(snap) = snap {
        tokio::spawn(review(s.clone(), snap));
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
            serve_on(
                listener,
                "pa".into(),
                "test".into(),
                out2,
                CaptureOptions {
                    accept: true,
                    redact: false,
                },
            )
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
        wait_for(&out).await;
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

    async fn wait_for(path: &std::path::Path) {
        for _ in 0..100 {
            if path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("{} was not written within 5 s", path.display());
    }

    async fn post_tools(accept: bool) -> (tempfile::TempDir, PathBuf, reqwest::StatusCode) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pa.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let out2 = out.clone();
        tokio::spawn(async move {
            serve_on(
                listener,
                "pa".into(),
                "test".into(),
                out2,
                CaptureOptions {
                    accept,
                    redact: true,
                },
            )
            .await
            .unwrap()
        });
        let status = reqwest::Client::new()
            .post(format!("http://{addr}/v1/chat/completions"))
            .json(&request())
            .send()
            .await
            .unwrap()
            .status();
        (dir, out, status)
    }

    #[tokio::test]
    async fn without_accept_nothing_is_saved() {
        let (_dir, out, status) = post_tools(false).await;
        assert!(status.is_success());
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn with_accept_the_snapshot_is_saved() {
        let (_dir, out, status) = post_tools(true).await;
        assert!(status.is_success());
        wait_for(&out).await;
        assert_eq!(Snapshot::load(&out).unwrap().tools.len(), 2);
    }

    #[test]
    fn redacted_snapshot_cleans_prompt_and_descriptions_and_notes_source() {
        let mut snap = snapshot_from_request(&request(), "pa", "test").unwrap();
        snap.system_prompt = Some("Home is /home/alice/x".into());
        snap.tools[0].description = "alice at a@b.co".into();
        let (snap, counts) = redacted_snapshot(snap, &Redactor::for_test("/home/alice", "alice"));
        assert_eq!(snap.system_prompt.as_deref(), Some("Home is /home/user/x"));
        assert_eq!(snap.tools[0].description, "user at user@example.com");
        assert_eq!(counts["home path"], 1);
        assert_eq!(counts["user name"], 1);
        assert!(snap.source.contains("redacted: "), "{}", snap.source);
        assert!(snap.source.contains("home path \u{d7}1"), "{}", snap.source);
    }

    #[test]
    fn redactor_replaces_private_details() {
        let r = Redactor::for_test("/home/alice", "alice");
        let (s, c) = r.redact("/home/alice/x alice said, malice");
        assert_eq!(s, "/home/user/x user said, malice");
        assert_eq!((c["home path"], c["user name"]), (1, 1));
        assert_eq!(r.redact("mail bob@corp.com").0, "mail user@example.com");
        let (s, c) = r.redact("10.1.2.3 127.0.0.1 192.0.2.7 0.0.0.0");
        assert_eq!(s, "192.0.2.1 127.0.0.1 192.0.2.7 0.0.0.0");
        assert_eq!(c["ip address"], 1);
        let (s, c) = r.redact("https://bob:pw@host/x");
        assert_eq!(s, "https://host/x");
        assert_eq!(c["url credentials"], 1);
        assert!(!c.contains_key("email"));
        // Placeholders survive.
        let (s, c) = r.redact("/home/user user@example.com");
        assert_eq!(s, "/home/user user@example.com");
        assert!(c.is_empty());
    }

    #[test]
    fn flags_find_tokens_and_personal_headings() {
        let text = format!("fine line\nkey {}\n## Learned skills\n", "a".repeat(40));
        let f = flags(&text);
        assert_eq!(f.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2, 3]);
        assert!(flags("a normal line").is_empty());
    }

    #[test]
    fn non_loopback_listen_needs_allow_remote() {
        let lo: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:1".parse().unwrap();
        assert!(check_listen(lo, false).is_ok());
        assert!(check_listen(any, false).is_err());
        assert!(check_listen(any, true).is_ok());
    }
}

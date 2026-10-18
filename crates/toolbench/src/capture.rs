//! `toolbench capture`: a fake OpenAI server. Point a product at it, send
//! one message, and its first request that carries `tools` is saved as a
//! snapshot: exactly the tool list and system prompt the model would see.
//! Every request gets a short "ok" answer (SSE when `stream` is true) so
//! the product finishes its turn normally.

use std::collections::BTreeMap;
use std::io::Write as _;
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
    skipped_user: Option<String>,
}

const GENERIC_NAMES: [&str; 5] = ["user", "root", "home", "example", "com"];

impl Redactor {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let mut homes = Vec::new();
        if let Some(h) = &home {
            if let Ok(c) = std::fs::canonicalize(h) {
                homes.push(c.to_string_lossy().into_owned());
            }
            homes.push(h.clone());
        }
        let candidates = std::env::var("USER")
            .ok()
            .into_iter()
            .chain(std::env::var("LOGNAME").ok())
            .chain(
                home.as_deref()
                    .and_then(|h| h.trim_end_matches('/').rsplit('/').next())
                    .map(str::to_string),
            )
            .collect();
        Self::new(homes, candidates)
    }

    /// `candidates` are tried in order; the first non-generic one is used.
    /// If all are generic, the first is remembered for a review warning.
    fn new(homes: Vec<String>, candidates: Vec<String>) -> Self {
        let mut home: Vec<String> = Vec::new();
        for h in homes {
            let h = h.trim_end_matches('/').to_string();
            if h.len() > 1 && !home.contains(&h) {
                home.push(h);
            }
        }
        let generic =
            |u: &str| u.chars().count() < 3 || GENERIC_NAMES.contains(&u.to_lowercase().as_str());
        let user = candidates.iter().find(|u| !generic(u)).cloned();
        let skipped_user = if user.is_none() {
            candidates.first().cloned()
        } else {
            None
        };
        Self {
            home,
            user,
            skipped_user,
        }
    }

    #[cfg(test)]
    fn for_test(home: &str, user: &str) -> Self {
        Self::new(vec![home.to_string()], vec![user.to_string()])
    }

    /// A review warning when the user name was too generic to redact.
    pub fn warning(&self) -> Option<String> {
        self.skipped_user
            .as_ref()
            .map(|u| format!("user name `{u}` too short/generic to redact"))
    }

    /// True when the text contains a home path or the user name
    /// (case-insensitive substring).
    fn mentions(&self, text: &str) -> bool {
        let t = text.to_lowercase();
        self.home.iter().any(|h| t.contains(&h.to_lowercase()))
            || self
                .user
                .as_ref()
                .is_some_and(|u| t.contains(&u.to_lowercase()))
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
            // The path must end at a boundary: `/home/al` is not `/home/alice`.
            let re = regex::Regex::new(&format!(
                r"{}(?:([^A-Za-z0-9_.-]|$)|([.-])([^A-Za-z0-9_]|$))",
                regex::escape(h)
            ))
            .unwrap();
            replace(
                &mut counts,
                &mut s,
                &re,
                "/home/user${1}${2}${3}",
                "home path",
            );
        }
        let url_user = regex::Regex::new(r"(?i)([a-z][a-z0-9+.-]*://)[^/?#\s@]+@").unwrap();
        replace(&mut counts, &mut s, &url_user, "$1", "url credentials");
        let email = regex::Regex::new(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)*\.[A-Za-z]{2,}").unwrap();
        // Keep the placeholder itself.
        let n = email
            .find_iter(&s)
            .filter(|m| m.as_str() != "user@example.com")
            .count();
        if n > 0 {
            s = email.replace_all(&s, "user@example.com").into_owned();
            *counts.entry("email").or_insert(0) += n;
        }
        let (ips, n) = redact_ips(&s);
        if n > 0 {
            s = ips;
            *counts.entry("ip address").or_insert(0) += n;
        }
        if let Some(u) = &self.user {
            let re = regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(u))).unwrap();
            replace(&mut counts, &mut s, &re, "user", "user name");
        }
        (s, counts)
    }
}

/// Replaces public-looking IPv4 addresses (not 127.x, 192.0.2.x, 0.0.0.0).
/// Letters or underscores may touch the address; digits and dots may not
/// (so `1.2.3.4.5` stays).
fn redact_ips(s: &str) -> (String, usize) {
    let re = regex::Regex::new(r"\d{1,3}(?:\.\d{1,3}){3}").unwrap();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut n = 0;
    for m in re.find_iter(s) {
        let before = s[..m.start()].chars().next_back();
        let after = s[m.end()..].chars();
        let mut it = after.clone();
        let glued_after = match it.next() {
            Some(c) if c.is_ascii_digit() => true,
            Some('.') => it.next().is_some_and(|c| c.is_ascii_digit()),
            _ => false,
        };
        let glued_before = before.is_some_and(|c| c.is_ascii_digit() || c == '.');
        let ip = m.as_str();
        let keep = ip.starts_with("127.") || ip.starts_with("192.0.2.") || ip == "0.0.0.0";
        if glued_before || glued_after || keep {
            continue;
        }
        out.push_str(&s[last..m.start()]);
        out.push_str("192.0.2.1");
        last = m.end();
        n += 1;
    }
    out.push_str(&s[last..]);
    (out, n)
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

/// Redacts every string leaf of a JSON value in place (keys untouched).
fn redact_value(
    v: &mut Value,
    r: &Redactor,
    merge: &mut impl FnMut(BTreeMap<&'static str, usize>),
) {
    match v {
        Value::String(s) => {
            let (t, c) = r.redact(s);
            merge(c);
            *s = t;
        }
        Value::Array(a) => a.iter_mut().for_each(|x| redact_value(x, r, merge)),
        Value::Object(o) => o.values_mut().for_each(|x| redact_value(x, r, merge)),
        _ => {}
    }
}

fn object_keys<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| object_keys(x, out)),
        Value::Object(o) => {
            for (k, x) in o {
                out.push(k);
                object_keys(x, out);
            }
        }
        _ => {}
    }
}

fn string_leaves<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| string_leaves(x, out)),
        Value::Object(o) => o.values().for_each(|x| string_leaves(x, out)),
        _ => {}
    }
}

/// Redacts the prompt, tool descriptions and schema string leaves; counts
/// are merged.
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
        redact_value(&mut t.input_schema, r, &mut merge);
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

/// Review lines for everything worth a human look in the (redacted)
/// snapshot: prompt, tool names, descriptions and schema string leaves.
fn review_flags(snap: &Snapshot, r: &Redactor) -> Vec<String> {
    let mut out = Vec::new();
    for (n, c) in flags(snap.system_prompt.as_deref().unwrap_or("")) {
        out.push(format!("system prompt line {n}: {c}"));
    }
    for t in &snap.tools {
        if r.mentions(&t.name) {
            out.push(format!("tool name mentions a private detail: {}", t.name));
        }
        let mut keys = Vec::new();
        object_keys(&t.input_schema, &mut keys);
        for k in keys.into_iter().filter(|k| r.mentions(k)) {
            out.push(format!(
                "{} schema key mentions a private detail: {k}",
                t.name
            ));
        }
        for (n, c) in flags(&t.name) {
            out.push(format!("tool name line {n}: {c}"));
        }
        for (n, c) in flags(&t.description) {
            out.push(format!("{} description line {n}: {c}", t.name));
        }
        let mut leaves = Vec::new();
        string_leaves(&t.input_schema, &mut leaves);
        for leaf in leaves {
            for (n, c) in flags(leaf) {
                out.push(format!("{} schema line {n}: {c}", t.name));
            }
        }
    }
    out
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
    /// Whether stdin is a terminal; decided once by the caller.
    pub interactive: bool,
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
    let r = Redactor::from_env();
    let snap = if s.opts.redact {
        let (snap, counts) = redacted_snapshot(snap, &r);
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
        if let Some(w) = r.warning() {
            eprintln!("capture: review: warning: {w}");
        }
        snap
    } else {
        eprintln!("capture: warning: --no-redact; private details may be saved");
        snap
    };
    eprintln!(
        "capture: review: product {}, {} tools, system prompt {} chars",
        snap.product,
        snap.tools.len(),
        snap.system_prompt.as_deref().unwrap_or("").chars().count()
    );
    for f in review_flags(&snap, &r) {
        eprintln!("capture: review: flagged {f}");
    }
    let save = if s.opts.accept {
        true
    } else if s.opts.interactive {
        let asked = tokio::task::spawn_blocking(|| {
            eprint!("save this snapshot? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).is_ok()
                && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
        })
        .await;
        match asked {
            Ok(yes) => yes,
            Err(e) => {
                eprintln!("capture: prompt failed ({e}); treating as no");
                false
            }
        }
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
    serve_on(
        listener,
        product,
        source,
        out,
        opts,
        Arc::new(Mutex::new(CaptureState::Waiting)),
    )
    .await
}

async fn serve_on(
    listener: tokio::net::TcpListener,
    product: String,
    source: String,
    out: PathBuf,
    opts: CaptureOptions,
    state: Arc<Mutex<CaptureState>>,
) -> Result<()> {
    let shared = Shared {
        product,
        source,
        out,
        opts,
        state,
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
                    interactive: false,
                },
                Arc::new(Mutex::new(CaptureState::Waiting)),
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

    async fn post_tools(
        accept: bool,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        reqwest::StatusCode,
        Arc<Mutex<CaptureState>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("pa.json");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let out2 = out.clone();
        let state = Arc::new(Mutex::new(CaptureState::Waiting));
        let state2 = state.clone();
        tokio::spawn(async move {
            serve_on(
                listener,
                "pa".into(),
                "test".into(),
                out2,
                CaptureOptions {
                    accept,
                    redact: true,
                    interactive: false,
                },
                state2,
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
        (dir, out, status, state)
    }

    /// Polls until the review has finished; returns the final state.
    async fn settled(state: &Arc<Mutex<CaptureState>>) -> CaptureState {
        for _ in 0..100 {
            let s = *state.lock().await;
            if s != CaptureState::Reviewing {
                return s;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("review did not finish within 5 s");
    }

    #[tokio::test]
    async fn without_accept_nothing_is_saved() {
        let (_dir, out, status, state) = post_tools(false).await;
        assert!(status.is_success());
        assert_eq!(settled(&state).await, CaptureState::Waiting);
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn with_accept_the_snapshot_is_saved() {
        let (_dir, out, status, state) = post_tools(true).await;
        assert!(status.is_success());
        assert_eq!(settled(&state).await, CaptureState::Done);
        assert_eq!(Snapshot::load(&out).unwrap().tools.len(), 2);
    }

    fn schema_snapshot() -> Snapshot {
        let mut snap = snapshot_from_request(&request(), "pa", "test").unwrap();
        snap.tools[0].input_schema = json!({"type": "object", "properties": {
            "path": {"description": "under /home/alice/docs",
                     "default": "/home/alice/x",
                     "enum": ["/home/alice/a", "bob@corp.com"]}},
            "required": ["path"]});
        snap
    }

    #[test]
    fn schema_string_leaves_are_redacted_and_keys_kept() {
        let (snap, counts) = redacted_snapshot(
            schema_snapshot(),
            &Redactor::for_test("/home/alice", "alice"),
        );
        let text = serde_json::to_string(&snap.tools[0].input_schema).unwrap();
        assert!(
            !text.contains("alice") && !text.contains("corp.com"),
            "{text}"
        );
        let p = &snap.tools[0].input_schema["properties"]["path"];
        assert_eq!(p["default"], "/home/user/x");
        assert_eq!(p["enum"][1], "user@example.com");
        assert_eq!(snap.tools[0].input_schema["required"][0], "path");
        assert_eq!(counts["home path"], 3);
        assert_eq!(counts["email"], 1);
    }

    #[test]
    fn review_flags_cover_schema_leaves_and_tool_names() {
        let mut snap = schema_snapshot();
        snap.tools[0].input_schema["properties"]["path"]["description"] =
            json!("## Learned skills");
        snap.tools[1].name = "t".repeat(40);
        let f = review_flags(&snap, &Redactor::for_test("/h/none", "xyz1"));
        assert!(f.iter().any(|l| l.contains("Learned")), "{f:?}");
        assert!(f.iter().any(|l| l.contains(&"t".repeat(40))), "{f:?}");
    }

    #[test]
    fn home_match_needs_a_path_boundary_and_ignores_trailing_slash() {
        let r = Redactor::for_test("/home/al", "xyz1");
        assert_eq!(r.redact("/home/alice/x").0, "/home/alice/x");
        let r = Redactor::for_test("/home/al/", "xyz1");
        assert_eq!(
            r.redact("/home/al/x and /home/al").0,
            "/home/user/x and /home/user"
        );
    }

    #[test]
    fn home_followed_by_sentence_punctuation_is_redacted() {
        let r = Redactor::new(
            vec!["/home/fakeal".into()],
            vec!["root".into(), "fakeal".into()],
        );
        assert_eq!(r.redact("Home is /home/fakeal.").0, "Home is /home/user.");
        assert_eq!(r.redact("see /home/fakeal- ok").0, "see /home/user- ok");
        // A different directory: only the user-name step touches it.
        assert_eq!(r.redact("/home/fakeal-old/x").0, "/home/user-old/x");
        assert_eq!(r.redact("/home/fakeal.txt").0, "/home/user.txt");
        assert!(r.warning().is_none());
    }

    #[test]
    fn generic_env_user_falls_back_to_the_home_dir_name() {
        let r = Redactor::new(
            vec!["/home/fakeal".into()],
            vec!["ab".into(), "fakeal".into()],
        );
        assert_eq!(r.redact("fakeal wrote").0, "user wrote");
        let r = Redactor::new(vec!["/home/x".into()], vec!["root".into(), "com".into()]);
        assert!(r.warning().unwrap().contains("`root`"));
        assert_eq!(r.redact("a com b").0, "a com b");
    }

    #[test]
    fn names_and_schema_keys_mentioning_private_details_are_flagged() {
        let r = Redactor::for_test("/home/alice", "alice");
        let mut snap = schema_snapshot();
        snap.tools[0].name = "read_Alice_notes".into();
        snap.tools[1].input_schema =
            json!({"type": "object", "properties": {"/home/alice/key": {"type": "string"}}});
        let f = review_flags(&snap, &r);
        assert!(f.iter().any(|l| l.contains("read_Alice_notes")), "{f:?}");
        assert!(f.iter().any(|l| l.contains("/home/alice/key")), "{f:?}");
        assert!(
            review_flags(&schema_snapshot(), &r)
                .iter()
                .all(|l| !l.contains("schema key"))
        );
    }

    #[test]
    fn url_userinfo_is_case_insensitive_and_stops_at_delimiters() {
        let r = Redactor::for_test("/h/none", "xyz1");
        assert_eq!(r.redact("HTTPS://bob:pw@host/x").0, "HTTPS://host/x");
        let (s, c) = r.redact("https://host?x=a@b.com");
        assert_eq!(s, "https://host?x=user@example.com");
        assert!(!c.contains_key("url credentials"));
    }

    #[test]
    fn email_has_no_trailing_dot_and_ignores_package_versions() {
        let r = Redactor::for_test("/h/none", "xyz1");
        assert_eq!(r.redact("npx pkg@1.2.3").0, "npx pkg@1.2.3");
        let (s, c) = r.redact("mail user@example.com.");
        assert_eq!(s, "mail user@example.com.");
        assert!(c.is_empty());
        assert_eq!(r.redact("a@b.co.uk.").0, "user@example.com.");
    }

    #[test]
    fn ipv4_glued_to_letters_is_redacted_but_versions_and_exclusions_stay() {
        let r = Redactor::for_test("/h/none", "xyz1");
        assert_eq!(r.redact("host10.0.0.5").0, "host192.0.2.1");
        assert_eq!(r.redact("a 10.0.0.1 10.0.0.2.").0, "a 192.0.2.1 192.0.2.1.");
        assert_eq!(r.redact("1.2.3.4.5").0, "1.2.3.4.5");
        assert_eq!(
            r.redact("127.0.0.1 192.0.2.7 0.0.0.0").0,
            "127.0.0.1 192.0.2.7 0.0.0.0"
        );
    }

    #[test]
    fn user_name_is_case_insensitive_and_generic_names_are_skipped() {
        let r = Redactor::for_test("/h/none", "Alice");
        assert_eq!(r.redact("ALICE and alice").0, "user and user");
        assert!(r.warning().is_none());
        for bad in ["ab", "root", "Home", "example", "user"] {
            let r = Redactor::for_test("/h/none", bad);
            assert_eq!(r.redact(&format!("{bad} here")).0, format!("{bad} here"));
            let w = r.warning().unwrap();
            assert!(
                w.contains(&format!("`{bad}`")) && w.contains("too short/generic"),
                "{w}"
            );
        }
    }

    #[test]
    fn source_suffix_lists_kinds_sorted_and_comma_separated() {
        let mut snap = snapshot_from_request(&request(), "pa", "test").unwrap();
        snap.system_prompt = Some("/home/alice a@b.co".into());
        let (snap, _) = redacted_snapshot(snap, &Redactor::for_test("/home/alice", "xyz1"));
        assert_eq!(
            snap.source,
            "test; redacted: email \u{d7}1, home path \u{d7}1"
        );
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
        assert!(check_listen("[::1]:1".parse().unwrap(), false).is_ok());
    }
}

//! The request body for one scenario. Mirrors what aivyx-pa sends: in
//! grammar mode no `tools`, the grammar instead, and the respond preamble
//! appended to the system prompt (Chapter Emboss); history is OpenAI-shaped
//! in both modes.

use serde_json::{Value, json};

use crate::config::{Endpoint, GrammarField, Mode};
use crate::scenario::{Message, Role, Scenario};
use crate::snapshot::Snapshot;

pub fn build_body(ep: &Endpoint, snap: &Snapshot, sc: &Scenario) -> Value {
    let constrain = ep.mode == Mode::Grammar;
    let mut messages = Vec::new();
    if let Some(system) =
        aivyx_stencil::system_message_for(snap.system_prompt.as_deref(), constrain)
    {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.extend(sc.messages.iter().map(message_json));
    let mut body = json!({
        "model": ep.model,
        "messages": messages,
        "stream": false,
        "max_tokens": ep.max_tokens,
    });
    match ep.mode {
        Mode::Native => {
            let tools: Vec<Value> = snap
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {"name": t.name, "description": t.description, "parameters": t.input_schema},
                }))
                .collect();
            body["tools"] = Value::Array(tools);
        }
        Mode::Grammar => {
            let grammar = aivyx_stencil::tool_call_grammar(&snap.tools);
            match ep.grammar_field {
                GrammarField::JsonSchema => body["json_schema"] = grammar,
                GrammarField::ResponseFormat => {
                    body["response_format"] = json!({
                        "type": "json_schema",
                        "json_schema": {"name": "tool_call", "schema": grammar},
                    })
                }
            }
        }
    }
    if let Some(t) = ep.temperature {
        body["temperature"] = json!(t);
    }
    for (k, v) in &ep.extra_body {
        body[k] = v.clone();
    }
    body
}

fn message_json(m: &Message) -> Value {
    match (m.role, &m.tool_call) {
        (Role::User, _) => json!({"role": "user", "content": m.content}),
        (Role::Assistant, Some(call)) => json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": call.id,
                "type": "function",
                "function": {"name": call.name, "arguments": call.arguments.to_string()},
            }],
        }),
        (Role::Assistant, None) => json!({"role": "assistant", "content": m.content}),
        (Role::Tool, _) => {
            json!({"role": "tool", "tool_call_id": m.tool_call_id, "content": m.content})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Endpoint, GrammarField, Mode};
    use crate::scenario::Scenario;
    use crate::snapshot::Snapshot;
    use aivyx_stencil::ToolSpec;
    use serde_json::json;

    fn ep(mode: Mode, grammar_field: GrammarField) -> Endpoint {
        Endpoint {
            name: "e".into(),
            base_url: "http://x/v1".into(),
            model: "m".into(),
            mode,
            grammar_field,
            max_tokens: 256,
            extra_body: Default::default(),
            temperature: None,
        }
    }

    fn snap() -> Snapshot {
        Snapshot {
            product: "pa".into(),
            source: "t".into(),
            system_prompt: Some("You are Aivyx.".into()),
            file_sha256: String::new(),
            tools: vec![ToolSpec {
                name: "fs.read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
        }
    }

    fn scenario() -> Scenario {
        toml::from_str(r#"
id = "s"
product = "pa"
category = "multi_step"
messages = [
  { role = "user", content = "Read a.md" },
  { role = "assistant", tool_call = { id = "c1", name = "fs.read", arguments = { path = "a.md" } } },
  { role = "tool", tool_call_id = "c1", content = "hi" },
]
[expect]
kind = "respond"
"#).unwrap()
    }

    #[test]
    fn temperature_is_sent_only_when_set() {
        let mut e = ep(Mode::Native, GrammarField::JsonSchema);
        let body = build_body(&e, &snap(), &scenario());
        assert!(body.get("temperature").is_none());
        e.temperature = Some(0.6);
        let body = build_body(&e, &snap(), &scenario());
        assert_eq!(body["temperature"], json!(0.6));
    }

    #[test]
    fn native_sends_tools_and_the_plain_system_prompt() {
        let body = build_body(
            &ep(Mode::Native, GrammarField::JsonSchema),
            &snap(),
            &scenario(),
        );
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["max_tokens"], json!(256));
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "You are Aivyx."})
        );
        assert_eq!(body["tools"][0]["function"]["name"], json!("fs.read"));
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["type"],
            json!("object")
        );
        assert!(body.get("json_schema").is_none());
        // History: the assistant call and the tool result, OpenAI-shaped.
        assert_eq!(body["messages"][2]["tool_calls"][0]["id"], json!("c1"));
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["arguments"],
            json!("{\"path\":\"a.md\"}")
        );
        assert_eq!(
            body["messages"][3],
            json!({"role": "tool", "tool_call_id": "c1", "content": "hi"})
        );
    }

    #[test]
    fn grammar_mode_sends_the_stencil_grammar_and_preamble_not_tools() {
        let body = build_body(
            &ep(Mode::Grammar, GrammarField::JsonSchema),
            &snap(),
            &scenario(),
        );
        assert!(body.get("tools").is_none());
        assert_eq!(
            body["json_schema"],
            aivyx_stencil::tool_call_grammar(&snap().tools)
        );
        let sys = body["messages"][0]["content"].as_str().unwrap();
        assert!(sys.starts_with("You are Aivyx.") && sys.contains(aivyx_stencil::RESPOND_PREAMBLE));
    }

    #[test]
    fn grammar_mode_can_use_response_format() {
        let body = build_body(
            &ep(Mode::Grammar, GrammarField::ResponseFormat),
            &snap(),
            &scenario(),
        );
        assert!(body.get("json_schema").is_none());
        assert_eq!(body["response_format"]["type"], json!("json_schema"));
        assert_eq!(
            body["response_format"]["json_schema"]["name"],
            json!("tool_call")
        );
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            aivyx_stencil::tool_call_grammar(&snap().tools)
        );
    }

    #[test]
    fn extra_body_fields_are_merged_into_the_request() {
        let mut e = ep(Mode::Grammar, GrammarField::ResponseFormat);
        e.extra_body.insert("enable_thinking".into(), json!(false));
        let body = build_body(&e, &snap(), &scenario());
        assert_eq!(body["enable_thinking"], json!(false));
        assert_eq!(body["model"], json!("m"));
    }
}

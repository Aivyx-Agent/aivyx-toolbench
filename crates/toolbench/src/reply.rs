//! What a model replied, extracted from a chat-completion body.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Mode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    /// The first tool call; `extra_calls` counts any further ones.
    ToolCall {
        name: String,
        arguments: Value,
        extra_calls: usize,
    },
    Text {
        text: String,
    },
    Unparseable {
        reason: String,
    },
}

fn unparseable(reason: impl Into<String>) -> Reply {
    Reply::Unparseable {
        reason: reason.into(),
    }
}

pub fn parse_reply(mode: Mode, body: &Value) -> Reply {
    let Some(message) = body.pointer("/choices/0/message") else {
        return unparseable("no choices[0].message in the response");
    };
    match mode {
        Mode::Native => {
            let calls = message.get("tool_calls").and_then(Value::as_array);
            match calls.filter(|c| !c.is_empty()) {
                Some(calls) => {
                    let function = &calls[0]["function"];
                    let Some(name) = function.get("name").and_then(Value::as_str) else {
                        return unparseable("tool call without a name");
                    };
                    let arguments = match function.get("arguments") {
                        None | Some(Value::Null) => json!({}),
                        Some(Value::String(s)) if s.trim().is_empty() => json!({}),
                        Some(Value::String(s)) => match serde_json::from_str(s) {
                            Ok(v) => v,
                            Err(e) => {
                                return unparseable(format!("tool arguments are not JSON: {e}"));
                            }
                        },
                        Some(v @ Value::Object(_)) => v.clone(),
                        Some(other) => return unparseable(format!("tool arguments are {other}")),
                    };
                    Reply::ToolCall {
                        name: name.to_string(),
                        arguments,
                        extra_calls: calls.len() - 1,
                    }
                }
                None => Reply::Text {
                    text: message
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                },
            }
        }
        Mode::Grammar => {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match aivyx_stencil::parse_constrained_output(content) {
                Some(aivyx_stencil::ConstrainedOutput::ToolCall { tool_name, input }) => {
                    Reply::ToolCall {
                        name: tool_name,
                        arguments: input,
                        extra_calls: 0,
                    }
                }
                Some(aivyx_stencil::ConstrainedOutput::Text(text)) => Reply::Text { text },
                None => unparseable("content is not a {name, arguments} object"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;
    use serde_json::{Value, json};

    fn native(message: Value) -> Value {
        json!({"choices": [{"index": 0, "message": message, "finish_reason": "stop"}]})
    }

    #[test]
    fn native_tool_call_with_string_arguments() {
        let body = native(json!({"role": "assistant", "content": null, "tool_calls": [
            {"id": "1", "type": "function", "function": {"name": "fs.read", "arguments": "{\"path\":\"a\"}"}},
            {"id": "2", "type": "function", "function": {"name": "fs.read", "arguments": "{}"}},
        ]}));
        assert_eq!(
            parse_reply(Mode::Native, &body),
            Reply::ToolCall {
                name: "fs.read".into(),
                arguments: json!({"path": "a"}),
                extra_calls: 1
            }
        );
    }

    #[test]
    fn native_object_arguments_and_empty_arguments() {
        let obj =
            native(json!({"tool_calls": [{"function": {"name": "a", "arguments": {"x": 1}}}]}));
        assert_eq!(
            parse_reply(Mode::Native, &obj),
            Reply::ToolCall {
                name: "a".into(),
                arguments: json!({"x": 1}),
                extra_calls: 0
            }
        );
        let empty = native(json!({"tool_calls": [{"function": {"name": "a", "arguments": ""}}]}));
        assert_eq!(
            parse_reply(Mode::Native, &empty),
            Reply::ToolCall {
                name: "a".into(),
                arguments: json!({}),
                extra_calls: 0
            }
        );
    }

    #[test]
    fn native_bad_arguments_are_unparseable() {
        let body =
            native(json!({"tool_calls": [{"function": {"name": "a", "arguments": "{oops"}}]}));
        assert!(matches!(
            parse_reply(Mode::Native, &body),
            Reply::Unparseable { .. }
        ));
        let nameless = native(json!({"tool_calls": [{"function": {"arguments": "{}"}}]}));
        assert!(matches!(
            parse_reply(Mode::Native, &nameless),
            Reply::Unparseable { .. }
        ));
    }

    #[test]
    fn native_text_reply() {
        let body = native(json!({"role": "assistant", "content": "4"}));
        assert_eq!(
            parse_reply(Mode::Native, &body),
            Reply::Text { text: "4".into() }
        );
    }

    #[test]
    fn grammar_replies_go_through_stencil() {
        let call =
            native(json!({"content": "{\"name\":\"fs.read\",\"arguments\":{\"path\":\"a\"}}"}));
        assert_eq!(
            parse_reply(Mode::Grammar, &call),
            Reply::ToolCall {
                name: "fs.read".into(),
                arguments: json!({"path": "a"}),
                extra_calls: 0
            }
        );
        let text =
            native(json!({"content": "{\"name\":\"respond\",\"arguments\":{\"text\":\"4\"}}"}));
        assert_eq!(
            parse_reply(Mode::Grammar, &text),
            Reply::Text { text: "4".into() }
        );
        let junk = native(json!({"content": "sure, here you go"}));
        assert!(matches!(
            parse_reply(Mode::Grammar, &junk),
            Reply::Unparseable { .. }
        ));
    }

    #[test]
    fn a_body_without_choices_is_unparseable() {
        assert!(matches!(
            parse_reply(Mode::Native, &json!({"error": "x"})),
            Reply::Unparseable { .. }
        ));
    }
}

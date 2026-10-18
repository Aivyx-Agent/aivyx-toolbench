//! aivyx-stencil — grammar-constrained tool calls (Chapter Stencil / Emboss, extracted from aivyx-pa).
//!
//! Grammar-constrained decoding forces a local model to emit a
//! **valid, real-named tool call by construction** instead of
//! hoping the prompt lands. [`tool_call_grammar`] turns the
//! turn's registered tools into a JSON Schema that admits *only*
//! a well-formed call — the constraint a provider then hands to
//! its decoder (for `mistralrs`, `Constraint::JsonSchema`).
//!
//! The function is **pure and provider-agnostic**: `&[ToolSpec]`
//! in, [`serde_json::Value`] out, no engine, no I/O. It lives at
//! the crate root (not under `mistral_rs/`) precisely so a future
//! `llama-server` `/completion` GBNF path can reuse it unchanged.
//!
//! ## Shape
//!
//! A `oneOf` discriminated union — one branch per tool, plus a
//! reserved [`RESPOND_SENTINEL`] branch:
//!
//! ```jsonc
//! { "oneOf": [
//!   { "type": "object",
//!     "properties": { "name": { "const": "fs.read" },
//!                     "parameters": { /* fs.read's input_schema verbatim */ } },
//!     "required": ["name", "parameters"], "additionalProperties": false },
//!   // …one branch per registered tool…
//!   { "type": "object",                                   // the sentinel
//!     "properties": { "name": { "const": "respond" },
//!                     "parameters": { "type": "object",
//!                                     "properties": { "text": { "type": "string" } },
//!                                     "required": ["text"] } },
//!     "required": ["name", "parameters"], "additionalProperties": false }
//! ] }
//! ```
//!
//! Each `name` is pinned to a `const` drawn from the registry, so
//! a **hallucinated name can never match**; that branch's
//! `parameters` is the matching tool's `input_schema` *verbatim*,
//! so **malformed parameters can never match**. Because every
//! branch's `name` const is distinct, an instance satisfies at
//! most one branch — `oneOf` (exactly-one) is the correct
//! combinator, and an unknown name or bad args satisfies *zero*
//! branches → invalid.
//!
//! ## The `respond` sentinel
//!
//! A constrained decoder is *forced* into the grammar, so without
//! an escape the model could never reply in plain text. The
//! reserved [`RESPOND_SENTINEL`] branch is that escape: the
//! provider (ST.3) unwraps a `{"name":"respond","parameters":{"text":…}}`
//! "call" back into an ordinary assistant text message rather than
//! dispatching it. The sentinel name has **no dot**, while every
//! real Aivyx tool is `namespace.action` — so it can never collide
//! with a registered tool.

use std::collections::HashMap;

use serde_json::{Value, json};

use serde::{Deserialize, Serialize};

/// One tool as a model sees it: the shape every product's tool list
/// converts to (aivyx-pa's `LlmToolDescriptor`, aivyx-coder's
/// `ToolDefinition`). `input_schema` is the tool's JSON Schema for its
/// arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Reserved pseudo-tool name the grammar admits so a constrained
/// model can decline to call a real tool and reply in plain text.
/// Has no `.` separator, so it cannot collide with a real
/// `namespace.action` tool name. ST.3's provider wiring unwraps a
/// call to this name back into an assistant text message.
pub const RESPOND_SENTINEL: &str = "respond";

/// Build a JSON Schema that admits **only** a valid tool call for
/// one of `tools` (or the [`RESPOND_SENTINEL`] text escape).
///
/// Pure and engine-free — the result is handed to a decoder as a
/// generation constraint. With an empty `tools` slice the grammar
/// still admits the sentinel, so a constrained turn can always
/// produce *something* well-formed.
///
/// Tool schemas may carry local `$defs` (or `definitions`). Their
/// definitions are hoisted to the grammar root, and `#/$defs/X` refs
/// are rewritten to point there, so a decoder can resolve them. Only
/// that shallow form is rewritten: deep refs (`#/$defs/X/...`), root
/// refs (`#`), `#/properties/...` refs and nested `$defs` inside a
/// definition are left as they are and may not resolve.
///
/// Object keys are emitted in serde_json's order, which is alphabetical
/// (no `preserve_order`), and decoders enforce that property order. The
/// tool input key is therefore named `parameters`, not `arguments`: it
/// sorts after `name`, so the model commits to a tool before writing its
/// input (with `arguments` it wrote the input first, then a name forced
/// to fit it).
pub fn tool_call_grammar(tools: &[ToolSpec]) -> Value {
    let mut branches: Vec<Value> = Vec::with_capacity(tools.len() + 1);
    let mut hoisted = serde_json::Map::new();
    for t in tools {
        let mut schema = t.input_schema.clone();
        if let Some(obj) = schema.as_object_mut() {
            let mut defs: Vec<(&'static str, String, Value)> = Vec::new();
            for (key, ns) in [("$defs", "$defs"), ("definitions", "definitions")] {
                if let Some(Value::Object(m)) = obj.remove(key) {
                    defs.extend(m.into_iter().map(|(n, d)| (ns, n, d)));
                }
            }
            if !defs.is_empty() {
                let tool = sanitize_tool_name(&t.name);
                let mut renames: RefRenames = HashMap::new();
                for (ns, n, _) in &defs {
                    let key = unique_def_key(&tool, n, &hoisted);
                    // Reserve the key now so later defs cannot reuse it.
                    hoisted.insert(key.clone(), Value::Null);
                    renames.insert((ns, n.clone()), key);
                }
                rewrite_refs(&mut schema, &renames);
                for (ns, n, mut d) in defs {
                    rewrite_refs(&mut d, &renames);
                    hoisted.insert(renames[&(ns, n)].clone(), d);
                }
            }
        }
        branches.push(tool_branch(&t.name, &schema));
    }
    branches.push(respond_branch());
    let mut grammar = json!({ "oneOf": branches });
    if !hoisted.is_empty() {
        grammar["$defs"] = Value::Object(hoisted);
    }
    grammar
}

/// (namespace, def name) -> new hoisted key.
type RefRenames = HashMap<(&'static str, String), String>;

/// Sanitised tool name used as the `$defs` key prefix.
fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `<tool>__<def>`, suffixed `__2`, `__3`, … until not in `taken`.
fn unique_def_key(tool: &str, def: &str, taken: &serde_json::Map<String, Value>) -> String {
    let base = format!("{tool}__{def}");
    let mut key = base.clone();
    let mut n = 2;
    while taken.contains_key(&key) {
        key = format!("{base}__{n}");
        n += 1;
    }
    key
}

/// Recursively rewrite local `#/$defs/<n>` / `#/definitions/<n>` refs
/// whose `<n>` is in `renames` to `#/$defs/<escaped new key>`.
fn rewrite_refs(v: &mut Value, renames: &RefRenames) {
    match v {
        Value::Object(m) => {
            for (k, val) in m.iter_mut() {
                if k == "$ref"
                    && let Value::String(r) = val
                {
                    let name = r
                        .strip_prefix("#/$defs/")
                        .map(|n| ("$defs", n))
                        .or_else(|| r.strip_prefix("#/definitions/").map(|n| ("definitions", n)));
                    if let Some((ns, name)) = name {
                        let name = name.replace("~1", "/").replace("~0", "~");
                        if let Some(new) = renames.get(&(ns, name)) {
                            *r = format!("#/$defs/{}", new.replace('~', "~0").replace('/', "~1"));
                        }
                    }
                } else {
                    rewrite_refs(val, renames);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rewrite_refs(x, renames)),
        _ => {}
    }
}

/// One union branch: `name` pinned to `name`, `parameters` set to
/// the tool's input schema verbatim.
fn tool_branch(name: &str, input_schema: &Value) -> Value {
    // Tools' input schemas are objects at the top level (the
    // registry validates this at registration). If a schema ever
    // isn't an object it can't be a meaningful `parameters`
    // constraint, so fall back to a permissive object — still
    // pinning the *name*, which is the load-bearing guarantee.
    let parameters = if input_schema.is_object() {
        input_schema.clone()
    } else {
        json!({ "type": "object" })
    };
    json!({
        "type": "object",
        "properties": {
            "name": { "const": name },
            "parameters": parameters,
        },
        "required": ["name", "parameters"],
        "additionalProperties": false,
    })
}

/// The reserved text-reply escape branch.
fn respond_branch() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "const": RESPOND_SENTINEL },
            "parameters": {
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false,
            },
        },
        "required": ["name", "parameters"],
        "additionalProperties": false,
    })
}

// ---------------------------------------------------------------------------
// Constrained-decoding I/O helpers — Chapter Emboss (EB.1).
//
// Shared by every provider that grammar-constrains tool calls (the
// in-process mistral.rs engine and llama.cpp's server). Both constrain
// output to the [`tool_call_grammar`] shape, so the model emits the
// same `{"name", "parameters"}` JSON in the message `content` — and both
// parse it the same way. These lived in the mistral.rs provider through
// Stencil/Bridle; Emboss promotes them here so there is one
// implementation, no drift.
// ---------------------------------------------------------------------------

/// The instruction appended to the system message under
/// grammar-constrained decoding (Chapter Bridle, BR.3). Stencil's
/// grammar *admits* the `respond` sentinel as the plain-text escape,
/// but a small model never *chooses* it unless told — ST.4 watched a 4B
/// loop on one tool call because it had no way to "finish." This is that
/// missing instruction.
pub const RESPOND_PREAMBLE: &str = "\
Tool-calling mode: every reply must be a single JSON object. To use a tool, emit \
{\"name\":\"<tool>\",\"parameters\":{…}}. To answer the user in plain text — or when \
you are done and need no tool — emit {\"name\":\"respond\",\"parameters\":{\"text\":\"…\"}}; \
this ends your turn. Do not repeat the same tool call: if a call did not help, either \
try a different one or `respond`.";

/// Build the system message for a turn, appending [`RESPOND_PREAMBLE`]
/// when decoding is constrained. Returns `None` when there is nothing
/// to send (no base prompt and not constrained) so the unconstrained
/// path stays byte-identical. Pure — unit-testable without an engine.
pub fn system_message_for(base: Option<&str>, constrain: bool) -> Option<String> {
    match (base, constrain) {
        (Some(s), false) => Some(s.to_string()),
        (None, false) => None,
        (Some(s), true) => Some(format!("{s}\n\n{RESPOND_PREAMBLE}")),
        (None, true) => Some(RESPOND_PREAMBLE.to_string()),
    }
}

/// Outcome of parsing a grammar-constrained turn's output (Chapter
/// Stencil ST.3). [`tool_call_grammar`] admits a single `{"name",
/// "parameters"}` object; the `respond` sentinel maps to plain text, any
/// other name to a real tool call.
#[derive(Debug, PartialEq)]
pub enum ConstrainedOutput {
    ToolCall { tool_name: String, input: Value },
    Text(String),
}

/// Parse the JSON a grammar-constrained turn produced. Returns `None`
/// when `content` isn't the expected shape — the grammar guarantees it
/// is, so `None` is purely defensive (the caller falls back to the
/// provider's native extraction). The tool input is read from
/// `parameters`, falling back to the legacy `arguments` key; `parameters`
/// wins if both are present.
pub fn parse_constrained_output(content: &str) -> Option<ConstrainedOutput> {
    let value: Value = serde_json::from_str(content.trim()).ok()?;
    let name = value.get("name")?.as_str()?;
    let arguments = value
        .get("parameters")
        .or_else(|| value.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if name == RESPOND_SENTINEL {
        let text = arguments
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();
        Some(ConstrainedOutput::Text(text))
    } else {
        Some(ConstrainedOutput::ToolCall {
            tool_name: name.to_string(),
            input: arguments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two sample tools with realistic object input schemas.
    fn sample_tools() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "fs.read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false,
                }),
            },
            ToolSpec {
                name: "web.fetch".to_string(),
                description: "Fetch a URL".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "url": { "type": "string" } },
                    "required": ["url"],
                    "additionalProperties": false,
                }),
            },
        ]
    }

    /// Compile the generated grammar into a validator. Asserts the
    /// grammar is itself a well-formed schema (the precondition for
    /// every other test).
    fn validator_for(tools: &[ToolSpec]) -> jsonschema::Validator {
        let grammar = tool_call_grammar(tools);
        jsonschema::validator_for(&grammar)
            .expect("tool_call_grammar must produce a valid JSON Schema")
    }

    #[test]
    fn admits_a_valid_tool_call() {
        let v = validator_for(&sample_tools());
        assert!(v.is_valid(&json!({
            "name": "fs.read",
            "parameters": { "path": "/etc/hosts" },
        })));
        // The second tool too — both branches are reachable.
        assert!(v.is_valid(&json!({
            "name": "web.fetch",
            "parameters": { "url": "https://example.com" },
        })));
    }

    #[test]
    fn rejects_an_unknown_tool_name() {
        let v = validator_for(&sample_tools());
        // The exact failure prompting could never fix: a name that
        // isn't in the registry. No branch's `const` matches.
        assert!(!v.is_valid(&json!({
            "name": "fs.write_file", // gemma4's classic hallucination
            "parameters": { "path": "x", "content": "y" },
        })));
        assert!(!v.is_valid(&json!({
            "name": "browser_navigate",
            "parameters": {},
        })));
    }

    #[test]
    fn rejects_malformed_arguments() {
        let v = validator_for(&sample_tools());
        // Real name, missing the required `path`.
        assert!(!v.is_valid(&json!({
            "name": "fs.read",
            "parameters": {},
        })));
        // Real name, wrong type for `path`.
        assert!(!v.is_valid(&json!({
            "name": "fs.read",
            "parameters": { "path": 123 },
        })));
        // Real name, an argument the schema doesn't allow.
        assert!(!v.is_valid(&json!({
            "name": "fs.read",
            "parameters": { "path": "/x", "bogus": true },
        })));
    }

    #[test]
    fn admits_the_respond_sentinel() {
        let v = validator_for(&sample_tools());
        assert!(v.is_valid(&json!({
            "name": RESPOND_SENTINEL,
            "parameters": { "text": "I've finished the task." },
        })));
        // …even with no tools registered — the escape is always
        // available so a constrained turn can produce plain text.
        let v_empty = validator_for(&[]);
        assert!(v_empty.is_valid(&json!({
            "name": "respond",
            "parameters": { "text": "hello" },
        })));
        // But the sentinel still has to be well-formed.
        assert!(!v_empty.is_valid(&json!({
            "name": "respond",
            "parameters": {},
        })));
    }

    #[test]
    fn rejects_missing_top_level_fields_and_cross_tool_args() {
        let v = validator_for(&sample_tools());
        // No `parameters` at all.
        assert!(!v.is_valid(&json!({ "name": "fs.read" })));
        // No `name` at all.
        assert!(!v.is_valid(&json!({ "parameters": { "path": "/x" } })));
        // fs.read's name with web.fetch's arguments — the name
        // const selects the fs.read branch, whose schema rejects
        // `url`/forbids the missing `path`.
        assert!(!v.is_valid(&json!({
            "name": "fs.read",
            "parameters": { "url": "https://example.com" },
        })));
    }

    // ---- Constrained-output parsing (moved here in Chapter Emboss EB.1) ----

    #[test]
    fn parse_constrained_real_tool_call() {
        let out = parse_constrained_output(
            r#"{"name": "fs.read", "parameters": {"path": "/etc/hosts"}}"#,
        )
        .expect("well-formed constrained call parses");
        assert_eq!(
            out,
            ConstrainedOutput::ToolCall {
                tool_name: "fs.read".to_string(),
                input: json!({"path": "/etc/hosts"}),
            }
        );
    }

    #[test]
    fn parse_constrained_respond_sentinel_unwraps_to_text() {
        let out =
            parse_constrained_output(r#"{"name": "respond", "parameters": {"text": "All done."}}"#)
                .expect("sentinel parses");
        assert_eq!(out, ConstrainedOutput::Text("All done.".to_string()));
    }

    #[test]
    fn parse_constrained_tolerates_surrounding_whitespace() {
        let out = parse_constrained_output(
            "\n  {\"name\": \"respond\", \"parameters\": {\"text\": \"hi\"}}\n",
        )
        .expect("trimmed JSON parses");
        assert_eq!(out, ConstrainedOutput::Text("hi".to_string()));
    }

    #[test]
    fn parse_constrained_rejects_non_json() {
        // Defensive: non-JSON / non-object content yields None so the
        // caller falls back to the provider's native extraction.
        assert!(parse_constrained_output("not json at all").is_none());
        assert!(parse_constrained_output(r#"{"missing": "name"}"#).is_none());
    }

    #[test]
    fn preamble_appended_only_when_constrained() {
        // Unconstrained → the operator prompt passes through untouched
        // (byte-identical), and an absent prompt stays absent.
        assert_eq!(
            system_message_for(Some("You are Aivyx."), false).as_deref(),
            Some("You are Aivyx.")
        );
        assert_eq!(system_message_for(None, false), None);

        // Constrained → the preamble is appended after the operator
        // prompt (appended, never replacing it).
        let got = system_message_for(Some("You are Aivyx."), true).unwrap();
        assert!(
            got.starts_with("You are Aivyx."),
            "operator prompt preserved"
        );
        assert!(got.contains(RESPOND_PREAMBLE), "preamble present");
        assert!(got.contains(RESPOND_SENTINEL), "names the respond sentinel");

        // Constrained with no base prompt → the preamble alone.
        assert_eq!(
            system_message_for(None, true).as_deref(),
            Some(RESPOND_PREAMBLE)
        );
    }

    #[test]
    fn tool_spec_round_trips_through_json() {
        let spec = sample_tools().remove(0);
        let text = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<ToolSpec>(&text).unwrap(), spec);
        assert!(
            serde_json::from_str::<ToolSpec>(
                r#"{"name":"a","description":"","input_schema":{},"extra":1}"#
            )
            .is_err()
        );
    }

    // ---- Local $defs hoisting ----

    fn tool(name: &str, schema: Value) -> ToolSpec {
        ToolSpec {
            name: name.to_string(),
            description: String::new(),
            input_schema: schema,
        }
    }

    fn call(name: &str, args: Value) -> Value {
        json!({ "name": name, "parameters": args })
    }

    fn set_tasks_schema(key: &str, prefix: &str) -> Value {
        json!({
            "type": "object",
            "properties": { "tasks": { "type": "array",
                "items": { "$ref": format!("#/{prefix}/TaskItem") } } },
            "required": ["tasks"],
            key: { "TaskItem": {
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            } },
        })
    }

    #[test]
    fn hoists_defs_into_grammar_root() {
        let mut tools = sample_tools();
        tools.push(tool("set_tasks", set_tasks_schema("$defs", "$defs")));
        let v = validator_for(&tools);
        assert!(v.is_valid(&call("set_tasks", json!({"tasks": [{"title": "a"}]}))));
        assert!(!v.is_valid(&call("set_tasks", json!({"tasks": [{"nope": 1}]}))));
        assert!(v.is_valid(&call("fs.read", json!({"path": "/x"}))));
        assert!(v.is_valid(&call("web.fetch", json!({"url": "u"}))));
        let g = tool_call_grammar(&tools);
        assert!(g["$defs"]["set_tasks__TaskItem"].is_object());
        let args = &g["oneOf"][2]["properties"]["parameters"];
        assert!(args.get("$defs").is_none());
    }

    #[test]
    fn hoists_legacy_definitions() {
        let mut tools = sample_tools();
        tools.push(tool(
            "set_tasks",
            set_tasks_schema("definitions", "definitions"),
        ));
        let v = validator_for(&tools);
        assert!(v.is_valid(&call("set_tasks", json!({"tasks": [{"title": "a"}]}))));
        assert!(!v.is_valid(&call("set_tasks", json!({"tasks": [{"nope": 1}]}))));
        assert!(v.is_valid(&call("fs.read", json!({"path": "/x"}))));
    }

    #[test]
    fn same_def_name_in_two_tools_does_not_clash() {
        let a = tool("a", set_tasks_schema("$defs", "$defs"));
        let b = tool(
            "b",
            json!({
                "type": "object",
                "properties": { "x": { "$ref": "#/$defs/TaskItem" } },
                "required": ["x"],
                "$defs": { "TaskItem": { "type": "integer" } },
            }),
        );
        let v = validator_for(&[a, b]);
        assert!(v.is_valid(&call("a", json!({"tasks": [{"title": "t"}]}))));
        assert!(!v.is_valid(&call("a", json!({"tasks": [1]}))));
        assert!(v.is_valid(&call("b", json!({"x": 3}))));
        assert!(!v.is_valid(&call("b", json!({"x": "s"}))));
    }

    #[test]
    fn colliding_sanitized_tool_names_get_unique_keys() {
        let mk = |n: &str| tool(n, set_tasks_schema("$defs", "$defs"));
        let g = tool_call_grammar(&[mk("fs.read"), mk("fs_read")]);
        let defs = g["$defs"].as_object().unwrap();
        assert!(defs.contains_key("fs_read__TaskItem"));
        assert!(defs.contains_key("fs_read__TaskItem__2"));
        jsonschema::validator_for(&g).expect("compiles");
    }

    #[test]
    fn tool_without_defs_branch_is_unchanged() {
        let tools = sample_tools();
        let g = tool_call_grammar(&tools);
        assert!(g.get("$defs").is_none());
        for (i, t) in tools.iter().enumerate() {
            assert_eq!(g["oneOf"][i], tool_branch(&t.name, &t.input_schema));
        }
    }

    #[test]
    fn nested_ref_between_sibling_defs_is_rewritten() {
        let t = tool(
            "t",
            json!({
                "type": "object",
                "properties": { "o": { "$ref": "#/$defs/Outer" },
                                "remote": { "$ref": "https://example.com/s.json" } },
                "required": ["o"],
                "$defs": {
                    "Outer": { "type": "object",
                        "properties": { "i": { "$ref": "#/$defs/Inner" } },
                        "required": ["i"] },
                    "Inner": { "type": "string" },
                },
            }),
        );
        let g = tool_call_grammar(std::slice::from_ref(&t));
        assert_eq!(
            g["$defs"]["t__Outer"]["properties"]["i"]["$ref"],
            "#/$defs/t__Inner"
        );
        assert_eq!(
            g["oneOf"][0]["properties"]["parameters"]["properties"]["remote"]["$ref"],
            "https://example.com/s.json"
        );
    }

    #[test]
    fn nested_ref_validates_end_to_end() {
        let t = tool(
            "t",
            json!({
                "type": "object",
                "properties": { "o": { "$ref": "#/$defs/Outer" } },
                "required": ["o"],
                "$defs": {
                    "Outer": { "type": "object",
                        "properties": { "i": { "$ref": "#/$defs/Inner" } },
                        "required": ["i"] },
                    "Inner": { "type": "string" },
                },
            }),
        );
        let v = validator_for(&[t]);
        assert!(v.is_valid(&call("t", json!({"o": {"i": "s"}}))));
        assert!(!v.is_valid(&call("t", json!({"o": {"i": 1}}))));
    }

    #[test]
    fn defs_and_definitions_with_one_name_stay_separate() {
        let t = tool(
            "t",
            json!({
                "type": "object",
                "properties": { "a": { "$ref": "#/$defs/X" },
                                "b": { "$ref": "#/definitions/X" } },
                "required": ["a", "b"],
                "$defs": { "X": { "type": "string" } },
                "definitions": { "X": { "type": "integer" } },
            }),
        );
        let v = validator_for(&[t]);
        assert!(v.is_valid(&call("t", json!({"a": "s", "b": 1}))));
        assert!(!v.is_valid(&call("t", json!({"a": 1, "b": 1}))));
        assert!(!v.is_valid(&call("t", json!({"a": "s", "b": "s"}))));
    }

    // ---- Name-first key order ----

    #[test]
    fn serialized_grammar_puts_name_before_parameters_in_every_branch() {
        let g = tool_call_grammar(&sample_tools());
        for branch in g["oneOf"].as_array().unwrap() {
            let text = serde_json::to_string(branch).unwrap();
            let n = text.find("\"name\":").expect("name key");
            let p = text.find("\"parameters\":").expect("parameters key");
            assert!(n < p, "name must precede parameters in {text}");
            assert_eq!(branch["required"], json!(["name", "parameters"]));
        }
    }

    #[test]
    fn old_arguments_key_is_rejected_by_the_grammar() {
        let v = validator_for(&sample_tools());
        assert!(!v.is_valid(&json!({
            "name": "fs.read",
            "arguments": { "path": "/etc/hosts" },
        })));
        assert!(!v.is_valid(&json!({
            "name": "respond",
            "arguments": { "text": "hi" },
        })));
    }

    #[test]
    fn parse_constrained_accepts_parameters_and_legacy_arguments() {
        let expect = ConstrainedOutput::ToolCall {
            tool_name: "fs.read".to_string(),
            input: json!({"path": "a"}),
        };
        assert_eq!(
            parse_constrained_output(r#"{"name":"fs.read","parameters":{"path":"a"}}"#),
            Some(expect)
        );
        assert_eq!(
            parse_constrained_output(r#"{"name":"fs.read","arguments":{"path":"old"}}"#),
            Some(ConstrainedOutput::ToolCall {
                tool_name: "fs.read".to_string(),
                input: json!({"path": "old"}),
            })
        );
        assert_eq!(
            parse_constrained_output(
                r#"{"name":"fs.read","arguments":{"path":"old"},"parameters":{"path":"new"}}"#
            ),
            Some(ConstrainedOutput::ToolCall {
                tool_name: "fs.read".to_string(),
                input: json!({"path": "new"}),
            })
        );
        assert_eq!(
            parse_constrained_output(r#"{"name":"respond","arguments":{"text":"old"}}"#),
            Some(ConstrainedOutput::Text("old".to_string()))
        );
    }
}

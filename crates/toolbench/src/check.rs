//! The five checks, in order (spec "Scoring").

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::reply::Reply;
use crate::scenario::{ArgCheck, ExpectKind, Scenario};
use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub parseable: bool,
    pub real_name: bool,
    pub schema_valid: bool,
    pub right_choice: bool,
    /// `None` unless the right tool was chosen and the scenario checks arguments.
    pub args_ok: Option<bool>,
}

impl Checks {
    /// Checks 1-3: a well-formed call to a real tool with valid arguments.
    pub fn valid_call(&self) -> bool {
        self.parseable && self.real_name && self.schema_valid
    }
}

pub fn check(reply: &Reply, sc: &Scenario, snap: &Snapshot) -> Checks {
    match reply {
        Reply::Unparseable { .. } => Checks::default(),
        Reply::Text { .. } => Checks {
            parseable: true,
            right_choice: sc.expect.kind == ExpectKind::Respond,
            ..Checks::default()
        },
        Reply::ToolCall {
            name, arguments, ..
        } => {
            let tool = snap.tool(name);
            let schema_valid = tool.is_some_and(|t| {
                jsonschema::validator_for(&t.input_schema).is_ok_and(|v| v.is_valid(arguments))
            });
            let right_choice = sc.expect.kind == ExpectKind::ToolCall
                && sc.expect.tools().contains(&name.as_str());
            let args_ok = (right_choice && !sc.expect.args.is_empty()).then(|| {
                sc.expect
                    .args
                    .iter()
                    .all(|(key, cond)| arg_passes(cond, arguments.get(key)))
            });
            Checks {
                parseable: true,
                real_name: tool.is_some(),
                schema_valid,
                right_choice,
                args_ok,
            }
        }
    }
}

fn arg_passes(cond: &ArgCheck, value: Option<&Value>) -> bool {
    if let Some(needle) = &cond.contains {
        return value
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains(needle.as_str()));
    }
    if let Some(expected) = &cond.equals {
        return value == Some(expected);
    }
    if let Some(present) = cond.present {
        return value.is_some() == present;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Snapshot;
    use aivyx_stencil::ToolSpec;
    use serde_json::{Value, json};

    fn snap() -> Snapshot {
        Snapshot {
            product: "pa".into(),
            source: "t".into(),
            system_prompt: None,
            tools: vec![ToolSpec {
                name: "fs.read".into(),
                description: "d".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"], "additionalProperties": false}),
            }],
        }
    }

    fn sc(expect: &str) -> Scenario {
        toml::from_str(&format!("id = \"s\"\nproduct = \"pa\"\ncategory = \"simple\"\nmessages = [{{ role = \"user\", content = \"x\" }}]\n[expect]\n{expect}")).unwrap()
    }

    fn call(name: &str, args: Value) -> Reply {
        Reply::ToolCall {
            name: name.into(),
            arguments: args,
            extra_calls: 0,
        }
    }

    #[test]
    fn a_perfect_call() {
        let c = check(
            &call("fs.read", json!({"path": "notes.md"})),
            &sc("kind = \"tool_call\"\ntool = \"fs.read\"\nargs.path = { contains = \"notes\" }"),
            &snap(),
        );
        assert_eq!(
            c,
            Checks {
                parseable: true,
                real_name: true,
                schema_valid: true,
                right_choice: true,
                args_ok: Some(true)
            }
        );
        assert!(c.valid_call());
    }

    #[test]
    fn hallucinated_name_and_bad_arguments() {
        let s = sc("kind = \"tool_call\"\ntool = \"fs.read\"");
        let fake = check(&call("fs.read_file", json!({"path": "a"})), &s, &snap());
        assert!(fake.parseable && !fake.real_name && !fake.schema_valid && !fake.right_choice);
        let missing = check(&call("fs.read", json!({})), &s, &snap());
        assert!(
            missing.real_name
                && !missing.schema_valid
                && missing.right_choice
                && !missing.valid_call()
        );
    }

    #[test]
    fn argument_conditions() {
        let r = call("fs.read", json!({"path": "a.md"}));
        let eq = check(
            &r,
            &sc("kind = \"tool_call\"\ntool = \"fs.read\"\nargs.path = { equals = \"b.md\" }"),
            &snap(),
        );
        assert_eq!(eq.args_ok, Some(false));
        let present = check(
            &r,
            &sc("kind = \"tool_call\"\ntool = \"fs.read\"\nargs.path = { present = true }"),
            &snap(),
        );
        assert_eq!(present.args_ok, Some(true));
        let absent = check(
            &r,
            &sc("kind = \"tool_call\"\ntool = \"fs.read\"\nargs.path = { present = false }"),
            &snap(),
        );
        assert_eq!(absent.args_ok, Some(false));
    }

    #[test]
    fn text_and_unparseable() {
        let respond = sc("kind = \"respond\"");
        let t = check(&Reply::Text { text: "4".into() }, &respond, &snap());
        assert!(t.parseable && t.right_choice && !t.valid_call());
        let wanted_call = check(
            &Reply::Text { text: "4".into() },
            &sc("kind = \"tool_call\"\ntool = \"fs.read\""),
            &snap(),
        );
        assert!(!wanted_call.right_choice);
        let u = check(
            &Reply::Unparseable { reason: "x".into() },
            &respond,
            &snap(),
        );
        assert_eq!(u, Checks::default());
        // Calling a tool when a text answer was expected is a wrong choice.
        assert!(!check(&call("fs.read", json!({"path": "a"})), &respond, &snap()).right_choice);
    }
}

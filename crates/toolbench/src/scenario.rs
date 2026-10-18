//! Hand-written, synthetic scenarios (`scenarios/**/*.toml`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::snapshot::Snapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Simple,
    SimilarTools,
    RequiredArgs,
    OptionalArgs,
    NoTool,
    MultiStep,
    RareTool,
    Misleading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// An earlier tool call in the conversation, made by the assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorCall {
    pub id: String,
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    pub tool_call: Option<PriorCall>,
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectKind {
    ToolCall,
    Respond,
}

/// Exactly one condition per argument.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgCheck {
    pub contains: Option<String>,
    pub equals: Option<Value>,
    pub present: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub kind: ExpectKind,
    pub tool: Option<String>,
    pub any_of: Option<Vec<String>>,
    #[serde(default)]
    pub args: BTreeMap<String, ArgCheck>,
}

impl Expect {
    /// The acceptable tool names (`tool` or `any_of`).
    pub fn tools(&self) -> Vec<&str> {
        self.tool
            .iter()
            .chain(self.any_of.iter().flatten())
            .map(String::as_str)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub product: String,
    pub category: Category,
    pub messages: Vec<Message>,
    pub expect: Expect,
}

pub fn validate(sc: &Scenario, snapshots: &BTreeMap<String, Snapshot>) -> Result<()> {
    let snap = snapshots
        .get(&sc.product)
        .with_context(|| format!("no snapshot for product `{}`", sc.product))?;
    ensure!(!sc.messages.is_empty(), "no messages");
    let mut call_ids = BTreeSet::new();
    for (i, m) in sc.messages.iter().enumerate() {
        match m.role {
            Role::User => ensure!(
                m.content.is_some() && m.tool_call.is_none() && m.tool_call_id.is_none(),
                "message {i}: a user message has `content` only"
            ),
            Role::Assistant => {
                ensure!(
                    m.content.is_some() != m.tool_call.is_some() && m.tool_call_id.is_none(),
                    "message {i}: an assistant message has `content` or `tool_call`, not both"
                );
                if let Some(call) = &m.tool_call {
                    ensure!(
                        snap.tool(&call.name).is_some(),
                        "message {i}: unknown tool `{}`",
                        call.name
                    );
                    ensure!(
                        call_ids.insert(call.id.clone()),
                        "message {i}: duplicate call id `{}`",
                        call.id
                    );
                }
            }
            Role::Tool => {
                let id = m
                    .tool_call_id
                    .as_deref()
                    .with_context(|| format!("message {i}: a tool message needs `tool_call_id`"))?;
                ensure!(
                    m.content.is_some() && m.tool_call.is_none(),
                    "message {i}: a tool message has `content`"
                );
                ensure!(
                    call_ids.contains(id),
                    "message {i}: result for unknown call `{id}`"
                );
            }
        }
    }
    let last = sc.messages.last().map(|m| m.role);
    ensure!(
        matches!(last, Some(Role::User | Role::Tool)),
        "the conversation must end on a user or tool message"
    );
    match sc.expect.kind {
        ExpectKind::Respond => ensure!(
            sc.expect.tools().is_empty() && sc.expect.args.is_empty(),
            "`respond` names no tool and no args"
        ),
        ExpectKind::ToolCall => {
            ensure!(
                sc.expect.tool.is_some() != sc.expect.any_of.is_some(),
                "give exactly one of `tool` or `any_of`"
            );
            let names = sc.expect.tools();
            ensure!(!names.is_empty(), "`any_of` is empty");
            for name in &names {
                let tool = snap
                    .tool(name)
                    .with_context(|| format!("unknown tool `{name}`"))?;
                for key in sc.expect.args.keys() {
                    ensure!(
                        tool.input_schema
                            .pointer(&format!("/properties/{key}"))
                            .is_some(),
                        "tool `{name}` has no argument `{key}`"
                    );
                }
            }
            for (key, check) in &sc.expect.args {
                let set = [
                    check.contains.is_some(),
                    check.equals.is_some(),
                    check.present.is_some(),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                ensure!(
                    set == 1,
                    "argument `{key}`: give exactly one of contains / equals / present"
                );
            }
        }
    }
    Ok(())
}

/// Every `*.toml` under `dir` (recursively), validated, sorted by id.
pub fn load_dir(dir: &Path, snapshots: &BTreeMap<String, Snapshot>) -> Result<Vec<Scenario>> {
    let mut files = Vec::new();
    collect_toml(dir, &mut files)?;
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut out = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let sc: Scenario =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        validate(&sc, snapshots).with_context(|| format!("{} ({})", sc.id, path.display()))?;
        if let Some(first) = seen.insert(sc.id.clone(), path.clone()) {
            bail!(
                "duplicate scenario id `{}` in {} and {}",
                sc.id,
                first.display(),
                path.display()
            );
        }
        out.push(sc);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn collect_toml(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_toml(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Snapshot;
    use aivyx_stencil::ToolSpec;
    use serde_json::json;

    fn snapshots() -> BTreeMap<String, Snapshot> {
        let tool = |name: &str| ToolSpec {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        };
        let snap = Snapshot {
            product: "pa".into(),
            source: "t".into(),
            system_prompt: None,
            tools: vec![tool("fs.read"), tool("fs.write")],
        };
        BTreeMap::from([("pa".to_string(), snap)])
    }

    fn parse(text: &str) -> Scenario {
        toml::from_str(text).unwrap()
    }

    const GOOD: &str = r#"
id = "pa-read"
product = "pa"
category = "simple"
messages = [{ role = "user", content = "Read notes.md" }]
[expect]
kind = "tool_call"
tool = "fs.read"
args.path = { contains = "notes.md" }
"#;

    #[test]
    fn a_good_scenario_parses_and_validates() {
        let sc = parse(GOOD);
        assert_eq!(sc.expect.tools(), ["fs.read"]);
        validate(&sc, &snapshots()).unwrap();
    }

    #[test]
    fn unknown_fields_are_rejected_at_parse_time() {
        assert!(toml::from_str::<Scenario>(&GOOD.replace("category", "categry")).is_err());
    }

    #[test]
    fn expect_rules() {
        let snaps = snapshots();
        // Unknown tool.
        assert!(
            validate(
                &parse(&GOOD.replace("tool = \"fs.read\"", "tool = \"fs.nope\"")),
                &snaps
            )
            .is_err()
        );
        // Argument key the tool's schema doesn't have.
        assert!(validate(&parse(&GOOD.replace("args.path", "args.url")), &snaps).is_err());
        // Respond must not name a tool.
        assert!(
            validate(
                &parse(&GOOD.replace("kind = \"tool_call\"", "kind = \"respond\"")),
                &snaps
            )
            .is_err()
        );
        // An arg check needs exactly one condition.
        assert!(
            validate(
                &parse(&GOOD.replace("{ contains = \"notes.md\" }", "{ }")),
                &snaps
            )
            .is_err()
        );
        // Unknown product.
        assert!(
            validate(
                &parse(&GOOD.replace("product = \"pa\"", "product = \"zz\"")),
                &snaps
            )
            .is_err()
        );
    }

    #[test]
    fn multi_step_history_must_be_consistent() {
        let snaps = snapshots();
        let ok = r#"
id = "pa-multi"
product = "pa"
category = "multi_step"
messages = [
  { role = "user", content = "Copy a.md to b.md" },
  { role = "assistant", tool_call = { id = "c1", name = "fs.read", arguments = { path = "a.md" } } },
  { role = "tool", tool_call_id = "c1", content = "hello" },
]
[expect]
kind = "tool_call"
tool = "fs.write"
"#;
        validate(&parse(ok), &snaps).unwrap();
        // A tool result for a call that never happened.
        assert!(
            validate(
                &parse(&ok.replace("tool_call_id = \"c1\"", "tool_call_id = \"c9\"")),
                &snaps
            )
            .is_err()
        );
        // The conversation must end on a user or tool message.
        let ends_on_assistant = ok.replace(
            "  { role = \"tool\", tool_call_id = \"c1\", content = \"hello\" },\n",
            "",
        );
        assert!(validate(&parse(&ends_on_assistant), &snaps).is_err());
    }

    #[test]
    fn load_dir_rejects_duplicate_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), GOOD).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.toml"), GOOD).unwrap();
        let err = load_dir(dir.path(), &snapshots()).unwrap_err();
        assert!(err.to_string().contains("pa-read"), "{err}");
    }

    /// The committed scenarios load against the committed snapshots.
    #[test]
    fn the_repository_scenarios_are_valid() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let snaps = crate::snapshot::load_dir(&root.join("fixtures/tools")).unwrap();
        let all = load_dir(&root.join("scenarios"), &snaps).unwrap();
        assert_eq!(all.len(), 60);
        for cat in [
            Category::Simple,
            Category::SimilarTools,
            Category::RequiredArgs,
            Category::OptionalArgs,
            Category::NoTool,
            Category::MultiStep,
            Category::RareTool,
            Category::Misleading,
        ] {
            assert!(
                all.iter().filter(|s| s.category == cat).count() >= 6,
                "{cat:?}"
            );
        }
    }
}

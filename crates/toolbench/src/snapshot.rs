//! A product's tool list as captured from a real request (`toolbench capture`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aivyx_stencil::{RESPOND_SENTINEL, ToolSpec};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// `pa` or `coder`; scenarios name it.
    pub product: String,
    /// Where it came from, e.g. "aivyx-pa 0.19.0, dev-run, sandbox access".
    pub source: String,
    /// The product's real system prompt, sent with every scenario.
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
}

impl Snapshot {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let snap: Snapshot =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        snap.validate()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(snap)
    }

    pub fn validate(&self) -> Result<()> {
        if self.tools.is_empty() {
            bail!("snapshot has no tools");
        }
        let mut seen = BTreeSet::new();
        for t in &self.tools {
            if t.name == RESPOND_SENTINEL {
                bail!("tool name `{RESPOND_SENTINEL}` is reserved for the grammar's text escape");
            }
            if !seen.insert(t.name.as_str()) {
                bail!("duplicate tool `{}`", t.name);
            }
            if !t.input_schema.is_object() {
                bail!("tool `{}`: input_schema is not a JSON object", t.name);
            }
        }
        Ok(())
    }

    pub fn tool(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.iter().find(|t| t.name == name)
    }
}

/// Every `*.json` in `dir`, keyed by product. Two files for one product is
/// an error (which one would scenarios get?).
pub fn load_dir(dir: &Path) -> Result<BTreeMap<String, Snapshot>> {
    let mut out = BTreeMap::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let snap = Snapshot::load(&path)?;
        if out.contains_key(&snap.product) {
            bail!(
                "two snapshots for product `{}` (second: {})",
                snap.product,
                path.display()
            );
        }
        out.insert(snap.product.clone(), snap);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "d".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }
    }

    fn snap(tools: Vec<ToolSpec>) -> Snapshot {
        Snapshot {
            product: "pa".into(),
            source: "test".into(),
            system_prompt: None,
            tools,
        }
    }

    #[test]
    fn a_valid_snapshot_passes() {
        assert!(
            snap(vec![spec("fs.read"), spec("fs.write")])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn the_respond_sentinel_is_reserved() {
        let err = snap(vec![spec("respond")]).validate().unwrap_err();
        assert!(err.to_string().contains("reserved"), "{err}");
    }

    #[test]
    fn duplicates_empty_lists_and_non_object_schemas_are_rejected() {
        assert!(snap(vec![spec("a"), spec("a")]).validate().is_err());
        assert!(snap(vec![]).validate().is_err());
        let mut bad = spec("a");
        bad.input_schema = json!("string");
        assert!(snap(vec![bad]).validate().is_err());
    }

    #[test]
    fn load_dir_keys_by_product_and_rejects_two_files_for_one_product() {
        let dir = tempfile::tempdir().unwrap();
        let write = |file: &str, product: &str| {
            let mut s = snap(vec![spec("x")]);
            s.product = product.into();
            std::fs::write(dir.path().join(file), serde_json::to_string(&s).unwrap()).unwrap();
        };
        write("pa.json", "pa");
        write("coder.json", "coder");
        let all = load_dir(dir.path()).unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["coder", "pa"]);
        write("pa2.json", "pa");
        assert!(load_dir(dir.path()).is_err());
    }

    #[test]
    fn coder_snapshot_grammar_compiles() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/tools");
        for f in ["coder.json", "pa.json"] {
            let text = std::fs::read_to_string(dir.join(f)).unwrap();
            let snap: Snapshot = serde_json::from_str(&text).unwrap();
            let grammar = aivyx_stencil::tool_call_grammar(&snap.tools);
            jsonschema::validator_for(&grammar)
                .unwrap_or_else(|e| panic!("{f}: grammar must compile: {e}"));
        }
    }
}

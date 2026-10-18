//! `toolbench.toml`: endpoints and the verdict's baseline/challenger.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The OpenAI `tools` field; the server decides how to constrain.
    Native,
    /// No `tools`; aivyx-stencil's grammar constrains the reply (Emboss).
    Grammar,
}

/// Where the grammar goes in a `grammar`-mode request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrammarField {
    /// llama-server's `json_schema` extension (what pa's Emboss sends).
    #[default]
    JsonSchema,
    /// Standard `response_format: {type: "json_schema", ...}` (mistral.rs, Ollama).
    ResponseFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub mode: Mode,
    #[serde(default)]
    pub grammar_field: GrammarField,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Extra top-level request fields (e.g. `enable_thinking = false`).
    #[serde(default)]
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    /// Sampling temperature; `None` = the server's default (not sent).
    #[serde(default)]
    pub temperature: Option<f64>,
}

fn default_max_tokens() -> u32 {
    1024
}

impl Endpoint {
    pub fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
    pub fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
    /// The temperature a request actually carries: `extra_body` wins.
    pub fn effective_temperature(&self) -> Option<f64> {
        self.extra_body
            .get("temperature")
            .and_then(serde_json::Value::as_f64)
            .or(self.temperature)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictConfig {
    /// The cheap fix: llama-server + Emboss grammar on the 9B model.
    pub baseline: String,
    /// The engine preview: mistralrs-server + grammar on the same model.
    pub challenger: String,
}

/// Settings applied to every endpoint that does not set its own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(rename = "endpoint")]
    pub endpoints: Vec<Endpoint>,
    pub verdict: Option<VerdictConfig>,
    #[serde(default)]
    pub defaults: Option<Defaults>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let default_temp = cfg.defaults.as_ref().and_then(|d| d.temperature);
        for ep in &mut cfg.endpoints {
            ep.temperature = ep.temperature.or(default_temp);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        let mut names = BTreeSet::new();
        for ep in &self.endpoints {
            ensure!(
                names.insert(ep.name.as_str()),
                "duplicate endpoint `{}`",
                ep.name
            );
        }
        if let Some(v) = &self.verdict {
            for name in [&v.baseline, &v.challenger] {
                ensure!(
                    names.contains(name.as_str()),
                    "verdict names unknown endpoint `{name}`"
                );
            }
        }
        Ok(())
    }

    pub fn endpoint(&self, name: &str) -> Result<&Endpoint> {
        match self.endpoints.iter().find(|e| e.name == name) {
            Some(ep) => Ok(ep),
            None => bail!("no endpoint `{name}` in the config"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = r#"
[[endpoint]]
name = "llama-native"
base_url = "http://127.0.0.1:8090/v1/"
model = "m"
mode = "native"

[[endpoint]]
name = "mistralrs-grammar"
base_url = "http://127.0.0.1:1234/v1"
model = "m"
mode = "grammar"
grammar_field = "response_format"

[verdict]
baseline = "llama-native"
challenger = "mistralrs-grammar"
"#;

    #[test]
    fn parses_and_finds_endpoints() {
        let cfg: Config = toml::from_str(CFG).unwrap();
        cfg.validate().unwrap();
        let ep = cfg.endpoint("llama-native").unwrap();
        assert_eq!(ep.chat_url(), "http://127.0.0.1:8090/v1/chat/completions");
        assert_eq!(ep.grammar_field, GrammarField::JsonSchema);
        assert_eq!(ep.max_tokens, 1024);
        assert!(cfg.endpoint("nope").is_err());
    }

    #[test]
    fn defaults_temperature_fills_endpoints_without_one_and_endpoint_wins() {
        let text = format!(
            "[defaults]\ntemperature = 0.6\n{}",
            CFG.replace(
                "model = \"m\"\nmode = \"native\"",
                "model = \"m\"\nmode = \"native\"\ntemperature = 0.2"
            )
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.endpoint("llama-native").unwrap().temperature, Some(0.2));
        assert_eq!(
            cfg.endpoint("mistralrs-grammar").unwrap().temperature,
            Some(0.6)
        );
    }

    #[test]
    fn no_defaults_leaves_temperature_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.toml");
        std::fs::write(&path, CFG).unwrap();
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.endpoint("llama-native").unwrap().temperature, None);
        assert_eq!(
            cfg.endpoint("llama-native")
                .unwrap()
                .effective_temperature(),
            None
        );
    }

    #[test]
    fn extra_body_temperature_wins_in_effective_temperature() {
        let mut ep = toml::from_str::<Config>(CFG).unwrap().endpoints.remove(0);
        ep.temperature = Some(0.2);
        assert_eq!(ep.effective_temperature(), Some(0.2));
        ep.extra_body
            .insert("temperature".into(), serde_json::json!(0.9));
        assert_eq!(ep.effective_temperature(), Some(0.9));
    }

    #[test]
    fn duplicate_names_and_unknown_verdict_endpoints_are_rejected() {
        let dup = CFG.replace("mistralrs-grammar\"\nbase_url", "llama-native\"\nbase_url");
        assert!(toml::from_str::<Config>(&dup).unwrap().validate().is_err());
        let bad = CFG.replace("challenger = \"mistralrs-grammar\"", "challenger = \"zz\"");
        assert!(toml::from_str::<Config>(&bad).unwrap().validate().is_err());
    }
}

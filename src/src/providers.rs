//! Provider catalog: which Anthropic-compatible endpoints the kernel can talk
//! to, and where each one's key comes from.
//!
//! Operators edit `$LAZAR_HOME/providers.toml` (outside the agent-writable
//! zones, like `access-policy.json`); the copy compiled into the binary is the
//! default when that file is absent. Front ends read the same catalog through
//! `lazar providers --json`, so provider knowledge lives in exactly one place
//! instead of the bash presets, launcher, and TUI tables it used to be
//! scattered across.

use serde::Deserialize;
use serde_json::{json, Value};
use std::{env, fs, path::Path, process::Command};

/// The catalog shipped with the kernel (`src/providers.toml`).
pub(crate) static DEFAULT_CATALOG: &str = include_str!("../providers.toml");

const CATALOG_FILE: &str = "providers.toml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Provider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_env: String,
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Model context window in tokens; sizes compaction. Omitted means the
    /// kernel default (`context::DEFAULT_CONTEXT_WINDOW_TOKENS`).
    #[serde(default)]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    #[serde(default)]
    pub providers: Vec<Provider>,
}

impl Catalog {
    /// Parse and validate catalog text. Every error names the offending
    /// provider so an operator can fix the file without reading the kernel.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut catalog: Catalog =
            toml::from_str(text).map_err(|e| format!("invalid providers.toml: {e}"))?;
        let mut seen = std::collections::HashSet::new();
        for provider in &mut catalog.providers {
            let id = provider.id.trim().to_string();
            if id.is_empty() {
                return Err("provider with an empty id".into());
            }
            if !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            {
                return Err(format!(
                    "provider '{id}': id may only contain letters, digits, '-', '_' and '.'"
                ));
            }
            if !seen.insert(id.clone()) {
                return Err(format!("duplicate provider id '{id}'"));
            }
            if !(provider.base_url.starts_with("https://")
                || provider.base_url.starts_with("http://"))
            {
                return Err(format!(
                    "provider '{id}': base_url must start with http:// or https:// (got {:?})",
                    provider.base_url
                ));
            }
            provider.base_url = provider.base_url.trim_end_matches('/').to_string();
            if provider.key_env.is_empty()
                || !provider
                    .key_env
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(format!(
                    "provider '{id}': key_env must be an environment variable name like MY_API_KEY (got {:?})",
                    provider.key_env
                ));
            }
            if provider.default_model.trim().is_empty() {
                return Err(format!("provider '{id}': default_model is empty"));
            }
            if provider.name.trim().is_empty() {
                provider.name = id.clone();
            }
            if !provider.models.contains(&provider.default_model) {
                provider.models.insert(0, provider.default_model.clone());
            }
            provider.id = id;
        }
        Ok(catalog)
    }

    /// The operator catalog at `$LAZAR_HOME/providers.toml`, or the built-in
    /// default when that file does not exist.
    pub(crate) fn load(home: &Path) -> Result<Self, String> {
        let path = home.join("user").join(CATALOG_FILE);
        if path.is_file() {
            let text = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            return Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()));
        }
        let path = home.join(CATALOG_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Self::parse(DEFAULT_CATALOG)
            }
            Err(err) => Err(format!("cannot read {}: {err}", path.display())),
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    fn ids(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.id.as_str()).collect()
    }
}

/// Where API keys come from. Abstracted so tests never touch the Keychain.
pub(crate) trait KeyStore {
    fn lookup(&self, account: &str) -> Option<String>;
}

/// Production key store: the environment variable named by `key_env`, else
/// the macOS Keychain entry `service=$LAZAR_KC_SERVICE (lazar), account=<key_env>`
/// (the `_meta/env-vars` convention). Keychain lookup is only permitted at
/// depth 0: nested lazar calls keep the documented secret isolation and
/// receive the active key only via `LAZAR_TOOL_INHERIT_PROVIDER_KEY` (or its
/// legacy `LAZAR_TOOL_INHERIT_ANTHROPIC_API_KEY` alias).
pub(crate) struct EnvThenKeychain {
    pub allow_keychain: bool,
}

impl KeyStore for EnvThenKeychain {
    fn lookup(&self, account: &str) -> Option<String> {
        if let Ok(value) = env::var(account) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
        if self.allow_keychain {
            keychain_password(account)
        } else {
            None
        }
    }
}

pub(crate) fn keychain_service() -> String {
    env::var("LAZAR_KC_SERVICE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "lazar".to_string())
}

fn keychain_password(account: &str) -> Option<String> {
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            account,
            "-s",
            &keychain_service(),
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!secret.is_empty()).then_some(secret)
}

/// A catalog entry with its key resolved (or not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub key_env: String,
    pub default_model: String,
    pub context_window: Option<u64>,
    pub api_key: Option<String>,
}

pub(crate) fn resolve(
    catalog: &Catalog,
    id: &str,
    keys: &dyn KeyStore,
) -> Result<Resolved, String> {
    let provider = catalog.get(id).ok_or_else(|| {
        format!(
            "unknown provider '{id}' (known: {}); edit $LAZAR_HOME/providers.toml to add one",
            catalog.ids().join(", ")
        )
    })?;
    Ok(Resolved {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        key_env: provider.key_env.clone(),
        default_model: provider.default_model.clone(),
        context_window: provider.context_window,
        api_key: keys.lookup(&provider.key_env),
    })
}

pub(crate) fn missing_key_message(provider: &Resolved) -> String {
    format!(
        "provider '{}' has no API key: export {} or store it with `security add-generic-password -a {} -s {} -w`",
        provider.id,
        provider.key_env,
        provider.key_env,
        keychain_service()
    )
}

/// One row of `lazar providers`: everything a picker needs, never a secret.
pub(crate) fn describe(catalog: &Catalog, keys: &dyn KeyStore) -> Vec<Value> {
    catalog
        .providers
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "base_url": p.base_url,
                "key_env": p.key_env,
                "key_available": keys.lookup(&p.key_env).is_some(),
                "default_model": p.default_model,
                "models": p.models,
                "context_window": p.context_window,
            })
        })
        .collect()
}

pub(crate) fn render_table(rows: &[Value]) -> String {
    let id_width = rows
        .iter()
        .filter_map(|r| r["id"].as_str())
        .map(str::len)
        .max()
        .unwrap_or(2)
        .max(2);
    let mut out = format!("{:<id_width$}  KEY  {:<40}  DEFAULT MODEL\n", "ID", "ENDPOINT");
    for row in rows {
        let key = if row["key_available"].as_bool().unwrap_or(false) {
            "yes"
        } else {
            "no "
        };
        out.push_str(&format!(
            "{:<id_width$}  {key}  {:<40}  {}\n",
            row["id"].as_str().unwrap_or("?"),
            row["base_url"].as_str().unwrap_or("?"),
            row["default_model"].as_str().unwrap_or("?"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    pub(crate) struct FakeKeys(pub HashMap<&'static str, &'static str>);

    impl KeyStore for FakeKeys {
        fn lookup(&self, account: &str) -> Option<String> {
            self.0.get(account).map(|s| s.to_string())
        }
    }

    #[test]
    fn built_in_catalog_is_valid_and_lists_the_known_presets() {
        let catalog = Catalog::parse(DEFAULT_CATALOG).expect("built-in catalog must parse");
        for id in ["anthropic", "minimax", "kimi", "kimi-code", "openrouter"] {
            let provider = catalog.get(id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(provider.models.contains(&provider.default_model));
            assert!(!provider.base_url.ends_with('/'));
        }
    }

    #[test]
    fn parse_rejects_duplicates_bad_urls_and_unknown_fields() {
        let dup = r#"
[[providers]]
id = "a"
name = "A"
base_url = "https://a.example"
key_env = "A_KEY"
default_model = "m"
[[providers]]
id = "a"
name = "A2"
base_url = "https://a2.example"
key_env = "A2_KEY"
default_model = "m"
"#;
        assert!(Catalog::parse(dup)
            .unwrap_err()
            .contains("duplicate provider id 'a'"));

        let bad_url = r#"
[[providers]]
id = "b"
name = "B"
base_url = "b.example"
key_env = "B_KEY"
default_model = "m"
"#;
        assert!(Catalog::parse(bad_url).unwrap_err().contains("base_url"));

        let typo = r#"
[[providers]]
id = "c"
name = "C"
baseurl = "https://c.example"
key_env = "C_KEY"
default_model = "m"
"#;
        assert!(Catalog::parse(typo)
            .unwrap_err()
            .contains("invalid providers.toml"));

        let bad_key_env = r#"
[[providers]]
id = "d"
name = "D"
base_url = "https://d.example"
key_env = "not-a-var"
default_model = "m"
"#;
        assert!(Catalog::parse(bad_key_env).unwrap_err().contains("key_env"));
    }

    #[test]
    fn parse_normalizes_trailing_slash_and_ensures_default_model_is_listed() {
        let text = r#"
[[providers]]
id = "e"
name = ""
base_url = "https://e.example/"
key_env = "E_KEY"
default_model = "e-default"
models = ["e-other"]
"#;
        let catalog = Catalog::parse(text).unwrap();
        let e = catalog.get("e").unwrap();
        assert_eq!(e.base_url, "https://e.example");
        assert_eq!(e.name, "e", "empty name falls back to the id");
        assert_eq!(
            e.models,
            vec!["e-default".to_string(), "e-other".to_string()]
        );
    }

    #[test]
    fn resolve_uses_the_key_store_and_reports_unknown_ids() {
        let catalog = Catalog::parse(DEFAULT_CATALOG).unwrap();
        let keys = FakeKeys(HashMap::from([("KIMI_API_KEY", "sk-kimi-test")]));

        let kimi = resolve(&catalog, "kimi", &keys).unwrap();
        assert_eq!(kimi.api_key.as_deref(), Some("sk-kimi-test"));
        assert_eq!(kimi.base_url, "https://api.moonshot.ai/anthropic");

        let minimax = resolve(&catalog, "minimax", &keys).unwrap();
        assert_eq!(minimax.api_key, None);
        let hint = missing_key_message(&minimax);
        assert!(hint.contains("MINIMAX_API_KEY"));
        assert!(hint.contains("security add-generic-password"));

        let err = resolve(&catalog, "nope", &keys).unwrap_err();
        assert!(err.contains("unknown provider 'nope'"));
        assert!(err.contains("kimi"));
    }

    #[test]
    fn describe_never_includes_secrets() {
        let catalog = Catalog::parse(DEFAULT_CATALOG).unwrap();
        let keys = FakeKeys(HashMap::from([(
            "ANTHROPIC_API_KEY",
            "sk-ant-very-secret",
        )]));
        let rows = describe(&catalog, &keys);
        let rendered = serde_json::to_string(&rows).unwrap() + &render_table(&rows);
        assert!(!rendered.contains("very-secret"));
        let anthropic = rows.iter().find(|r| r["id"] == "anthropic").unwrap();
        assert_eq!(anthropic["key_available"], true);
        let minimax = rows.iter().find(|r| r["id"] == "minimax").unwrap();
        assert_eq!(minimax["key_available"], false);
        assert!(render_table(&rows).lines().next().unwrap().starts_with("ID"));
    }

    #[test]
    fn load_prefers_the_operator_file_and_falls_back_to_the_built_in() {
        let home = env::temp_dir().join(format!(
            "lazar-providers-{}-{}",
            std::process::id(),
            crate::now_millis()
        ));
        fs::create_dir_all(&home).unwrap();
        assert!(Catalog::load(&home).unwrap().get("anthropic").is_some());

        fs::write(
            home.join(CATALOG_FILE),
            "[[providers]]\nid=\"only\"\nname=\"Only\"\nbase_url=\"https://only.example\"\nkey_env=\"ONLY_KEY\"\ndefault_model=\"m\"\n",
        )
        .unwrap();
        let catalog = Catalog::load(&home).unwrap();
        assert!(catalog.get("only").is_some());
        assert!(catalog.get("anthropic").is_none());

        fs::write(home.join(CATALOG_FILE), "this is not toml = = =").unwrap();
        let err = Catalog::load(&home).unwrap_err();
        assert!(err.contains("providers.toml"));
        fs::remove_dir_all(home).unwrap();
    }
}

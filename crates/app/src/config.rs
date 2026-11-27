//! The optional configuration file (`fathom.toml` in the data directory, or the path in
//! `FATHOM_CONFIG`): which export adapters are enabled and which destinations exist.
//!
//! Credentials never have to be written in the file: any value can be `{ env = "NAME" }` (read
//! from an environment variable) or `{ file = "/path" }` (read from a file, for example a
//! Docker or Kubernetes secret). A literal secret in the file also works, so protect the file.
//!
//! Parsing is pure (the environment is passed in) so it is fully testable. A broken destination
//! is reported and skipped; it never stops the application from starting.

use std::collections::BTreeMap;
use std::path::Path;

use application::ports::{DestinationConfig, DestinationFactory};
use application::registry::DestinationRegistry;
use secrecy::SecretString;

/// Setting names whose values are secrets: kept out of logs and out of the UI.
const SECRET_KEYS: &[&str] = &[
    "access_key_id",
    "secret_access_key",
    "session_token",
    "password",
    "token",
];

/// Settings that are folder paths (a leading `~` means the home directory).
const PATH_KEYS: &[&str] = &["path", "mount_path"];

#[derive(Debug, Default)]
pub struct Config {
    /// Adapter kinds that may be used. Default: only `local`.
    pub enabled: Vec<String>,
    pub max_export_rows: Option<u32>,
    pub destinations: Vec<DestinationConfig>,
    /// The `[keys]` table: action id → key combinations (an empty list unbinds the action).
    pub keys: std::collections::BTreeMap<String, Vec<String>>,
    /// Things wrong with individual entries (those entries are skipped).
    pub problems: Vec<String>,
}

pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

fn scalar_text(v: &toml::Value) -> Option<String> {
    match v {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A value: a literal, `{ env = "NAME" }` or `{ file = "path" }`. The error never contains the
/// value, only where it was supposed to come from.
fn resolve_value(
    key: &str,
    v: &toml::Value,
    env: EnvLookup<'_>,
    read_file: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    if let Some(table) = v.as_table() {
        return match (table.get("env"), table.get("file")) {
            (Some(name), None) => {
                let name = name
                    .as_str()
                    .ok_or_else(|| format!("`{key}`: `env` must be a variable name"))?;
                env(name)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("`{key}`: the environment variable `{name}` is not set"))
            }
            (None, Some(path)) => {
                let path = path
                    .as_str()
                    .ok_or_else(|| format!("`{key}`: `file` must be a path"))?;
                read_file(path)
                    .map(|s| s.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| format!("`{key}`: could not read `{path}`"))
            }
            _ => Err(format!(
                "`{key}`: use a value, `{{ env = \"NAME\" }}` or `{{ file = \"path\" }}`"
            )),
        };
    }
    scalar_text(v).ok_or_else(|| format!("`{key}`: expected a string, number or boolean"))
}

fn expand_home(value: &str, home: Option<&Path>) -> String {
    match (value.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ if value == "~" => home.map_or_else(|| value.to_owned(), |h| h.display().to_string()),
        _ => value.to_owned(),
    }
}

fn parse_destination(
    entry: &toml::Table,
    env: EnvLookup<'_>,
    read_file: &dyn Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Result<DestinationConfig, String> {
    let name = entry
        .get("name")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or("a destination has no `name`")?
        .to_owned();
    let kind = entry
        .get("kind")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| format!("`{name}` has no `kind`"))?
        .to_owned();
    let mut settings = BTreeMap::new();
    let mut secrets = BTreeMap::new();
    for (key, value) in entry {
        if key == "name" || key == "kind" {
            continue;
        }
        let text =
            resolve_value(key, value, env, read_file).map_err(|e| format!("`{name}`: {e}"))?;
        if SECRET_KEYS.contains(&key.as_str()) {
            secrets.insert(key.clone(), SecretString::from(text));
        } else if PATH_KEYS.contains(&key.as_str()) {
            settings.insert(key.clone(), expand_home(&text, home));
        } else {
            settings.insert(key.clone(), text);
        }
    }
    Ok(DestinationConfig {
        name,
        kind,
        settings,
        secrets,
    })
}

/// Reads the configuration text. `Err` only for a syntax error in the file itself.
pub fn parse(
    text: &str,
    env: EnvLookup<'_>,
    read_file: &dyn Fn(&str) -> Option<String>,
    home: Option<&Path>,
) -> Result<Config, String> {
    let root: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| format!("the configuration file is not valid TOML: {e}"))?;
    let mut config = Config {
        enabled: default_enabled(),
        ..Config::default()
    };
    let export = root.get("export").and_then(toml::Value::as_table);
    if let Some(export) = export {
        if let Some(list) = export.get("enabled") {
            match list.as_array() {
                Some(items) => {
                    config.enabled = items
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.trim().to_lowercase()))
                        .collect();
                }
                None => config
                    .problems
                    .push("`export.enabled` must be a list such as [\"local\", \"s3\"]".into()),
            }
        }
        if let Some(rows) = export.get("max_rows") {
            match rows
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
            {
                Some(n) => config.max_export_rows = Some(n),
                None => config
                    .problems
                    .push("`export.max_rows` must be a positive whole number".into()),
            }
        }
        if let Some(entries) = export.get("destination").and_then(toml::Value::as_array) {
            for entry in entries {
                let Some(table) = entry.as_table() else {
                    config
                        .problems
                        .push("each `[[export.destination]]` must be a table".into());
                    continue;
                };
                match parse_destination(table, env, read_file, home) {
                    Ok(d) => {
                        if config.destinations.iter().any(|x| x.name == d.name) {
                            config
                                .problems
                                .push(format!("two destinations are named `{}`", d.name));
                        } else {
                            config.destinations.push(d);
                        }
                    }
                    Err(e) => config.problems.push(e),
                }
            }
        }
    }
    if let Some(keys) = root.get("keys") {
        match keys.as_table() {
            Some(table) => {
                for (action, value) in table {
                    match parse_key_list(value) {
                        Some(list) => {
                            config.keys.insert(action.clone(), list);
                        }
                        None => config.problems.push(format!(
                            "keys.{action} must be a key combination such as \"Ctrl+S\", a list of them, or \"none\""
                        )),
                    }
                }
            }
            None => config
                .problems
                .push("`keys` must be a table: [keys]".into()),
        }
    }
    Ok(config)
}

fn parse_key_list(value: &toml::Value) -> Option<Vec<String>> {
    match value {
        toml::Value::String(s) => Some(vec![s.clone()]),
        toml::Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

/// Every built-in adapter kind: the default when the configuration does not say otherwise.
pub fn default_enabled() -> Vec<String> {
    ["local", "s3", "smb", "nfs"].map(str::to_owned).into()
}

/// Builds the registry: only enabled kinds, through their factories. `default_local` is used
/// when nothing usable was configured, so exporting always has somewhere to go.
pub fn build_registry(
    config: &Config,
    factories: &[Box<dyn DestinationFactory>],
    default_local: &Path,
) -> DestinationRegistry {
    let mut registry = DestinationRegistry::new();
    for problem in &config.problems {
        registry.report_problem(problem.clone());
    }
    for d in &config.destinations {
        if !config.enabled.contains(&d.kind) {
            registry.report_problem(format!(
                "`{}` uses the `{}` adapter, which is not enabled. Add it to `enabled` under [export].",
                d.name, d.kind
            ));
            continue;
        }
        let Some(factory) = factories.iter().find(|f| f.kind() == d.kind) else {
            registry.report_problem(format!(
                "`{}`: this build has no `{}` adapter.",
                d.name, d.kind
            ));
            continue;
        };
        match factory.create(d) {
            Ok(destination) => registry.register(d.name.clone(), destination),
            Err(e) => registry.report_problem(e.to_string()),
        }
    }
    if registry.list().is_empty()
        && let Some(factory) = factories.iter().find(|f| f.kind() == "local")
    {
        let mut settings = BTreeMap::new();
        settings.insert("path".to_owned(), default_local.display().to_string());
        let fallback = DestinationConfig {
            name: "Local exports".into(),
            kind: "local".into(),
            settings,
            secrets: BTreeMap::new(),
        };
        if let Ok(destination) = factory.create(&fallback) {
            registry.register(fallback.name, destination);
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn no_files(_: &str) -> Option<String> {
        None
    }

    fn parse_with(text: &str, vars: &'static [(&'static str, &'static str)]) -> Config {
        parse(text, &env(vars), &no_files, Some(Path::new("/home/u"))).expect("valid toml")
    }

    const SAMPLE: &str = r#"
[export]
enabled = ["local", "s3"]
max_rows = 5000

[[export.destination]]
name = "Analytics archive"
kind = "s3"
bucket = "acme-analytics"
prefix = "exports/"
access_key_id = { env = "KEY_ID" }
secret_access_key = { env = "KEY_SECRET" }

[[export.destination]]
name = "Local exports"
kind = "local"
path = "~/fathom-exports"
"#;

    #[test]
    fn a_full_file_is_read_with_env_indirection_and_home_expansion() {
        let c = parse_with(SAMPLE, &[("KEY_ID", "AKIA123"), ("KEY_SECRET", "s3cr3t")]);
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert_eq!(c.enabled, ["local", "s3"]);
        assert_eq!(c.max_export_rows, Some(5000));
        let s3 = &c.destinations[0];
        assert_eq!(s3.settings["bucket"], "acme-analytics");
        assert_eq!(s3.secrets["access_key_id"].expose_secret(), "AKIA123");
        assert!(
            !s3.settings.contains_key("access_key_id"),
            "secrets stay out of settings"
        );
        assert_eq!(c.destinations[1].settings["path"], "/home/u/fathom-exports");
    }

    #[test]
    fn a_missing_variable_skips_the_destination_and_names_the_variable_not_a_value() {
        let c = parse_with(SAMPLE, &[("KEY_ID", "AKIA123")]);
        assert_eq!(c.destinations.len(), 1, "only the local one survives");
        assert!(c.problems[0].contains("KEY_SECRET"), "{:?}", c.problems);
        assert!(!c.problems[0].contains("AKIA123"));
    }

    #[test]
    fn values_can_come_from_files_and_literals() {
        let read = |p: &str| (p == "/run/secrets/pw").then(|| "from-file\n".to_owned());
        let text = r#"
[[export.destination]]
name = "Share"
kind = "smb"
mount_path = "/mnt/team"
password = { file = "/run/secrets/pw" }
token = "literal"
"#;
        let c = parse(text, &env(&[]), &read, None).unwrap();
        let d = &c.destinations[0];
        assert_eq!(d.secrets["password"].expose_secret(), "from-file");
        assert_eq!(d.secrets["token"].expose_secret(), "literal");
        assert!(c.problems.is_empty());
    }

    #[test]
    fn bad_entries_are_reported_and_do_not_stop_the_good_ones() {
        let text = r#"
[[export.destination]]
kind = "local"

[[export.destination]]
name = "No kind"

[[export.destination]]
name = "Weird"
kind = "s3"
bucket = ["not", "a", "scalar"]

[[export.destination]]
name = "Fine"
kind = "local"
path = "/tmp/x"

[[export.destination]]
name = "Fine"
kind = "local"
path = "/tmp/y"
"#;
        let c = parse_with(text, &[]);
        assert_eq!(c.destinations.len(), 1);
        assert_eq!(c.problems.len(), 4, "{:?}", c.problems);
    }

    #[test]
    fn invalid_toml_is_an_error_for_the_caller_to_report() {
        assert!(parse("[export", &env(&[]), &no_files, None).is_err());
    }

    #[test]
    fn the_keys_table_accepts_a_string_or_a_list_and_reports_other_types() {
        let c = parse_with(
            "[keys]\nrun_query = \"Ctrl+R\"\nsave_query = [\"Ctrl+S\", \"F2\"]\nexport_data = []\nfocus_mode = 5",
            &[],
        );
        assert_eq!(c.keys["run_query"], ["Ctrl+R"]);
        assert_eq!(c.keys["save_query"], ["Ctrl+S", "F2"]);
        assert!(c.keys["export_data"].is_empty());
        assert!(!c.keys.contains_key("focus_mode"));
        assert_eq!(c.problems.len(), 1);
        assert!(c.problems[0].contains("keys.focus_mode"));
    }

    #[test]
    fn defaults_enable_every_adapter_and_reject_nonsense_limits() {
        let c = parse_with("", &[]);
        assert_eq!(c.enabled, ["local", "s3", "smb", "nfs"]);
        let c = parse_with("[export]\nmax_rows = 0\nenabled = \"s3\"", &[]);
        assert_eq!(c.problems.len(), 2);
    }

    // ---- building the registry ---------------------------------------------------------------

    fn factories() -> Vec<Box<dyn DestinationFactory>> {
        vec![
            Box::new(export_local::FolderFactory::local()),
            Box::new(export_local::FolderFactory::smb()),
            Box::new(export_s3::S3Factory),
        ]
    }

    #[test]
    fn only_enabled_adapters_are_built_and_the_rest_are_explained() {
        let text = r#"
[export]
enabled = ["local"]

[[export.destination]]
name = "Archive"
kind = "s3"
bucket = "b"

[[export.destination]]
name = "Mine"
kind = "local"
path = "/tmp/mine"
"#;
        let registry = build_registry(&parse_with(text, &[]), &factories(), Path::new("/d"));
        let names: Vec<_> = registry.list().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Mine"]);
        assert!(
            registry
                .problems()
                .iter()
                .any(|p| p.contains("not enabled")),
            "{:?}",
            registry.problems()
        );
    }

    #[test]
    fn an_adapter_the_build_does_not_have_is_reported() {
        let text = "[export]\nenabled = [\"local\", \"nfs\"]\n[[export.destination]]\nname = \"N\"\nkind = \"nfs\"\npath = \"/mnt\"";
        let registry = build_registry(&parse_with(text, &[]), &factories(), Path::new("/d"));
        assert!(
            registry
                .problems()
                .iter()
                .any(|p| p.contains("no `nfs` adapter"))
        );
    }

    #[test]
    fn with_nothing_configured_exports_still_have_a_local_folder() {
        let registry = build_registry(
            &parse_with("", &[]),
            &factories(),
            Path::new("/data/exports"),
        );
        let list = registry.list();
        assert_eq!(list.len(), 1);
        assert_eq!(
            (list[0].name.as_str(), list[0].kind.as_str()),
            ("Local exports", "local")
        );
        assert_eq!(list[0].location, "/data/exports");
    }

    #[test]
    fn a_destination_that_fails_validation_is_reported_not_fatal() {
        let text = "[export]\nenabled = [\"local\", \"s3\"]\n[[export.destination]]\nname = \"NoBucket\"\nkind = \"s3\"";
        let registry = build_registry(&parse_with(text, &[]), &factories(), Path::new("/d"));
        assert!(registry.problems().iter().any(|p| p.contains("bucket")));
        assert_eq!(
            registry.list().len(),
            1,
            "falls back to the default local folder"
        );
    }

    #[test]
    fn the_shipped_example_file_is_valid_and_builds() {
        let example = include_str!("../../../fathom.example.toml");
        let config = parse_with(example, &[]);
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        assert_eq!(config.enabled, ["local", "s3", "smb", "nfs"]);
        assert_eq!(config.max_export_rows, Some(100_000));
        let registry = build_registry(&config, &factories_all(), Path::new("/d"));
        let names: Vec<_> = registry.list().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Local exports", "Analytics archive"]);
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }

    #[test]
    fn every_commented_example_destination_is_valid_when_uncommented() {
        // Uncomment the optional blocks (lines that start with `# ` followed by TOML) and check
        // that the result still parses and every destination builds.
        let example = include_str!("../../../fathom.example.toml");
        let mut lines = Vec::new();
        let mut in_block = false;
        for line in example.lines() {
            if line.starts_with("# [[export.destination]]") {
                in_block = true;
            }
            if in_block {
                if let Some(rest) = line.strip_prefix("# ") {
                    lines.push(rest.to_owned());
                    continue;
                }
                in_block = false;
            }
            lines.push(line.to_owned());
        }
        let config = parse_with(
            &lines.join("\n"),
            &[("MINIO_ACCESS_KEY", "a"), ("MINIO_SECRET_KEY", "b")],
        );
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        let registry = build_registry(&config, &factories_all(), Path::new("/d"));
        let names: Vec<_> = registry.list().into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            [
                "Local exports",
                "Analytics archive",
                "MinIO",
                "Team share",
                "Build archive"
            ]
        );
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
    }

    fn factories_all() -> Vec<Box<dyn DestinationFactory>> {
        vec![
            Box::new(export_local::FolderFactory::local()),
            Box::new(export_local::FolderFactory::smb()),
            Box::new(export_local::FolderFactory::nfs()),
            Box::new(export_s3::S3Factory),
        ]
    }
}

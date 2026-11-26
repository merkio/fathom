//! Architecture guard: enforces the layer rules from PLAN.md section 2.
//!
//! Reads `cargo metadata` (declared dependencies, no resolution needed) and fails when a
//! workspace crate depends on something its layer must not know about.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Build,
    Dev,
}

#[derive(Debug, Clone)]
struct Dep {
    name: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
struct Package {
    name: String,
    deps: Vec<Dep>,
}

#[derive(Debug, PartialEq, Eq)]
enum Role {
    Domain,
    Application,
    Adapter,
    Ui,
    App,
    TestSupport,
}

fn role_of(name: &str) -> Option<Role> {
    match name {
        "domain" => Some(Role::Domain),
        "application" => Some(Role::Application),
        "storage" => Some(Role::Adapter),
        "ui-egui" => Some(Role::Ui),
        "app" => Some(Role::App),
        "test-support" => Some(Role::TestSupport),
        n if n.starts_with("db-") || n.starts_with("llm-") => Some(Role::Adapter),
        _ => None,
    }
}

/// External crates that domain may use (normal and build dependencies).
const DOMAIN_ALLOWED: &[&str] = &["serde", "thiserror", "uuid"];

const DB_AND_HTTP: &[&str] = &["sqlx", "reqwest", "rusqlite", "keyring"];
const UI: &[&str] = &["egui", "eframe", "egui_extras"];
const RUNTIME: &[&str] = &["tokio", "async-std"];

/// Workspace crates a crate of the given role may depend on.
fn allowed_workspace_deps(role: &Role) -> Option<&'static [&'static str]> {
    match role {
        Role::Domain => Some(&[]),
        Role::Application => Some(&["domain"]),
        Role::Adapter | Role::Ui | Role::TestSupport => Some(&["domain", "application"]),
        Role::App => None, // anything
    }
}

/// External crates a role must never depend on, whatever the dependency kind.
fn forbidden_external(role: &Role) -> Vec<&'static str> {
    let join = |parts: &[&[&'static str]]| parts.iter().flat_map(|p| p.iter().copied()).collect();
    match role {
        Role::Domain => join(&[DB_AND_HTTP, UI, RUNTIME]),
        Role::Application => join(&[DB_AND_HTTP, UI]),
        Role::Ui => join(&[DB_AND_HTTP]),
        Role::Adapter => join(&[UI]),
        Role::TestSupport | Role::App => vec![],
    }
}

fn check(packages: &[Package]) -> Vec<String> {
    let workspace: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let mut violations = Vec::new();

    for pkg in packages {
        let Some(role) = role_of(&pkg.name) else {
            violations.push(format!(
                "`{}` is not covered by the architecture rules; classify it in role_of()",
                pkg.name
            ));
            continue;
        };
        let forbidden = forbidden_external(&role);

        for dep in &pkg.deps {
            if workspace.contains(dep.name.as_str()) {
                // Any crate may use the shared test helpers as a dev-dependency.
                if dep.kind == Kind::Dev && dep.name == "test-support" {
                    continue;
                }
                if let Some(allowed) = allowed_workspace_deps(&role)
                    && !allowed.contains(&dep.name.as_str())
                {
                    violations.push(format!(
                        "`{}` must not depend on workspace crate `{}`",
                        pkg.name, dep.name
                    ));
                }
                continue;
            }

            if forbidden.contains(&dep.name.as_str()) {
                violations.push(format!(
                    "`{}` must not depend on `{}` (forbidden for its layer)",
                    pkg.name, dep.name
                ));
            } else if role == Role::Domain
                && dep.kind != Kind::Dev
                && !DOMAIN_ALLOWED.contains(&dep.name.as_str())
            {
                violations.push(format!(
                    "`domain` may only depend on {DOMAIN_ALLOWED:?}, found `{}`",
                    dep.name
                ));
            }
        }
    }
    violations
}

fn load_workspace() -> Result<Vec<Package>, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(root)
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    let meta: Value = serde_json::from_slice(&out.stdout)?;
    let mut packages = Vec::new();
    for p in meta["packages"]
        .as_array()
        .ok_or("no packages in metadata")?
    {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"]
            .as_array()
            .ok_or("package without dependencies")?
        {
            let kind = match d["kind"].as_str() {
                None => Kind::Normal,
                Some("build") => Kind::Build,
                Some("dev") => Kind::Dev,
                Some(other) => return Err(format!("unknown dependency kind `{other}`").into()),
            };
            deps.push(Dep {
                name: d["name"]
                    .as_str()
                    .ok_or("dependency without name")?
                    .to_owned(),
                kind,
            });
        }
        packages.push(Package { name, deps });
    }
    Ok(packages)
}

#[test]
fn workspace_respects_layer_rules() -> Result<(), Box<dyn Error>> {
    let packages = load_workspace()?;
    assert!(
        packages.iter().any(|p| p.name == "domain"),
        "workspace not loaded"
    );
    let violations = check(&packages);
    assert!(
        violations.is_empty(),
        "architecture violations:\n  {}",
        violations.join("\n  ")
    );
    Ok(())
}

// --- the rules themselves are tested against synthetic graphs ---

fn pkg(name: &str, deps: &[(&str, Kind)]) -> Package {
    Package {
        name: name.into(),
        deps: deps
            .iter()
            .map(|(n, k)| Dep {
                name: (*n).into(),
                kind: *k,
            })
            .collect(),
    }
}

fn valid_graph() -> Vec<Package> {
    use Kind::{Dev, Normal};
    vec![
        pkg(
            "domain",
            &[("serde", Normal), ("thiserror", Normal), ("uuid", Normal)],
        ),
        pkg(
            "application",
            &[
                ("domain", Normal),
                ("async-trait", Normal),
                ("sqlparser", Normal),
            ],
        ),
        pkg(
            "db-postgres",
            &[
                ("application", Normal),
                ("domain", Normal),
                ("sqlx", Normal),
            ],
        ),
        pkg(
            "db-mysql",
            &[
                ("application", Normal),
                ("sqlx", Normal),
                ("test-support", Dev),
            ],
        ),
        pkg(
            "llm-ollama",
            &[("application", Normal), ("reqwest", Normal)],
        ),
        pkg(
            "storage",
            &[
                ("application", Normal),
                ("rusqlite", Normal),
                ("keyring", Normal),
            ],
        ),
        pkg(
            "ui-egui",
            &[
                ("application", Normal),
                ("domain", Normal),
                ("eframe", Normal),
            ],
        ),
        pkg(
            "test-support",
            &[("application", Normal), ("domain", Normal)],
        ),
        pkg(
            "app",
            &[
                ("db-postgres", Normal),
                ("db-mysql", Normal),
                ("ui-egui", Normal),
            ],
        ),
    ]
}

fn with_extra_dep(crate_name: &str, dep: &str, kind: Kind) -> Vec<Package> {
    let mut graph = valid_graph();
    if let Some(p) = graph.iter_mut().find(|p| p.name == crate_name) {
        p.deps.push(Dep {
            name: dep.into(),
            kind,
        });
    }
    graph
}

#[test]
fn valid_graph_passes() {
    assert_eq!(check(&valid_graph()), Vec::<String>::new());
}

#[test]
fn sqlx_in_domain_is_rejected() {
    let v = check(&with_extra_dep("domain", "sqlx", Kind::Normal));
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].contains("`domain` must not depend on `sqlx`"));
}

#[test]
fn tokio_in_domain_is_rejected_even_as_dev_dependency() {
    assert_eq!(
        check(&with_extra_dep("domain", "tokio", Kind::Dev)).len(),
        1
    );
}

#[test]
fn unlisted_crate_in_domain_is_rejected() {
    assert_eq!(
        check(&with_extra_dep("domain", "chrono", Kind::Normal)).len(),
        1
    );
}

#[test]
fn drivers_and_ui_in_application_are_rejected() {
    for bad in ["sqlx", "reqwest", "egui", "rusqlite"] {
        assert_eq!(
            check(&with_extra_dep("application", bad, Kind::Normal)).len(),
            1,
            "{bad}"
        );
    }
}

#[test]
fn adapters_must_not_depend_on_each_other() {
    let v = check(&with_extra_dep("db-mysql", "db-postgres", Kind::Normal));
    assert_eq!(v.len(), 1, "{v:?}");
}

#[test]
fn ui_must_not_import_adapters_or_drivers() {
    assert_eq!(
        check(&with_extra_dep("ui-egui", "db-postgres", Kind::Normal)).len(),
        1
    );
    assert_eq!(
        check(&with_extra_dep("ui-egui", "sqlx", Kind::Normal)).len(),
        1
    );
}

#[test]
fn inner_layers_must_not_depend_outward() {
    assert_eq!(
        check(&with_extra_dep("application", "ui-egui", Kind::Normal)).len(),
        1
    );
    assert_eq!(
        check(&with_extra_dep("domain", "application", Kind::Normal)).len(),
        1
    );
    assert_eq!(
        check(&with_extra_dep("db-mysql", "app", Kind::Normal)).len(),
        1
    );
}

#[test]
fn unclassified_crate_is_rejected() {
    let mut graph = valid_graph();
    graph.push(pkg("mystery", &[]));
    assert_eq!(check(&graph).len(), 1);
}

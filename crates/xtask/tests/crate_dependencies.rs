//! The crate graph keeps the edges that the architecture decides: core names
//! no vendor crate (ADR-0002), no adapter depends on another adapter, and
//! `ralphy-pricing` and `ralphy-release` are leaf crates (ADR-0056 §4), and so
//! is `ralphy-git-read` (ADR-0069 D1). The
//! graph comes from `cargo metadata`, so every dependency kind counts: a
//! dev-dependency on an adapter breaks the rule in core's tests too.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[test]
fn core_and_adapters_keep_their_dependency_edges() {
    let graph = workspace_graph();
    assert!(
        graph.iter().any(|(name, _)| name == "ralphy-core") && graph.len() >= 10,
        "expected the workspace packages in cargo metadata, found {:?}",
        graph.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    assert!(
        graph
            .iter()
            .any(|(name, deps)| name == "ralphy-cli" && deps.iter().any(|d| d == "ralphy-core")),
        "the graph lost the known edge ralphy-cli -> ralphy-core: {graph:?}"
    );
    let edges = forbidden_edges(&graph);
    assert!(
        edges.is_empty(),
        "crate dependencies that break the map in docs/ARCHITECTURE.md §5:\n{}",
        edges.join("\n")
    );
}

#[test]
fn each_forbidden_edge_is_reported() {
    let graph = vec![
        node(
            "ralphy-core",
            &["ralphy-adapter-support", "ralphy-agent-claude"],
        ),
        node(
            "ralphy-agent-codex",
            &["ralphy-agent-claude", "ralphy-core"],
        ),
        node("ralphy-release", &["ralphy-proc-util"]),
        node("ralphy-pricing", &["ralphy-core"]),
        node("ralphy-cli", &["ralphy-agent-claude", "ralphy-core"]),
        node("ralphy-git-read", &["ralphy-proc-util"]),
    ];
    let core = "ralphy-core depends on no adapter and not on ralphy-adapter-support";
    let leaf = "ralphy-pricing and ralphy-release are leaf crates";
    assert_eq!(
        forbidden_edges(&graph),
        vec![
            format!("ralphy-core -> ralphy-adapter-support: {core}"),
            format!("ralphy-core -> ralphy-agent-claude: {core}"),
            "ralphy-agent-codex -> ralphy-agent-claude: no adapter depends on another adapter"
                .to_string(),
            format!("ralphy-release -> ralphy-proc-util: {leaf}"),
            format!("ralphy-pricing -> ralphy-core: {leaf}"),
            "ralphy-git-read -> ralphy-proc-util: ralphy-git-read is a leaf crate".to_string(),
        ]
    );
}

fn node(name: &str, deps: &[&str]) -> (String, Vec<String>) {
    (
        name.to_string(),
        deps.iter().map(|d| d.to_string()).collect(),
    )
}

/// Each `(package, internal dependency)` pair that a rule forbids, as
/// `"<from> -> <to>: <rule>"`.
fn forbidden_edges(graph: &[(String, Vec<String>)]) -> Vec<String> {
    let is_adapter = |name: &str| name.starts_with("ralphy-agent-");
    let mut out = Vec::new();
    for (from, deps) in graph {
        for to in deps {
            let rule =
                if from == "ralphy-core" && (is_adapter(to) || to == "ralphy-adapter-support") {
                    Some("ralphy-core depends on no adapter and not on ralphy-adapter-support")
                } else if is_adapter(from) && is_adapter(to) {
                    Some("no adapter depends on another adapter")
                } else if from == "ralphy-pricing" || from == "ralphy-release" {
                    Some("ralphy-pricing and ralphy-release are leaf crates")
                } else if from == "ralphy-git-read" {
                    Some("ralphy-git-read is a leaf crate")
                } else {
                    None
                };
            if let Some(rule) = rule {
                out.push(format!("{from} -> {to}: {rule}"));
            }
        }
    }
    out
}

/// Every workspace package with the workspace packages it depends on.
fn workspace_graph() -> Vec<(String, Vec<String>)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .unwrap_or_else(|e| panic!("running cargo metadata: {e}"));
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("parsing cargo metadata: {e}"));
    let packages = metadata["packages"]
        .as_array()
        .expect("cargo metadata always has a packages array");
    let names: BTreeSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    packages
        .iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let deps = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d["name"].as_str())
                .filter(|d| names.contains(d))
                .map(str::to_string)
                .collect();
            Some((name.to_string(), deps))
        })
        .collect()
}

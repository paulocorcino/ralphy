//! The adapter roster: the daemon's own enumeration of launchable adapters,
//! served read-only so the workbench never keeps a second vendor list
//! (ADR-0040 Tier 4). Availability is a presence snapshot for this daemon's
//! environment, never a login probe or a launch gate.

use std::path::PathBuf;

use crate::dispatch::agent_flag;
use crate::session::Agent;

/// One launchable adapter as the console menu needs it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentRow {
    pub id: &'static str,
    pub label: &'static str,
    pub accelerator: &'static str,
    pub available: bool,
    pub reason: Option<&'static str>,
}

/// The `Alt+Shift+<digit>` accelerator each adapter answers to. EXHAUSTIVE on
/// purpose: an eighth `Agent` variant does not compile until it is given a
/// digit, which is what makes "onboarding a vendor needs no frontend change"
/// enforced rather than hoped. Digits `8` and `9` are free; `0` belongs to the
/// frontend's plain console, which is not a vendor adapter and never enters
/// this enumeration.
fn accelerator(a: Agent) -> &'static str {
    match a {
        Agent::Claude => "1",
        Agent::Codex => "2",
        Agent::OpenCode => "3",
        Agent::Kimi => "4",
        Agent::Copilot => "5",
        Agent::Cursor => "6",
        Agent::Gemini => "7",
    }
}

/// The roster, ordered by accelerator digit ascending — the order the menu
/// renders. `id`/`label` reuse [`agent_flag`], the same string the frontend
/// sends back on `/ws/session?agent=`.
pub fn roster() -> Vec<AgentRow> {
    roster_with(&Agent::locate_program)
}

pub fn roster_with(locator: &dyn Fn(Agent) -> Option<PathBuf>) -> Vec<AgentRow> {
    let mut rows: Vec<AgentRow> = Agent::ALL
        .iter()
        .map(|&a| {
            let available = locator(a)
                .as_deref()
                .is_some_and(|path| !ralphy_proc_util::is_windows_mount_path(path));
            AgentRow {
                id: agent_flag(a),
                label: agent_flag(a),
                accelerator: accelerator(a),
                available,
                reason: (!available).then_some("not installed here"),
            }
        })
        .collect();
    // Numeric, not lexicographic: a future two-digit accelerator would otherwise
    // sort "10" between "1" and "2". `Agent::ALL` is alphabetical, so the sort
    // is doing real work — it is not a no-op guarding a list already in order.
    rows.sort_by_key(|r| r.accelerator.parse::<u32>().unwrap_or(u32::MAX));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_roster_covers_every_launchable_agent() {
        let rows = roster();
        assert_eq!(
            rows.len(),
            Agent::ALL.len(),
            "the roster must serve exactly one row per launchable agent"
        );
        let served: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.id).collect();
        let expected: std::collections::BTreeSet<&str> =
            Agent::ALL.iter().map(|&a| agent_flag(a)).collect();
        assert_eq!(
            served, expected,
            "the roster's ids must equal Agent::ALL's flags — a vendor missing here is invisible to the menu"
        );
        for row in &rows {
            assert_eq!(row.label, row.id, "label mirrors the flag");
        }
    }

    #[test]
    fn accelerators_are_unique_and_stable() {
        let rows = roster();
        let digits: std::collections::BTreeSet<&str> = rows.iter().map(|r| r.accelerator).collect();
        assert_eq!(
            digits.len(),
            rows.len(),
            "two adapters share an accelerator digit"
        );
        assert!(
            !digits.contains("0"),
            "digit 0 is the plain console's, never an adapter's"
        );
        // The digits operators already have in their fingers (issue #304): these
        // pairs are a compatibility contract, not an implementation detail.
        for (id, digit) in [
            ("claude", "1"),
            ("codex", "2"),
            ("opencode", "3"),
            ("kimi", "4"),
            ("copilot", "5"),
            ("cursor", "6"),
            ("gemini", "7"),
        ] {
            let row = rows
                .iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} is missing from the roster"));
            assert_eq!(
                row.accelerator, digit,
                "{id}'s accelerator moved — muscle memory built on today's menu breaks"
            );
        }
        // Served in digit order, so the menu renders without re-sorting.
        let order: Vec<&str> = rows.iter().map(|r| r.accelerator).collect();
        assert_eq!(order, ["1", "2", "3", "4", "5", "6", "7"]);
    }

    #[test]
    fn availability_is_computed_per_roster_with_the_injected_locator() {
        let calls = RefCell::new(Vec::new());
        let rows = roster_with(&|agent| {
            calls.borrow_mut().push(agent);
            (agent == Agent::Codex).then(|| PathBuf::from("/usr/local/bin/codex"))
        });
        assert_eq!(rows.len(), Agent::ALL.len());
        assert_eq!(*calls.borrow(), Agent::ALL);
        for row in rows {
            if row.id == "codex" {
                assert!(row.available);
                assert_eq!(row.reason, None);
            } else {
                assert!(!row.available);
                assert_eq!(row.reason, Some("not installed here"));
            }
        }
    }

    #[test]
    fn foreign_mount_candidates_are_unavailable() {
        let rows = roster_with(&|agent| {
            Some(if agent == Agent::OpenCode {
                PathBuf::from("/mnt/c/Users/x/AppData/Roaming/npm/opencode")
            } else {
                PathBuf::from("/usr/local/bin/vendor")
            })
        });
        let opencode = rows.iter().find(|row| row.id == "opencode").unwrap();
        assert!(!opencode.available);
        assert_eq!(opencode.reason, Some("not installed here"));
        assert!(rows
            .iter()
            .filter(|row| row.id != "opencode")
            .all(|row| row.available && row.reason.is_none()));
    }

    #[test]
    fn availability_is_presence_only() {
        let calls = std::cell::Cell::new(0);
        let rows = roster_with(&|_| {
            calls.set(calls.get() + 1);
            Some(PathBuf::from("/usr/local/bin/vendor"))
        });
        assert_eq!(calls.get(), Agent::ALL.len());
        assert!(rows.iter().all(|row| row.available));
    }

    #[test]
    fn daemon_manifest_has_no_vendor_dependency() {
        let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
        let vendor_dependencies = vendor_dependency_paths(&manifest);
        assert!(
            vendor_dependencies.is_empty(),
            "ralphy-daemon must not link a vendor adapter crate: {vendor_dependencies:?}"
        );
    }

    #[test]
    fn vendor_dependency_scan_includes_target_specific_tables() {
        let manifest: toml::Value = toml::from_str(
            r#"
                [dependencies]
                serde = "1"

                [target.'cfg(windows)'.dependencies]
                ralphy-agent-trap = "1"
            "#,
        )
        .unwrap();

        assert_eq!(
            vendor_dependency_paths(&manifest),
            ["target.cfg(windows).dependencies.ralphy-agent-trap"]
        );
    }

    fn vendor_dependency_paths(manifest: &toml::Value) -> Vec<String> {
        fn visit(value: &toml::Value, path: &mut Vec<String>, found: &mut Vec<String>) {
            let Some(table) = value.as_table() else {
                return;
            };
            for (name, child) in table {
                path.push(name.clone());
                if name.ends_with("dependencies") {
                    if let Some(dependencies) = child.as_table() {
                        found.extend(
                            dependencies
                                .keys()
                                .filter(|dependency| dependency.starts_with("ralphy-agent-"))
                                .map(|dependency| {
                                    path.iter()
                                        .map(String::as_str)
                                        .chain(std::iter::once(dependency.as_str()))
                                        .collect::<Vec<_>>()
                                        .join(".")
                                }),
                        );
                    }
                }
                visit(child, path, found);
                path.pop();
            }
        }

        let mut found = Vec::new();
        visit(manifest, &mut Vec::new(), &mut found);
        found
    }
}

use std::fmt::Write as _;

/// Routing decision for `ralphy models`: passthrough to the agent's own lister,
/// the adapter's catalog, or an unsupported note when the agent has no
/// model-listing capability.
pub(crate) enum ModelsAction {
    Passthrough,
    /// The models `agy models` lists for the signed-in plan (ADR-0086 D4).
    AntigravityCatalog,
    Unsupported(String),
}

fn agent_slug(a: crate::CliAgent) -> &'static str {
    match a {
        crate::CliAgent::Claude => "claude",
        crate::CliAgent::Codex => "codex",
        crate::CliAgent::Copilot => "copilot",
        crate::CliAgent::Cursor => "cursor",
        crate::CliAgent::Kimi => "kimi",
        crate::CliAgent::OpenCode => "opencode",
        crate::CliAgent::Antigravity => "antigravity",
    }
}

fn unsupported_note(a: crate::CliAgent) -> String {
    format!(
        "`ralphy models` is only supported for the opencode and antigravity agents; \
         {slug} has no model-listing command.",
        slug = agent_slug(a)
    )
}

fn plan_action(a: crate::CliAgent) -> ModelsAction {
    match a {
        crate::CliAgent::OpenCode => ModelsAction::Passthrough,
        crate::CliAgent::Antigravity => ModelsAction::AntigravityCatalog,
        crate::CliAgent::Claude
        | crate::CliAgent::Codex
        | crate::CliAgent::Copilot
        | crate::CliAgent::Cursor
        | crate::CliAgent::Kimi => ModelsAction::Unsupported(unsupported_note(a)),
    }
}

#[derive(clap::Args)]
pub(crate) struct ModelsArgs {
    /// Which agent's model list to show.
    #[arg(
        long = "agent",
        value_parser = crate::cli::AgentParser::<crate::CliAgent>::new(),
        default_value = "opencode"
    )]
    pub agent: crate::CliAgent,
}

pub(crate) fn run(args: &ModelsArgs) -> anyhow::Result<()> {
    match plan_action(args.agent) {
        ModelsAction::Passthrough => ralphy_agent_opencode::list_models(),
        ModelsAction::AntigravityCatalog => {
            out!(
                "{}",
                catalog_text(&ralphy_agent_antigravity::fetch_catalog()?)
            );
            Ok(())
        }
        ModelsAction::Unsupported(note) => {
            anyhow::bail!("{note}");
        }
    }
}

/// One line per model: the id `--model` takes, then the label `agy` shows.
fn catalog_text(entries: &[ralphy_agent_antigravity::ModelEntry]) -> String {
    let mut text = String::new();
    for entry in entries {
        writeln!(text, "{}  {}", entry.id, entry.label).expect(crate::WRITE_TO_STRING);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_lists_the_agy_catalog_for_antigravity() {
        assert!(matches!(
            plan_action(crate::CliAgent::Antigravity),
            ModelsAction::AntigravityCatalog
        ));
        let entry = |id: &str, label: &str| ralphy_agent_antigravity::ModelEntry {
            id: id.to_owned(),
            label: label.to_owned(),
        };
        let entries = [
            entry("gemini-3.8-flash-low", "Gemini 3.8 Flash (Low)"),
            entry("claude-sonnet-5-5", "Claude Sonnet 5.5"),
        ];
        assert_eq!(
            catalog_text(&entries),
            "gemini-3.8-flash-low  Gemini 3.8 Flash (Low)\nclaude-sonnet-5-5  Claude Sonnet 5.5\n"
        );
    }

    #[test]
    fn models_routing_opencode_and_others() {
        assert!(
            matches!(
                plan_action(crate::CliAgent::OpenCode),
                ModelsAction::Passthrough
            ),
            "OpenCode must route to Passthrough"
        );

        let action = plan_action(crate::CliAgent::Claude);
        match action {
            ModelsAction::Unsupported(note) => {
                assert!(
                    note.contains("opencode"),
                    "unsupported note must mention 'opencode'"
                );
                assert!(
                    note.contains("claude"),
                    "unsupported note must mention 'claude'"
                );
            }
            ModelsAction::Passthrough | ModelsAction::AntigravityCatalog => {
                panic!("Claude has no model list");
            }
        }
    }
}

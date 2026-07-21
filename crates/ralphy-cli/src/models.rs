/// Routing decision for `ralphy models`: passthrough to the agent's own lister,
/// or an unsupported note when the agent has no model-listing capability.
pub enum ModelsAction {
    Passthrough,
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
    }
}

fn unsupported_note(a: crate::CliAgent) -> String {
    format!(
        "`ralphy models` is only supported for the opencode agent; \
         {slug} has no model-listing command.",
        slug = agent_slug(a)
    )
}

fn plan_action(a: crate::CliAgent) -> ModelsAction {
    match a {
        crate::CliAgent::OpenCode => ModelsAction::Passthrough,
        other => ModelsAction::Unsupported(unsupported_note(other)),
    }
}

#[derive(clap::Args)]
pub struct ModelsArgs {
    /// Which agent's model list to show.
    #[arg(long = "agent", value_enum, default_value_t = crate::CliAgent::OpenCode)]
    pub agent: crate::CliAgent,
}

pub fn run(args: ModelsArgs) -> anyhow::Result<()> {
    match plan_action(args.agent) {
        ModelsAction::Passthrough => ralphy_agent_opencode::list_models(),
        ModelsAction::Unsupported(note) => {
            anyhow::bail!("{note}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ModelsAction::Passthrough => {
                panic!("Claude must not route to Passthrough");
            }
        }
    }
}

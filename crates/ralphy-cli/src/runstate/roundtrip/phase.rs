//! Round trips of the agent-phase events: `planning` and `executing`.

use super::*;

/// `planning` round-trips its model and effort. `cmd` reaches the bus even
/// though no decoder arm reads it. The empty string every adapter uses for an
/// absent model/effort decodes to `None` — the shape opencode's `?None`
/// rendered (the encoding-skew collapse). ADR-0044 D9: a `variant` value never
/// populates `effort` (OpenCode's dialect rides its own field), and a real
/// effort rung lands in `effort`, not `variant`.
#[test]
fn roundtrip_planning() {
    // (case, cmd, model, effort, variant, fields.effort, fields.variant, decoded)
    let rows = [
        (
            "model and effort",
            "claude -p",
            "claude-opus-4",
            "high",
            "",
            Some("high"),
            None,
            RunEvent::Planning {
                model: Some("claude-opus-4".into()),
                effort: Some("high".into()),
            },
        ),
        (
            "absent model and effort",
            "opencode run",
            "",
            "",
            "",
            None,
            None,
            RunEvent::Planning {
                model: None,
                effort: None,
            },
        ),
        (
            "a variant does not fold into effort",
            "opencode run",
            "",
            "",
            "high",
            None,
            Some("high"),
            RunEvent::Planning {
                model: None,
                effort: None,
            },
        ),
        (
            "an effort decodes without a variant",
            "claude -p",
            "",
            "medium",
            "",
            Some("medium"),
            None,
            RunEvent::Planning {
                model: None,
                effort: Some("medium".into()),
            },
        ),
    ];
    for (case, cmd, model, effort, variant, fields_effort, fields_variant, want) in rows {
        let ev = one(|| ralphy_core::emit::planning(cmd, model, effort, variant));
        assert_eq!(ev.fields.cmd.as_deref(), Some(cmd), "{case}: cmd");
        assert_eq!(ev.fields.effort.as_deref(), fields_effort, "{case}: effort");
        assert_eq!(
            ev.fields.variant.as_deref(),
            fields_variant,
            "{case}: variant"
        );
        assert_eq!(decode(&ev), Some(want), "{case}");
    }
}

/// `executing`'s twin of [`roundtrip_planning`]. It decodes `model` through
/// `unwrap_or_default()` rather than keeping the `Option`, so it needs its own
/// absent-value rows: 4 of the 5 executing sites pass `""` for `effort`, and
/// `budget_min = 0` is the "no budget reported" sentinel the other 3 adapters
/// emit.
#[test]
fn roundtrip_executing() {
    // (case, cmd, budget, model, effort, variant, fields.effort, fields.variant, decoded)
    let rows = [
        (
            "model, effort and budget",
            "interactive claude over the PTY",
            45,
            "claude-opus-4",
            "high",
            "",
            Some("high"),
            None,
            RunEvent::Executing {
                number: 0,
                budget_min: 45,
                model: "claude-opus-4".into(),
                effort: Some("high".into()),
            },
        ),
        (
            "absent model, effort and budget",
            "kimi",
            0,
            "",
            "",
            "",
            None,
            None,
            RunEvent::Executing {
                number: 0,
                budget_min: 0,
                model: String::new(),
                effort: None,
            },
        ),
        (
            "a variant does not fold into effort",
            "opencode run",
            0,
            "",
            "",
            "high",
            None,
            Some("high"),
            RunEvent::Executing {
                number: 0,
                budget_min: 0,
                model: String::new(),
                effort: None,
            },
        ),
        (
            "an effort decodes without a variant",
            "claude -p",
            0,
            "",
            "medium",
            "",
            Some("medium"),
            None,
            RunEvent::Executing {
                number: 0,
                budget_min: 0,
                model: String::new(),
                effort: Some("medium".into()),
            },
        ),
    ];
    for (case, cmd, budget, model, effort, variant, fields_effort, fields_variant, want) in rows {
        let ev = one(|| ralphy_core::emit::executing(cmd, budget, model, effort, variant));
        assert_eq!(ev.fields.cmd.as_deref(), Some(cmd), "{case}: cmd");
        assert_eq!(ev.fields.effort.as_deref(), fields_effort, "{case}: effort");
        assert_eq!(
            ev.fields.variant.as_deref(),
            fields_variant,
            "{case}: variant"
        );
        assert_eq!(decode(&ev), Some(want), "{case}");
    }
}

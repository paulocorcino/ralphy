/// Render a [`RunEvent`] to a plain, ANSI-free line (local timestamp + outcome
/// glyph + body). The non-TTY / `NO_COLOR` clean-line path; also the public seam
/// the unit tests assert against.
#[cfg(test)]
fn render_plain_line(
    event: &RunEvent,
    ts: &DateTime<Local>,
    duration: Option<Duration>,
) -> Option<String> {
    render_line(
        event,
        ts,
        &LineExtra {
            duration,
            ..Default::default()
        },
        RenderOpts {
            color: false,
            emoji: true,
        },
    )
}

use super::*;
use crate::runstate::{RunEvent, RunState, SkipKind};
use chrono::TimeZone;
use tracing::Level;

mod active_line;
mod panel;

#[test]
fn render_plain_finished_carries_timestamp_glyph_and_no_ansi() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::IssueClosed {
        number: 30,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    };
    let line = render_plain_line(&event, &ts, Some(Duration::from_secs(133))).expect("a line");

    assert!(
        line.contains("2026-06-10 14:03:21"),
        "carries the local timestamp: {line}"
    );
    assert!(line.contains('✅'), "carries the outcome glyph: {line}");
    assert!(line.contains("#30"), "carries the issue number: {line}");
    assert!(line.contains("2m13s"), "carries the duration: {line}");
    assert!(
        !line.contains('\u{1b}'),
        "plain line has no ANSI escape byte: {line:?}"
    );
}

#[test]
fn render_done_line_shows_model_effort_duration_and_compact_meter() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    // The meter/model/effort/duration ride on `LineExtra` (the presenter computes
    // them in `drive`); the event itself only names the issue.
    let extra = LineExtra {
        duration: Some(Duration::from_secs(776)),
        model: Some("sonnet".into()),
        effort: Some("medium".into()),
        meter: Some(Meter {
            usage: UsageLite {
                input: 41_200,
                cache_read: 902_000,
                cache_creation: 22_000,
                output: 18_400,
                model: None,
            },
            usd: Some(6.10),
            partial: false,
        }),
    };
    let line = render_line(
        &RunEvent::IssueClosed {
            number: 45,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        },
        &ts,
        &extra,
        RenderOpts {
            color: false,
            emoji: true,
        },
    )
    .expect("a line");
    assert!(line.contains("#45 done"), "issue + outcome: {line}");
    assert!(line.contains("sonnet / medium"), "model / effort: {line}");
    assert!(line.contains("(12m56s)"), "issue duration: {line}");
    // Compact emoji breakdown + USD, scaled and joined by ` · `.
    assert!(line.contains("↑41.2k"), "input glyph + tokens: {line}");
    assert!(line.contains("⚡902.0k"), "cache-read: {line}");
    assert!(line.contains("❄22.0k"), "cache-write: {line}");
    assert!(line.contains("↓18.4k"), "output: {line}");
    assert!(line.contains("$6.10"), "read-time USD: {line}");
    assert!(!line.contains('\u{1b}'), "no ANSI byte: {line:?}");
}

#[test]
fn render_done_line_omits_meter_when_zero() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::IssueClosed {
        number: 9,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    };
    let line = render_plain_line(&event, &ts, None).expect("a line");
    assert!(line.contains("#9 done"), "issue + outcome: {line}");
    assert!(!line.contains('↑'), "no meter when usage is zero: {line}");
    assert!(!line.contains('$'), "no cost when usage is zero: {line}");
}

#[test]
fn render_plain_notice_shows_warn_and_error_glyphs() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let warn_line = render_plain_line(
        &RunEvent::Notice {
            level: Level::WARN,
            message: "could not return to 'main'".to_string(),
        },
        &ts,
        None,
    )
    .expect("warn renders a line");
    assert!(warn_line.contains('⚠'), "warn glyph: {warn_line}");
    assert!(
        warn_line.contains("could not return to 'main'"),
        "warn message: {warn_line}"
    );

    let error_line = render_plain_line(
        &RunEvent::Notice {
            level: Level::ERROR,
            message: "boom".to_string(),
        },
        &ts,
        None,
    )
    .expect("error renders a line");
    assert!(error_line.contains('💥'), "error glyph: {error_line}");
    assert!(error_line.contains("boom"), "error message: {error_line}");
}

#[test]
fn render_plain_sleep_started_ended_deadline_return_some_and_executing_none() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();

    let sleep_start = render_plain_line(
        &RunEvent::SleepStarted {
            reset: "15:30".to_string(),
            target_epoch: 1_000_000,
        },
        &ts,
        None,
    )
    .expect("SleepStarted renders a line");
    assert!(
        sleep_start.contains("usage limit"),
        "SleepStarted body: {sleep_start}"
    );
    assert!(
        sleep_start.contains("15:30"),
        "SleepStarted reset time: {sleep_start}"
    );

    let sleep_end =
        render_plain_line(&RunEvent::SleepEnded, &ts, None).expect("SleepEnded renders a line");
    assert!(
        sleep_end.contains("resuming"),
        "SleepEnded body: {sleep_end}"
    );

    let deadline = render_plain_line(&RunEvent::DeadlinePassed { number: 42 }, &ts, None)
        .expect("DeadlinePassed renders a line");
    assert!(
        deadline.contains("deadline"),
        "DeadlinePassed body: {deadline}"
    );
    assert!(
        deadline.contains("#42"),
        "DeadlinePassed number: {deadline}"
    );

    // Executing is live-region only — no permanent line.
    assert_eq!(
        render_plain_line(
            &RunEvent::Executing {
                number: 0,
                model: String::new(),
                budget_min: 0,
                effort: None,
            },
            &ts,
            None,
        ),
        None
    );
}

#[test]
fn styled_sleep_started_is_live_region_only() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let event = RunEvent::SleepStarted {
        reset: "08:10".to_string(),
        target_epoch: 1_000_000,
    };
    let styled = RenderOpts {
        color: true,
        emoji: true,
    };
    assert_eq!(
        render_line(&event, &ts, &LineExtra::default(), styled),
        None
    );

    let plain = RenderOpts {
        color: false,
        emoji: true,
    };
    assert!(render_line(&event, &ts, &LineExtra::default(), plain)
        .expect("plain sleep line")
        .contains("sleeping until 08:10"));
}

#[test]
fn render_plain_knowledge_consolidation_carries_glyph_and_counts() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 14, 2, 16, 0)
        .single()
        .unwrap();
    let started = render_plain_line(&RunEvent::KnowledgeConsolidating { notes: 4 }, &ts, None)
        .expect("KnowledgeConsolidating renders a line");
    assert!(started.contains('📚'), "knowledge glyph: {started}");
    assert!(started.contains('4'), "note count: {started}");
    assert!(started.contains("KNOWLEDGE.md"), "target file: {started}");

    let done = render_plain_line(&RunEvent::KnowledgeConsolidated { archived: 4 }, &ts, None)
        .expect("KnowledgeConsolidated renders a line");
    assert!(done.contains('📚'), "knowledge glyph: {done}");
    assert!(
        done.contains("4 note(s) archived"),
        "archived count: {done}"
    );
    assert!(!done.contains('\u{1b}'), "no ANSI byte: {done:?}");
}

#[test]
fn render_plain_needs_split_names_the_bundle_and_next_step() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let line = render_plain_line(&RunEvent::NeedsSplit { number: 3 }, &ts, None)
        .expect("NeedsSplit renders a line");
    assert!(line.contains("#3 bundle — needs split"), "{line}");
    assert!(line.contains("/to-issues"), "{line}");
}

#[test]
fn render_plain_skipped_shows_skip_label() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    // A dependency skip with no resolved blockers keeps the bare fallback.
    let blocked = render_plain_line(
        &RunEvent::Skipped {
            number: 7,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("Skipped renders a line");
    assert!(blocked.contains("skipped (blocked)"), "{blocked}");

    // A dependency skip that knows its blocker names it (`blocked by #139`).
    let blocked_by = render_plain_line(
        &RunEvent::Skipped {
            number: 140,
            kind: SkipKind::BlockedBy,
            label: None,
            blockers: vec![139],
        },
        &ts,
        None,
    )
    .expect("Skipped renders a line");
    assert!(
        blocked_by.contains("skipped (blocked by #139)"),
        "{blocked_by}"
    );

    let stop_before = render_plain_line(
        &RunEvent::Skipped {
            number: 8,
            kind: SkipKind::StopBefore,
            label: None,
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("StopBefore renders a line");
    assert!(
        stop_before.contains("skipped (stop-before)"),
        "{stop_before}"
    );

    let human_return = render_plain_line(
        &RunEvent::Skipped {
            number: 9,
            kind: SkipKind::HumanReturn,
            label: Some("needs-info".to_string()),
            blockers: vec![],
        },
        &ts,
        None,
    )
    .expect("HumanReturn renders a line");
    assert!(
        human_return.contains("skipped (needs-info)"),
        "{human_return}"
    );
}

#[test]
fn render_plain_human_blocked_names_the_blocker() {
    let ts = Local
        .with_ymd_and_hms(2026, 6, 10, 14, 3, 21)
        .single()
        .unwrap();
    let hitl = render_plain_line(
        &RunEvent::HumanBlocked {
            number: 16,
            on: vec![30],
        },
        &ts,
        None,
    )
    .expect("HumanBlocked renders a line");
    // Names the issue the operator must clear, not a bare phrase.
    assert!(hitl.contains("#16 waiting on human at #30"), "{hitl}");
    // The human-gate glyph (🙋), not the generic skip glyph, and no "skipped"
    // wording — it asks for a person, not a queue retry.
    assert!(hitl.contains("🙋"), "{hitl}");
    assert!(!hitl.contains("skipped"), "{hitl}");

    // With no resolved blocker, it degrades to the bare phrase.
    let bare = render_plain_line(
        &RunEvent::HumanBlocked {
            number: 7,
            on: vec![],
        },
        &ts,
        None,
    )
    .expect("HumanBlocked renders a line");
    assert!(bare.contains("#7 waiting on human"), "{bare}");
    assert!(!bare.contains(" at "), "{bare}");
}

#[test]
fn normalize_remote_url_handles_ssh_https_and_dot_git() {
    // SCP-style SSH → https, `.git` stripped.
    assert_eq!(
        normalize_remote_url("git@github.com:paulocorcino/ocs-inventory-go-server.git"),
        "https://github.com/paulocorcino/ocs-inventory-go-server"
    );
    // ssh:// URL form.
    assert_eq!(
        normalize_remote_url("ssh://git@github.com/owner/repo.git"),
        "https://github.com/owner/repo"
    );
    // Already https, only `.git` removed.
    assert_eq!(
        normalize_remote_url("https://github.com/owner/repo.git"),
        "https://github.com/owner/repo"
    );
    // https without `.git` is left intact.
    assert_eq!(
        normalize_remote_url("https://github.com/owner/repo"),
        "https://github.com/owner/repo"
    );
}

#[test]
fn render_info_line_emoji_plain_and_omits_missing_segments() {
    let emoji = RenderOpts {
        color: false,
        emoji: true,
    };
    let full = render_info_line(
        "ocs-inventory",
        Some("main"),
        Some("https://github.com/owner/repo"),
        emoji,
    );
    assert_eq!(
        full,
        "📦 ocs-inventory · 🌿 main · 🔗 https://github.com/owner/repo"
    );

    // No URL (local-only repo): the 🔗 segment is omitted entirely.
    let no_url = render_info_line("proj", Some("dev"), None, emoji);
    assert_eq!(no_url, "📦 proj · 🌿 dev");

    // Plain path: no emoji, no ANSI byte.
    let plain = render_info_line(
        "proj",
        Some("dev"),
        Some("https://x/y"),
        RenderOpts {
            color: false,
            emoji: false,
        },
    );
    assert_eq!(plain, "proj · dev · https://x/y");
    assert!(!plain.contains('\u{1b}'), "no ANSI byte: {plain:?}");
}

#[test]
fn fmt_duration_formats_minutes_and_seconds() {
    assert_eq!(fmt_duration(Duration::from_secs(13)), "13s");
    assert_eq!(fmt_duration(Duration::from_secs(133)), "2m13s");
    assert_eq!(fmt_duration(Duration::from_secs(120)), "2m00s");
}

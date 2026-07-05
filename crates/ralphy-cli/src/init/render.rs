use std::io::Write as _;
use std::time::Duration;

use anyhow::{Context, Result};
use console::Style;
use ralphy_core::DiagnosisReport;

use super::gate::{format_report, Agent, EnvFindings, HardFail};
use super::resolve::{
    display_bool, display_kind, display_list, display_opt, format_config_echo, resolve_bool,
    resolve_kind, resolve_list, resolve_text, seed_questions,
};
use super::wizard::{InitConfig, Question};

/// Whether the console Q&A should emit ANSI styling: an attended stdout TTY
/// without `NO_COLOR`, mirroring the presenter's detection in `ui.rs` so init and
/// the run queue agree on when to colour.
pub(crate) fn qa_color() -> bool {
    console::Term::stdout().is_term() && std::env::var_os("NO_COLOR").is_none()
}

/// The content width the Q&A wraps to: the terminal's columns, clamped so help
/// stays readable on a narrow pane and doesn't sprawl across an ultra-wide one.
fn qa_width() -> usize {
    (console::Term::stdout().size().1 as usize).clamp(48, 92)
}

/// `force_styling` overrides console's own TTY probe: the caller's `color`
/// decision (from [`qa_color`]) is already authoritative, so honour it — this is
/// what keeps the styled path testable off a TTY.
pub(crate) fn forced(style: Style) -> Style {
    style.force_styling(true)
}

/// Greedy word-wrap `text` into lines no wider than `width`, breaking only on
/// whitespace (a word longer than `width` overflows its own line rather than
/// splitting mid-word — the bug the old single-line prompt showed as `em\npty`).
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }

    if !cur.is_empty() {
        lines.push(cur);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Render one question's prompt block as a small wizard step: a `[idx/total]`
/// counter and bold-cyan label, the explanation word-wrapped under it with a
/// hanging indent, then the seeded default and cursor arrow the answer is typed
/// after. Pure over `color`/`width` so both the styled and the plain (non-TTY /
/// `NO_COLOR`) forms are unit-testable.
fn render_question(q: &Question, idx: usize, total: usize, color: bool, width: usize) -> String {
    // 8 columns aligns the help and value lines under the label, past "  [n/n] ".
    const INDENT: &str = "        ";

    let help_width = width.saturating_sub(INDENT.len()).max(24);
    let help = wrap_text(&q.help, help_width).join(&format!("\n{INDENT}"));
    if color {
        let counter = forced(Style::new().dim()).apply_to(format!("[{idx}/{total}]"));
        let label = forced(Style::new().cyan().bold()).apply_to(&q.label);
        let help = forced(Style::new().dim()).apply_to(help);
        let default = forced(Style::new().green()).apply_to(&q.default);
        let arrow = forced(Style::new().cyan().bold()).apply_to("›");
        let opt = if q.clearable {
            forced(Style::new().dim())
                .apply_to(" · optional")
                .to_string()
        } else {
            String::new()
        };
        format!("\n  {counter} {label}\n{INDENT}{help}\n{INDENT}{default}{opt} {arrow} ")
    } else {
        let opt = if q.clearable { " (optional)" } else { "" };
        format!(
            "\n  [{idx}/{total}] {}\n{INDENT}{}\n{INDENT}{}{opt} > ",
            q.label, help, q.default
        )
    }
}

/// Echo the captured config as a styled key/value list (dim labels, green values)
/// on a colour TTY, or the plain [`format_config_echo`] text otherwise. Labels
/// mirror the friendly Q&A wording so the summary reads as a recap of the answers.
pub(crate) fn print_captured_config(cfg: &InitConfig) {
    if !qa_color() {
        print!("{}", format_config_echo(cfg));
        return;
    }

    let row = |label: &str, value: String| {
        println!(
            "  {} {}",
            forced(Style::new().dim()).apply_to(format!("{label:<18}")),
            forced(Style::new().green()).apply_to(value)
        );
    };
    row("Repository type", display_kind(cfg.repo_kind));
    row(
        "Language & build",
        display_opt(cfg.language_build.as_deref()),
    );
    row("Backlog", display_opt(cfg.backlog_location.as_deref()));
    row("Planning docs", display_list(&cfg.milestone_docs));
    row("Skills folder", display_opt(cfg.skills_dir.as_deref()));
    row("Architecture docs", display_bool(cfg.has_context_or_adrs));
    row("Code host", display_opt(cfg.remote_host.as_deref()));
    row("Plan from docs", display_bool(cfg.adopt_prd_roadmap));
}

/// Print the environment-gate findings as a styled checklist (green ✓ / red ✗,
/// dim agent status) on a colour TTY, falling back to the plain [`format_report`]
/// text the unit tests and non-TTY consumers depend on.
pub(crate) fn print_gate_report(f: &EnvFindings, fails: &[HardFail]) {
    if !qa_color() {
        print!("{}", format_report(f, fails));
        return;
    }

    let mark = |good: bool| {
        if good {
            forced(Style::new().green().bold())
                .apply_to("✓")
                .to_string()
        } else {
            forced(Style::new().red().bold()).apply_to("✗").to_string()
        }
    };
    let dim = |s: &str| {
        forced(Style::new().dim())
            .apply_to(s.to_string())
            .to_string()
    };

    println!(
        "\n{}",
        forced(Style::new().cyan().bold()).apply_to("Environment")
    );
    println!("  {} python", mark(f.python));
    println!("  {} gh auth", mark(f.gh_authenticated));
    println!("  {} GitHub remote", mark(f.github_remote));
    println!("  {}", dim("agents"));
    for agent in &Agent::ALL {
        let name = agent.cli_name();
        let present = f.agents_present.contains(agent);
        let logged = present && f.agents_logged_in.contains(agent);
        let (glyph, status) = if logged {
            (
                forced(Style::new().green().bold())
                    .apply_to("✓")
                    .to_string(),
                "logged in",
            )
        } else if present {
            (dim("·"), "not logged in")
        } else {
            (dim("·"), "absent")
        };
        println!("    {glyph} {name:<9} {}", dim(status));
    }

    if fails.is_empty() {
        println!(
            "  {} {}",
            mark(true),
            forced(Style::new().green()).apply_to("all checks passed")
        );
    } else {
        println!("  {} {} blocker(s)", mark(false), fails.len());
    }
}

/// Print a secondary status line dimmed on a colour TTY (plain otherwise), so the
/// running commentary recedes behind the headers and prompts.
pub(crate) fn print_note(text: &str) {
    if qa_color() {
        println!("{}", forced(Style::new().dim()).apply_to(text));
    } else {
        println!("{text}");
    }
}

/// Print a success line: a green ✓ and the message, so finished steps read at a
/// glance the same way the gate's checklist does. Plain (`✓ text`) off a TTY.
pub(crate) fn print_ok(text: &str) {
    if qa_color() {
        println!(
            "  {} {text}",
            forced(Style::new().green().bold()).apply_to("✓")
        );
    } else {
        println!("  {text}");
    }
}

/// Print a list row under a section — a dim bullet and the item — for the file
/// lists and plans the stages emit (scaffold files, label actions, …).
pub(crate) fn print_bullet(text: &str) {
    if qa_color() {
        println!("  {} {text}", forced(Style::new().dim()).apply_to("·"));
    } else {
        println!("  - {text}");
    }
}

/// Ask a yes/no question with a styled, single-line prompt (a cyan `›`, the
/// question, a dim `[Y/n]`/`[y/N]` reflecting `default_yes`) and return the raw
/// answer for the stage's decision fn to resolve. Centralises every confirmation
/// so they all look alike instead of bare `print!("… [Y/n]: ")`.
pub(crate) fn ask_yes_no(question: &str, default_yes: bool) -> Result<String> {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    if qa_color() {
        print!(
            "\n  {} {question} {} ",
            forced(Style::new().cyan().bold()).apply_to("›"),
            forced(Style::new().dim()).apply_to(hint)
        );
    } else {
        print!("\n  > {question} {hint} ");
    }

    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading answer from stdin")?;
    Ok(line)
}

/// Run `f` while a spinner animates next to `message` on a colour TTY, so the
/// multi-second agent calls (diagnosis, issue drafting) show life instead of a
/// frozen cursor. Off a TTY it just prints the message and runs `f` — no ANSI,
/// no animation. The spinner is cleared when `f` returns; the caller prints the
/// outcome line.
pub(crate) fn with_spinner<T>(message: &str, f: impl FnOnce() -> T) -> T {
    if !qa_color() {
        print_note(message);
        return f();
    }

    let pb = indicatif::ProgressBar::new_spinner();
    // `unwrap` is safe: the template is a compile-time constant that always parses.
    let style = indicatif::ProgressStyle::with_template("  {spinner:.cyan} {msg}")
        .unwrap()
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]);
    pb.set_style(style);
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(90));
    let out = f();
    pb.finish_and_clear();
    out
}

/// Print a styled section header — a bold-cyan title and an optional dim subtitle
/// — so init's stages read like the run queue's branded output rather than bare
/// `println!`s.
pub(crate) fn print_section(title: &str, subtitle: Option<&str>) {
    if qa_color() {
        println!("\n{}", forced(Style::new().cyan().bold()).apply_to(title));
        if let Some(s) = subtitle {
            println!("{}", forced(Style::new().dim()).apply_to(s));
        }
    } else {
        println!("\n{title}");
        if let Some(s) = subtitle {
            println!("{s}");
        }
    }
}

/// Run the interactive, diagnosis-seeded Q&A on real stdin/stdout, resolving each
/// answer into an [`InitConfig`]. The pure resolvers do the work; this is the thin
/// impure shell (printing prompts, reading lines).
pub(crate) fn run_qa(report: &DiagnosisReport) -> Result<InitConfig> {
    let questions = seed_questions(report);
    let color = qa_color();
    let width = qa_width();
    let total = questions.len();
    let read_line = |i: usize| -> Result<String> {
        print!(
            "{}",
            render_question(&questions[i], i + 1, total, color, width)
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .context("reading answer from stdin")?;
        Ok(line)
    };

    // Indices match the order in `seed_questions`.
    let repo_kind = resolve_kind(report.repo_kind, &read_line(0)?);
    let language_build = resolve_text(report.language_build.as_deref(), &read_line(1)?);
    let backlog_location = resolve_text(report.backlog_location.as_deref(), &read_line(2)?);
    let milestone_docs = resolve_list(&report.milestone_docs, &read_line(3)?);
    let skills_dir = resolve_text(report.skills_dir.as_deref(), &read_line(4)?);
    let has_context_or_adrs = resolve_bool(report.has_context_or_adrs, &read_line(5)?);
    let remote_host = resolve_text(report.remote_host.as_deref(), &read_line(6)?);
    let adopt_prd_roadmap = resolve_bool(!report.milestone_docs.is_empty(), &read_line(7)?);

    Ok(InitConfig {
        repo_kind,
        language_build,
        backlog_location,
        milestone_docs,
        skills_dir,
        has_context_or_adrs,
        remote_host,
        adopt_prd_roadmap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_question_plain_shows_counter_label_help_and_default() {
        let q = Question {
            label: "Repo kind".into(),
            help: "Empty repo to scaffold, or existing codebase to adopt?".into(),
            default: "existing".into(),
            clearable: false,
        };
        let out = render_question(&q, 1, 8, false, 80);
        // No ANSI escapes on the plain path, and every part is present.
        assert!(!out.contains('\u{1b}'));
        assert!(out.contains("[1/8]"));
        assert!(out.contains("Repo kind"));
        assert!(out.contains("existing codebase to adopt"));
        assert!(out.contains("existing"));
        // A non-clearable field is not marked optional.
        assert!(!out.contains("optional"));
    }

    #[test]
    fn render_question_clearable_marks_optional_and_color_emits_ansi() {
        let q = Question {
            label: "Backlog location".into(),
            help: "Where the backlog lives.".into(),
            default: "none".into(),
            clearable: true,
        };
        assert!(render_question(&q, 3, 8, false, 80).contains("optional"));
        // The styled path wraps content in ANSI escapes.
        assert!(render_question(&q, 3, 8, true, 80).contains('\u{1b}'));
    }

    #[test]
    fn wrap_text_breaks_on_whitespace_within_width() {
        let lines = wrap_text("the quick brown fox jumps", 9);
        assert!(lines.iter().all(|l| l.chars().count() <= 9), "{lines:?}");
        // Joining with spaces round-trips the words in order.
        assert_eq!(lines.join(" "), "the quick brown fox jumps");
    }

    #[test]
    fn wrap_text_overflows_a_word_longer_than_width() {
        // A single word wider than `width` lands on its own line, never split.
        let lines = wrap_text("short superlongunbreakableword end", 8);
        assert!(
            lines.contains(&"superlongunbreakableword".to_string()),
            "{lines:?}"
        );
    }
}

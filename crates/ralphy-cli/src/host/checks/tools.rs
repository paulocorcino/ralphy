//! The tool checks of a host. `doctor` owns the tool list (ADR-0056 §12): when
//! Ralphy on the host knows `ralphy doctor --json`, its lines replace the
//! probe's git and gh checks. The probe stays for a host with no Ralphy, or
//! one older than `doctor`. None of these checks blocks: a host can be added
//! first and get its tools later.

use crate::doctor::{DoctorReport, Tool, ToolLine};

use super::super::shell::HostOs;
use super::super::ssh::HostOutput;
use super::{CheckId, CheckStatus, HostCheck};

/// Read what `ralphy doctor --json` printed. `doctor` exits non-zero when a
/// tool is missing, so the exit code says nothing here. `None` is an older
/// Ralphy that does not know the command.
///
/// Each line is read on its own: a newer Ralphy on the host may report a tool
/// this one does not know, and that line is skipped, not the whole report.
pub(crate) fn parse_doctor(out: &HostOutput) -> Option<DoctorReport> {
    // A login shell may print a banner first; the JSON is the last line.
    let last = out.stdout.lines().rev().find(|l| !l.trim().is_empty())?;
    let value: serde_json::Value = match serde_json::from_str(last.trim()) {
        Ok(value) => value,
        Err(e) => {
            tracing::debug!(error = %e, "the host printed no doctor report");
            return None;
        }
    };
    let tools = value.get("tools")?.as_array()?;
    Some(DoctorReport {
        tools: tools
            .iter()
            .filter_map(|line| match serde_json::from_value::<ToolLine>(line.clone()) {
                Ok(line) => Some(line),
                Err(e) => {
                    tracing::debug!(error = %e, "skipping a doctor line this Ralphy does not know");
                    None
                }
            })
            .collect(),
    })
}

/// Put the tool checks in `checks`: the `doctor` result when there is one,
/// else the probe's git and gh checks with a word that more comes later.
pub(crate) fn with_doctor(checks: &mut Vec<HostCheck>, doctor: Option<&DoctorReport>, os: HostOs) {
    let Some(report) = doctor else {
        for c in checks
            .iter_mut()
            .filter(|c| matches!(c.id, CheckId::Git | CheckId::Gh))
        {
            c.text
                .push_str(". Python and the agent CLIs are checked once Ralphy runs on the host");
        }
        return;
    };
    let at = checks
        .iter()
        .position(|c| matches!(c.id, CheckId::Git | CheckId::Gh))
        .unwrap_or(checks.len());
    checks.retain(|c| !matches!(c.id, CheckId::Git | CheckId::Gh));
    let tools = doctor_checks(report, os);
    let at = at.min(checks.len());
    checks.splice(at..at, tools);
}

fn doctor_checks(report: &DoctorReport, os: HostOs) -> Vec<HostCheck> {
    let mut out = Vec::new();
    for (tool, id) in [
        (Tool::Git, CheckId::Git),
        (Tool::Gh, CheckId::Gh),
        (Tool::Python, CheckId::Python),
    ] {
        if let Some(line) = report.tools.iter().find(|l| l.tool == tool) {
            out.push(HostCheck {
                id,
                status: status(line, os),
                text: line.text.clone(),
            });
        }
    }
    out.push(agent_check(report));
    out
}

/// A missing tool's fix as the host offers it. Git keeps the host's own
/// install commands: they are how a fix is offered on that OS.
fn status(line: &ToolLine, os: HostOs) -> CheckStatus {
    if line.ok {
        return CheckStatus::Pass;
    }
    match (line.tool, os) {
        (Tool::Git, HostOs::MacOs) => CheckStatus::Copy("xcode-select --install".to_string()),
        (Tool::Git, HostOs::Windows) => {
            CheckStatus::Copy("winget install --id Git.Git -e".to_string())
        }
        _ => line
            .fix
            .clone()
            .map_or(CheckStatus::Warn, CheckStatus::Copy),
    }
}

/// One check for every agent CLI: the found ones in one line, or what is
/// missing.
fn agent_check(report: &DoctorReport) -> HostCheck {
    let agents: Vec<&ToolLine> = report
        .tools
        .iter()
        .filter(|l| l.tool == Tool::Agent)
        .collect();
    let found: Vec<String> = agents
        .iter()
        .filter(|l| l.ok)
        .map(|l| match &l.version {
            Some(v) => format!("{} ({v})", l.name),
            None => l.name.clone(),
        })
        .collect();
    if found.is_empty() {
        let text = agents
            .first()
            .map_or("no agent CLI found".to_string(), |l| l.text.clone());
        return HostCheck {
            id: CheckId::Agent,
            status: CheckStatus::Warn,
            text,
        };
    }
    HostCheck {
        id: CheckId::Agent,
        status: CheckStatus::Pass,
        text: found.join(", "),
    }
}

#[cfg(test)]
mod tests;

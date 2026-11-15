//! The one handler of an error that ends the CLI: it prints the failure in the chain for a
//! person, or writes it as one JSON line for the daemon (ADR-0078 D11, D13).

use std::process::ExitCode;

use clap::ValueEnum;
use ralphy_daemon::DaemonFailure;
use ralphy_wording::Failure;

/// How the CLI reports an error that ends it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum FailureFormat {
    /// Lines a person reads.
    #[default]
    Text,
    /// One JSON line on stderr, for a program that runs ralphy.
    Json,
}

/// Reports `err` on stderr in `format`; the exit code is 1.
pub(crate) fn failure_exit(err: &anyhow::Error, format: FailureFormat) -> ExitCode {
    let failure = err.chain().find_map(|e| e.downcast_ref::<Failure>());
    match format {
        FailureFormat::Text => print_text(err, failure),
        FailureFormat::Json => print_json(err, failure),
    }
    ExitCode::FAILURE
}

fn print_text(err: &anyhow::Error, failure: Option<&Failure>) {
    let Some(failure) = failure else {
        eoutln!("Error: {err:?}");
        return;
    };
    eoutln!("error: {}", failure.text.cause);
    if let Some(remedy) = &failure.text.remedy {
        eoutln!("{remedy}");
    }
    if let Some(cause) = failure.details.get("cause") {
        eoutln!("  cause: {cause}");
    }
    let rest: Vec<String> = err
        .chain()
        .filter(|e| e.downcast_ref::<Failure>().is_none())
        .map(ToString::to_string)
        .collect();
    if rest.is_empty() {
        return;
    }
    eoutln!("\nCaused by:");
    for (i, line) in rest.iter().enumerate() {
        eoutln!("    {i}: {line}");
    }
}

fn print_json(err: &anyhow::Error, failure: Option<&Failure>) {
    let chain = format!("{err:#}");
    let failure = failure
        .cloned()
        .unwrap_or_else(|| DaemonFailure::RalphyFailed.failure(&[("cause", chain.as_str())]));
    let line = serde_json::json!({ "failure": failure, "chain": chain });
    eoutln!("{line}");
}

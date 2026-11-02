//! The tracing and presenter install of a run.

use crate::delivery;
use crate::ui;

/// Install the tracing stack. The full structured log always goes to the run's
/// `ralphy.log` (no colour, local timestamps). On screen, the animated presenter
/// (ADR-0006) renders the run's lifecycle by default; `--verbose` (or a set
/// `RUST_LOG`/`RALPHY_LOG`) drops to raw INFO `fmt` lines and disables animation
/// so debugging is unobstructed.
///
/// Local timestamps everywhere fix the reported UTC-vs-local bug at the source:
/// the `fmt` layers use `ChronoLocal`, and the presenter composes its own local
/// timestamps via `chrono::Local`.
///
/// Always returns a `PresenterHandle` — `plain()` on the raw-stderr path (no colour,
/// no bars, no-op `finalize`), or the animated presenter's handle otherwise. The
/// caller routes the early-exit notice and the final panel through it uniformly.
pub(crate) fn init_tracing(
    log_file: Option<std::fs::File>,
    verbose: bool,
    notifier: Option<delivery::DeliveryLayer>,
    events: Option<delivery::DeliveryLayer>,
    snapshot: delivery::DeliveryLayer,
) -> ui::PresenterHandle {
    use tracing_subscriber::fmt::time::ChronoLocal;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // The run-border notices are FOLDED from `ralphy_core::emit` events now (#222),
    // where they used to be unconditional `println!`s. A narrow `RUST_LOG` (say
    // `warn`) must not silence the operator-facing notice, so that target is pinned
    // at INFO regardless of the env filter.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"))
        .add_directive(
            "ralphy_core::emit=info"
                .parse()
                .expect("a static, valid filter directive"),
        );

    // `--verbose`, or any explicit env filter, means the operator wants the raw
    // log on screen — drop the presenter and disable animation (ADR-0006 D3).
    let raw_stderr = verbose
        || std::env::var_os("RUST_LOG").is_some()
        || std::env::var_os("RALPHY_LOG").is_some();

    let timer = ChronoLocal::new("%Y-%m-%d %H:%M:%S".to_string());

    // `ralphy.log` always carries the full uncoloured, local-time log.
    let file_layer = log_file.map(|file| {
        fmt::layer()
            .with_ansi(false)
            .with_timer(timer.clone())
            .with_writer(move || file.try_clone().expect("clone ralphy.log handle"))
    });

    // The notifier Layer (when installed) composes alongside the file/presenter
    // layers so it sees every consumed event; `Option<Layer>` is a no-op when None.
    // The run-border notice fold (#222) is installed on BOTH console paths — the
    // presenter is dropped entirely under `--verbose`/`RUST_LOG`, so folding inside
    // it would silently lose the notice on the raw-stderr path.
    let edge = std::sync::Arc::new(std::sync::Mutex::new(ui::EdgeNoticeState::default()));

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(notifier)
        .with(events)
        .with(snapshot)
        .with(ui::EdgeNoticeLayer::new(std::sync::Arc::clone(&edge)));

    if raw_stderr {
        let stderr_layer = fmt::layer().with_timer(timer).with_writer(std::io::stderr);
        registry.with(stderr_layer).init();
        ui::PresenterHandle::plain().with_edge(edge)
    } else {
        let presenter = ui::Presenter::new();
        let handle = presenter.handle().with_edge(edge);
        registry.with(presenter).init();
        handle
    }
}

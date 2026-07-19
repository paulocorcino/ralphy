//! The run-border console notice, folded off the event bus (#222).
//!
//! Both borders of a run that does no work — an empty queue and an `--if-idle`
//! deferral — used to print imperatively from `run_cmd`. They now ride the bus as
//! events and this tiny `Layer` folds the operator-facing sentence out of them;
//! `PresenterHandle::print_edge_notice` takes it and prints it.
//!
//! It is a `Layer`, not part of the presenter's own `LiveState`, because
//! `--verbose`/`RUST_LOG` drops the presenter entirely (`run/wiring.rs`) — folding
//! inside the presenter would silently lose the notice on the raw-stderr path.
//! ADR-0006 still holds: this is a fold, not a renderer.

use std::sync::{Arc, Mutex};

use tracing::subscriber::Subscriber;
use tracing::Event;
use tracing_subscriber::layer::{Context, Layer};

use crate::runstate::{event_to_runevent, EventFields, RunEvent};

/// What the fold accumulates across a run's borders.
#[derive(Debug, Default)]
pub(crate) struct EdgeNoticeState {
    /// The human-readable queue scope phrase off `queue built` (LOG-ONLY field),
    /// the only part of the empty-queue notice no other event carries.
    scope: Option<String>,
    /// The notice to print, once a border event has produced one.
    notice: Option<String>,
}

impl EdgeNoticeState {
    /// Take the pending notice, leaving the state empty (a notice prints once).
    pub(crate) fn take(&mut self) -> Option<String> {
        self.notice.take()
    }

    /// Fold one decoded event. Only the three border cases matter; everything
    /// else is a no-op.
    fn fold(&mut self, ev: &RunEvent) {
        match ev {
            RunEvent::QueueBuilt { scope, .. } => self.scope = scope.clone(),
            RunEvent::RunFinished { outcome, .. } if outcome == "no_work" => {
                let scope = self.scope.clone().unwrap_or_default();
                self.notice = Some(format!("No open issues for {scope} to process. Done."));
            }
            RunEvent::RunSkipped { reason } => self.notice = Some(reason.clone()),
            _ => {}
        }
    }
}

/// The `tracing` layer that drives [`EdgeNoticeState`] off the live event stream.
pub(crate) struct EdgeNoticeLayer {
    state: Arc<Mutex<EdgeNoticeState>>,
}

impl EdgeNoticeLayer {
    pub(crate) fn new(state: Arc<Mutex<EdgeNoticeState>>) -> Self {
        EdgeNoticeLayer { state }
    }
}

impl<S: Subscriber> Layer<S> for EdgeNoticeLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = EventFields {
            level: *event.metadata().level(),
            ..EventFields::default()
        };
        event.record(&mut fields);
        let Some(run_event) =
            event_to_runevent(event.metadata().target(), &fields.message, &fields)
        else {
            return;
        };
        // A poisoned lock still carries a usable state — a dropped notice would be
        // an invisible regression, so recover rather than skip.
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        guard.fold(&run_event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_built(scope: &str) -> RunEvent {
        RunEvent::QueueBuilt {
            count: 0,
            order: vec![],
            stop_before: None,
            issues: serde_json::Value::Null,
            assignee_filter: None,
            scope: Some(scope.into()),
        }
    }

    fn run_finished(outcome: &str) -> RunEvent {
        RunEvent::RunFinished {
            outcome: outcome.into(),
            issues_done: 0,
            issues_skipped: 0,
            issues_total: 0,
            up: 0,
            cr: 0,
            cw: 0,
            out: 0,
            duration_s: 0,
        }
    }

    /// The migration's real contract: the folded texts must be BYTE-identical to
    /// the imperative `print_notice` strings `run_cmd` used to emit, or the
    /// operator's console silently changed wording under a refactor.
    #[test]
    fn edge_notices_are_byte_identical_to_the_imperative_texts() {
        let mut empty = EdgeNoticeState::default();
        empty.fold(&queue_built("labels [AFK]"));
        empty.fold(&run_finished("no_work"));
        assert_eq!(
            empty.take().as_deref(),
            Some("No open issues for labels [AFK] to process. Done.")
        );

        let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
        let mut deferred = EdgeNoticeState::default();
        deferred.fold(&RunEvent::RunSkipped {
            reason: reason.into(),
        });
        assert_eq!(deferred.take().as_deref(), Some(reason));
    }

    /// A run that DID work must not print an edge notice — only the `no_work`
    /// outcome folds one.
    #[test]
    fn a_working_run_folds_no_notice() {
        let mut state = EdgeNoticeState::default();
        state.fold(&queue_built("labels [AFK]"));
        state.fold(&run_finished("completed"));
        assert_eq!(state.take(), None);
    }

    /// A notice prints once: the second take is empty.
    #[test]
    fn taking_the_notice_clears_it() {
        let mut state = EdgeNoticeState::default();
        state.fold(&RunEvent::RunSkipped {
            reason: "deferred".into(),
        });
        assert_eq!(state.take().as_deref(), Some("deferred"));
        assert_eq!(state.take(), None);
    }
}

//! The reattach shapes of a console open: a writer attach, with or without a
//! take-over, and a read-only watch.

use axum::http::StatusCode;

use super::refuse::Refusal;
use super::upgrade::{Open, Opened};
use super::SessionLabels;
use crate::routes::audit_layer::{ConsoleKind, ConsoleRecord};
use crate::session;

impl Open {
    pub(super) fn reattach(self, id: session::SessionId) -> Opened {
        let environment = self
            .locally_owned
            .as_ref()
            .and_then(|info| info.environment.clone())
            .unwrap_or_else(|| self.environment.clone());
        // A reattach re-announces the name the child was LAUNCHED under; the spec
        // is long gone, so the session record is where it comes from. Without
        // this a reload would blank the name on a console that still answers to it.
        let labels = self
            .locally_owned
            .as_ref()
            .map(|info| SessionLabels {
                name: info.name.clone(),
                checkout: info.checkout.clone(),
                watching: false,
            })
            .unwrap_or_default();
        // A watcher never touches the writer slot, so it is dispatched BEFORE the
        // attach branch and is never refused as busy.
        if self.query.watch == Some(1) {
            return self.watch(id, environment, labels);
        }
        self.attach(id, environment, labels)
    }

    fn watch(self, id: session::SessionId, environment: String, labels: SessionLabels) -> Opened {
        match self.sessions.watch(id, self.query.since) {
            Ok(att) => Opened::Session {
                att,
                id,
                environment,
                labels,
            },
            // `watch` never yields `Busy`; matching the variant keeps that a
            // compile-time fact rather than a comment.
            Err(session::AttachError::Unknown) => {
                Opened::refused(StatusCode::NOT_FOUND, "unknown session")
            }
            Err(session::AttachError::Busy | session::AttachError::Superseded) => {
                Opened::refused(StatusCode::CONFLICT, "session busy")
            }
        }
    }

    fn attach(self, id: session::SessionId, environment: String, labels: SessionLabels) -> Opened {
        let takeover = self.query.takeover == Some(1);
        let attached = self.sessions.attach_as(
            id,
            takeover,
            self.query.holder(),
            self.query.attempt,
            self.query.since,
        );
        match attached {
            Ok(att) => {
                if takeover {
                    self.audit_takeover(id);
                }
                Opened::Session {
                    att,
                    id,
                    environment,
                    labels,
                }
            }
            Err(session::AttachError::Unknown) => {
                Opened::refused(StatusCode::NOT_FOUND, "unknown session")
            }
            Err(session::AttachError::Busy) => {
                Opened::refused(StatusCode::CONFLICT, "session busy")
            }
            Err(session::AttachError::Superseded) => Opened::Refused(Refusal::Supersede {
                attempt: self.query.attempt,
            }),
        }
    }

    fn audit_takeover(&self, id: session::SessionId) {
        self.who.console(&ConsoleRecord {
            kind: ConsoleKind::Takeover,
            session: Some(id),
            agent: None,
            repo: self.locally_owned.as_ref().map(|info| info.repo.clone()),
            peer: None,
            holder: self.tab.holder.clone(),
        });
    }
}

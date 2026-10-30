//! The free-console launch of `/ws/session` (`console=1`): relayed to a peer,
//! joined to the live session of its window record, or spawned here.

use std::path::PathBuf;

use axum::http::StatusCode;

use super::join::hold;
use super::peer_console::PeerConsole;
use super::upgrade::{launch_record, Open, Opened, Placement};
use super::SessionLabels;
use crate::session;

impl Open {
    pub(super) async fn console(mut self) -> Opened {
        let command = self
            .query
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_owned);
        // The label the session list and the desk record carry: the command
        // for a startup-command console, `console` for the bare shell.
        let agent_label = command.clone().unwrap_or_else(|| "console".to_string());
        match self.place().await {
            None | Some(Placement::Local) => self.spawn_console(command, agent_label).await,
            Some(Placement::Peer {
                peer,
                slug,
                repo_ref,
            }) => {
                self.peer_console(&peer, &slug, &repo_ref, command, agent_label)
                    .await
            }
            Some(Placement::UnknownDaemon(why)) => Opened::refused(StatusCode::BAD_GATEWAY, why),
        }
    }

    async fn peer_console(
        self,
        peer: &crate::peer::PeerDescriptor,
        slug: &str,
        repo_ref: &str,
        command: Option<String>,
        agent_label: String,
    ) -> Opened {
        PeerConsole {
            sessions: self.sessions,
            query: &self.query,
            peer,
            slug,
            repo_ref,
            command,
            agent_label,
            daemon_id: self.daemon_id,
            environment: self.environment,
            bound_port: self.bound_port,
            tab: self.tab,
            who: self.who,
            record: self.record,
        }
        .launch()
        .await
    }

    async fn spawn_console(self, command: Option<String>, agent_label: String) -> Opened {
        let repo_path = match self.query.repo.as_deref() {
            Some(slug) => match self.registered_repo(slug, "a console session") {
                Ok(path) => Some(PathBuf::from(path)),
                Err(why) => return Opened::refused(StatusCode::BAD_REQUEST, why),
            },
            None => None,
        };
        let cwd = session::console_cwd(repo_path);
        let spec = session::console_spec(cwd, 24, 80, command.as_deref());
        let repo_label = self.query.repo.clone().unwrap_or_else(|| "~".to_string());
        let (_claim, joined) = self.claim_and_join().await;
        if let Some(joined) = joined {
            return joined;
        }
        let spawned = self
            .sessions
            .spawn_attached(
                repo_label,
                agent_label,
                "console".to_string(),
                None,
                None,
                self.record.clone(),
                spec,
            )
            .inspect(|(_, att)| hold(att, self.tab.holder.as_deref(), self.query.attempt));
        match spawned {
            Ok((id, att)) => {
                self.who.console(&launch_record(
                    id,
                    "console",
                    self.query.repo.clone(),
                    None,
                    &self.tab,
                ));
                Opened::Session {
                    att,
                    id,
                    environment: self.environment,
                    labels: SessionLabels::default(),
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn a console session");
                Opened::refused(StatusCode::INTERNAL_SERVER_ERROR, "failed to spawn session")
            }
        }
    }
}

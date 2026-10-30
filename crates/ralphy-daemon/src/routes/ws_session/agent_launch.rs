//! The agent launch of `/ws/session` (`agent=`): the repo, the checkout and
//! the vendor gates are settled before anything is spawned.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;

use super::gemini_root;
use super::join::hold;
use super::refuse::Refusal;
use super::upgrade::{launch_record, Open, Opened};
use super::SessionLabels;
use crate::routes::blocking_read;
use crate::{agent_state, auth, checkout, confine, registry, session};

/// A launch whose repo, checkout and gates are settled: what is asked for and
/// the directory the child runs in.
struct SettledLaunch {
    agent_str: String,
    agent: session::Agent,
    repo: String,
    path: String,
    checkout: Option<checkout::Checkout>,
    cwd: PathBuf,
}

impl Open {
    pub(super) async fn agent(self) -> Opened {
        let Some((agent_str, agent)) = self.requested_agent() else {
            return Opened::refused(StatusCode::BAD_REQUEST, "unknown agent");
        };
        let Some(repo) = self.query.repo.clone() else {
            return Opened::refused(StatusCode::BAD_REQUEST, "unknown repo");
        };
        let path = match self.registered_repo(&repo, "a session") {
            Ok(path) => path,
            Err(why) => return Opened::refused(StatusCode::BAD_REQUEST, why),
        };
        let root = PathBuf::from(&path);
        let (checkout, cwd) = match self.settle_cwd(agent, &root).await {
            Ok(settled) => settled,
            Err(refusal) => return Opened::Refused(refusal),
        };
        let launch = SettledLaunch {
            agent_str,
            agent,
            repo,
            path,
            checkout,
            cwd,
        };
        self.spawn_agent(launch).await
    }

    fn requested_agent(&self) -> Option<(String, session::Agent)> {
        let agent_str = self.query.agent.as_deref()?;
        let agent = session::Agent::from_query(agent_str)?;
        Some((agent_str.to_string(), agent))
    }

    /// The selected checkout and the directory the child runs in, once the
    /// vendor gates have passed.
    async fn settle_cwd(
        &self,
        agent: session::Agent,
        root: &Path,
    ) -> Result<(Option<checkout::Checkout>, PathBuf), Refusal> {
        let checkout = self.resolve_checkout(root).await?;
        let cwd = checkout
            .as_ref()
            .map_or_else(|| root.to_path_buf(), |c| c.dir(root));
        self.vendor_gate(agent, root, &cwd)
            .await
            .map_err(|why| Refusal::refuse(StatusCode::BAD_REQUEST, why))?;
        Ok((checkout, cwd))
    }

    /// ADR-0063 §3: the selected checkout, resolved with the same resolver and
    /// confinement as every git-backed verb (`spawn_cwd`), off the runtime.
    /// INVARIANT: this returns BEFORE the Cursor gate, the Gemini gate, `spec_for`
    /// and every spawn — a bad name writes nothing and spawns nothing.
    async fn resolve_checkout(&self, root: &Path) -> Result<Option<checkout::Checkout>, Refusal> {
        let Some(name) = self.query.checkout.as_deref() else {
            return Ok(None);
        };
        let (name, primary) = (name.to_string(), root.to_path_buf());
        let resolved = blocking_read(move || {
            let c = checkout::resolve(&name, |n| checkout::is_linked(&primary, n))?;
            confine::confine(&primary, &c.prefix(""))?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(c)
        })
        .await;
        match resolved {
            Some(Ok(c)) => Ok(Some(c)),
            Some(Err(_)) => Err(Refusal::refuse(StatusCode::BAD_REQUEST, checkout::UNKNOWN)),
            None => Err(Refusal::refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "unavailable",
            )),
        }
    }

    /// ADR-0042 D6: an ordinary Cursor run uploads the enclosing repository. The
    /// run path is gated in the adapter, but this interactive launch spawns
    /// `cursor-agent` directly — so the gate has to run here too, BEFORE the spec
    /// is built and anything is spawned: it writes `.cursorindexingignore` into the
    /// unprotected repo (announced on the daemon log) and then proceeds. A write
    /// failure (read-only tree) is the only way it stops the launch. The gate
    /// walks up from `cwd`, so a checkout gets its own opt-out and the primary
    /// keeps its own (ADR-0063 §3); the opt-in is the primary's `.ralphy/`.
    ///
    /// ADR-0043 D4/D6: a Gemini child is contained by an owned configuration root
    /// AND the policy document inside it. The daemon may not import the adapter
    /// that writes them (ADR-0032 §10), so `gemini_root::ensure` asks the CLI to
    /// (ADR-0040 Amendment 3). INVARIANT: this refusal precedes `spec_for` and
    /// every spawn path, so no Gemini child is ever created outside the owned root.
    async fn vendor_gate(
        &self,
        agent: session::Agent,
        root: &Path,
        cwd: &Path,
    ) -> Result<(), String> {
        if agent == session::Agent::Cursor {
            ralphy_proc_util::cursor::indexing_gate(cwd, session::cursor_indexing_allowed(root))
                .map_err(|e| e.to_string())?;
        }
        if agent == session::Agent::Gemini {
            gemini_root::ensure(root, &self.daemon_id).await?;
        }
        Ok(())
    }

    async fn spawn_agent(self, launch: SettledLaunch) -> Opened {
        let SettledLaunch {
            agent_str,
            agent,
            repo,
            path,
            checkout,
            cwd,
        } = launch;
        // The id first: the agent-state files are named by it and must exist
        // before the child that reads them is launched (ADR-0059 §5). Only a
        // vendor with hooks gets the slot; the store dir failing to resolve means
        // no hooks, never no console. The record claim comes first, so a launch
        // that joins a live session burns no id and writes no file.
        let (_claim, joined) = self.claim_and_join().await;
        if let Some(joined) = joined {
            return joined;
        }
        let id = self.sessions.reserve_id();
        let status = match agent_state::state_source(agent) {
            agent_state::StateSource::Hooks => auth::store_dir()
                .ok()
                .and_then(|d| agent_state::StatusFiles::for_agent(agent, &d.join("sessions"), id)),
            agent_state::StateSource::None => None,
        };
        // The fallback `--name` is built from the project NAME: a remoteless repo's
        // slug is a `path-<hash>` key, and the operator reads this name.
        let project = registry::project_name(&repo, &path);
        let root = PathBuf::from(&path);
        let console = self.query.name().map(str::to_string);
        // Off the worker: a Claude spec waits on `claude --version`, up to the
        // read limit, and a tokio worker must not wait on a child process.
        let built = tokio::task::spawn_blocking(move || {
            session::spec_with_status(
                agent,
                &root,
                cwd,
                &project,
                console.as_deref(),
                24,
                80,
                status,
            )
        })
        .await;
        let spec = match built {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!(error = %e, "failed to build a workbench session");
                return Opened::refused(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to spawn session",
                );
            }
        };
        // Lifted before the spec moves into the spawn: the bridge announces the name
        // in `session-open`, which is how the shell learns it without deriving the
        // format a second time.
        let labels = SessionLabels {
            name: spec.name.clone(),
            checkout: checkout.as_ref().map(|c| c.name().to_string()),
            watching: false,
        };
        let spawned = self
            .sessions
            .spawn_attached_as(
                id,
                repo.to_string(),
                agent_str.clone(),
                "agent".to_string(),
                None,
                labels.checkout.clone(),
                self.record.clone(),
                spec,
            )
            .inspect(|(_, att)| hold(att, self.tab.holder.as_deref(), self.query.attempt));
        match spawned {
            Ok((id, att)) => {
                self.who.console(&launch_record(
                    id,
                    &agent_str,
                    Some(repo.clone()),
                    None,
                    &self.tab,
                ));
                Opened::Session {
                    att,
                    id,
                    environment: self.environment,
                    labels,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn a workbench session");
                Opened::refused(StatusCode::INTERNAL_SERVER_ERROR, "failed to spawn session")
            }
        }
    }
}

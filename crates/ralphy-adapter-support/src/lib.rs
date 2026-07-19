//! Adapter support: the shared, vendor-neutral machinery every Ralphy **adapter**
//! leans on. This crate owns the mechanical plumbing that is identical by nature
//! across vendors: the **OS-level headless runner** (spawn a child `Command`,
//! drain stdout/stderr without deadlocking, poll to completion-or-timeout, kill on
//! the deadline, collect the captured output), the **one-shot JSON session runner**
//! ([`run_json_session`]), **auth-error** and **usage-limit** detection scaffolds
//! ([`auth_error`], [`detect_limit`], [`scan_json_lines`]), and the
//! **session-file snapshot-diff** helpers ([`session_files_appeared`],
//! [`list_session_files`]). Every one of these takes the vendor-specific part —
//! markers, formats, extensions, schema closures — as a parameter.
//!
//! ## Why this does NOT reopen ADR-0004
//!
//! ADR-0004 states there is "deliberately no shared 'headless runner' that both
//! bend to fit." That prohibition is about a shared **`Outcome`-detection**
//! runner — the semantic completion protocol each vendor must shape itself. This
//! crate extracts **only mechanical plumbing**, which is identical by nature, not
//! by imposition. It owns **no** completion protocol and produces **no**
//! `Outcome`: the headless runner hands back raw, still-separate stdout and
//! stderr; the JSON runner returns whatever the adapter's own validation closure
//! parses; the auth/limit scaffolds return a `bool`/`Option`, never an `Outcome`.
//! Each adapter's `classify_*` function still maps captured output onto its own
//! `Outcome`, and every vendor-specific decision (which markers signal auth, which
//! reset-string format to parse) stays in the adapter. This extraction is the
//! mechanical floor *beneath* the seam ADR-0004 protects, not a violation of it.
//! (This rationale is recorded here so a future architecture review does not
//! re-flag the shared crate as an ADR-0004 violation.)
//!
//! The public surface speaks only `std` types ([`Command`], [`Duration`],
//! [`ExitStatus`], [`String`]) — no `portable-pty`, no vendor names. Building the
//! `Command` (binary, flags, env scrub) stays in each adapter, as does slicing the
//! returned [`HeadlessOutput`] into the adapter's own local return shape.

mod budget;
pub use budget::{issue_deadline, IssueBudget};

mod classify;
pub use classify::{classify, CompletionSignals};

mod detect;
pub use detect::{auth_error, detect_limit, scan_json_lines};

mod json_session;
pub use json_session::{
    run_init_session, run_json_session, run_text_session, JsonSession, TextSession,
};

mod resume;
pub use resume::{plan_is_finalized_for, plan_trailer};

mod scaffold;
pub use scaffold::{run_exec_session, run_plan_session, ExecCfg, PlanCfg};

mod session_files;
pub use session_files::{list_session_files, session_files_appeared};

mod sentinel;
pub use sentinel::{blocked_reason, done_sentinel, DONE_SENTINEL, PLAN_CHARTER, PROMPT_EXECUTE};

mod assets;
pub use assets::materialize_assets;

pub use ralphy_proc_util::{
    find_program, home_dir, home_scoped_path, locate_program, locate_program_with, resolve_program,
};

mod headless;
pub use headless::{
    run_headless, run_headless_logged, run_headless_logged_watched, HeadlessCall, HeadlessOutput,
    HeadlessRun,
};

mod idle;
pub use idle::{IdleWatch, ProgressBeat, IDLE_REAPED_MSG};

mod degraded;
pub use degraded::{DegradedAction, DegradedWatch, API_DEGRADED_MSG, API_RECOVERED_MSG};

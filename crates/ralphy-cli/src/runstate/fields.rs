//! The raw `tracing` field extractor: [`EventFields`] plus the [`Visit`] impl that
//! populates it, consumed by [`super::event::event_to_runevent`].

use tracing::field::{Field, Visit};
use tracing::Level;

use super::UsageLite;

/// The typed fields extracted off one `tracing` event. Populated by the [`Visit`]
/// impl and consumed by [`event_to_runevent`]. The union of all fields across every
/// consumed event shape; unused fields remain at their `Default` values.
#[derive(Debug)]
pub struct EventFields {
    pub level: Level,
    pub message: String,
    pub number: Option<u64>,
    pub title: Option<String>,
    pub open_steps: Option<u64>,
    pub count: Option<u64>,
    pub budget_min: Option<u64>,
    pub order: Option<String>,
    /// The first `stop-before` issue number on a `queue built` event (0 = none).
    pub stop_before: Option<u64>,
    /// The enriched per-issue snapshot on a `queue built` event (ADR-0020): the
    /// JSON array string the CLI serializes from `resolve_queue_view`. Absent on a
    /// legacy `queue built` with no snapshot; parsed back into a `Value` by the
    /// decoder (`Value::Null` when absent or unparseable).
    pub issues_json: Option<String>,
    pub outcome: Option<String>,
    pub reset: Option<String>,
    pub target_epoch: Option<i64>,
    pub model: Option<String>,
    pub tokens: Option<u64>,
    /// Reasoning effort label (`low`/`medium`/`high`); adapters also report it as
    /// `variant` (OpenCode), folded into the same slot.
    pub effort: Option<String>,
    /// Per-phase token breakdown carried on `plan written` / `green — issue
    /// closed`: `up` input, `cr` cache-read, `cw` cache-write, `out` output.
    pub up: Option<u64>,
    pub cr: Option<u64>,
    pub cw: Option<u64>,
    pub out: Option<u64>,
    /// The Debug-formatted human-blocker list (`[30]`) on a `blocked — waiting on
    /// human` event (ADR-0014): the issue(s) a person must clear.
    pub human_blockers: Option<String>,
    /// The parking label on a `human-return label — skipping issue` event
    /// (ADR-0016): which human-return label outranked the queue label.
    pub label: Option<String>,
    // --- ADR-0019 run-boundary fields (`run started` / `run finished`) ---
    /// The repo slug on a `run started` event.
    pub repo: Option<String>,
    /// The comma-joined queue labels on a `run started` event (raw; the decoder
    /// splits it into the typed `Vec<String>`).
    pub queue_labels: Option<String>,
    /// The execution agent name on a `run started` event.
    pub agent: Option<String>,
    /// The plan agent name on a `run started` event.
    pub plan_agent: Option<String>,
    /// The branch policy (`new`/`current`) on a `run started` event.
    pub branch_mode: Option<String>,
    /// The base branch on a `run started` event (wire key `base`, #96 — renamed from
    /// `branch`; feeds `RunEvent::RunStarted.branch`).
    pub base: Option<String>,
    /// The run's deadline in hours on a `run started` event (`0.0` = none).
    pub deadline_hours: Option<f64>,
    /// The green-issue count on a `run finished` event.
    pub issues_done: Option<u64>,
    /// The skipped-issue count on a `run finished` event.
    pub issues_skipped: Option<u64>,
    /// The queue size on a `run finished` event.
    pub issues_total: Option<u64>,
    /// The run's wall-clock seconds on a `run finished` event.
    pub duration_s: Option<u64>,
    /// The serialized `[{text,status}]` steps on a `plan written` event (#96): the
    /// JSON array string the runner emits; parsed back into `PlanWritten.steps`.
    pub steps_json: Option<String>,
    /// The raw `plan.md` markdown on a `plan opened`/`plan closed` event (#96).
    pub plan_md: Option<String>,
    /// The resolved concrete login the queue was scoped to on a `queue built`
    /// event (ADR-0021 §5); absent / empty = whole queue.
    pub assignee_filter: Option<String>,
}

impl Default for EventFields {
    fn default() -> Self {
        EventFields {
            level: Level::INFO,
            message: String::new(),
            number: None,
            title: None,
            open_steps: None,
            count: None,
            budget_min: None,
            order: None,
            stop_before: None,
            issues_json: None,
            outcome: None,
            reset: None,
            target_epoch: None,
            model: None,
            tokens: None,
            effort: None,
            up: None,
            cr: None,
            cw: None,
            out: None,
            human_blockers: None,
            label: None,
            repo: None,
            queue_labels: None,
            agent: None,
            plan_agent: None,
            branch_mode: None,
            base: None,
            deadline_hours: None,
            issues_done: None,
            issues_skipped: None,
            issues_total: None,
            duration_s: None,
            steps_json: None,
            plan_md: None,
            assignee_filter: None,
        }
    }
}

impl Visit for EventFields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "number" => self.number = Some(value),
            "open_steps" => self.open_steps = Some(value),
            "count" => self.count = Some(value),
            "budget_min" => self.budget_min = Some(value),
            "stop_before" => self.stop_before = Some(value),
            "tokens" => self.tokens = Some(value),
            "up" => self.up = Some(value),
            "cr" => self.cr = Some(value),
            "cw" => self.cw = Some(value),
            "out" => self.out = Some(value),
            "issues_done" => self.issues_done = Some(value),
            "issues_skipped" => self.issues_skipped = Some(value),
            "issues_total" => self.issues_total = Some(value),
            "duration_s" => self.duration_s = Some(value),
            _ => {}
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "target_epoch" {
            self.target_epoch = Some(value);
        }
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "deadline_hours" {
            self.deadline_hours = Some(value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message = value.to_string(),
            "title" => self.title = Some(value.to_string()),
            "order" => self.order = Some(value.to_string()),
            "issues_json" => self.issues_json = Some(value.to_string()),
            "outcome" => self.outcome = Some(value.to_string()),
            "reset" => self.reset = Some(value.to_string()),
            "model" => self.model = clean_opt(value),
            "effort" | "variant" => self.effort = clean_opt(value),
            "label" => self.label = clean_opt(value),
            "repo" => self.repo = Some(value.to_string()),
            "queue_labels" => self.queue_labels = Some(value.to_string()),
            "agent" => self.agent = Some(value.to_string()),
            "plan_agent" => self.plan_agent = Some(value.to_string()),
            "branch_mode" => self.branch_mode = Some(value.to_string()),
            "base" => self.base = Some(value.to_string()),
            "steps_json" => self.steps_json = Some(value.to_string()),
            "plan_md" => self.plan_md = Some(value.to_string()),
            // The resolved queue assignee scope (ADR-0021 §5); empty → None.
            "assignee_filter" => self.assignee_filter = clean_opt(value),
            _ => {}
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        match field.name() {
            "message" => self.message = rendered,
            "title" => self.title = Some(rendered),
            "order" => self.order = Some(rendered),
            // The `%`-formatted (Display) enriched snapshot arrives here as the raw
            // JSON array string (ADR-0020); the decoder parses it into a `Value`.
            "issues_json" => self.issues_json = Some(rendered),
            "outcome" => self.outcome = Some(rendered),
            "reset" => self.reset = Some(rendered.clone()),
            // The `?`-formatted `Vec<u64>` human-blocker list (`[30]`), kept raw
            // for the decoder to read the numbers out of (ADR-0014).
            "human_blockers" => self.human_blockers = Some(rendered),
            // The `%`-formatted (Display) parking label on a human-return skip
            // (ADR-0016) arrives here via tracing's Display wrapper.
            "label" => self.label = clean_opt(&rendered),
            // Debug-formatted `Option<String>` / `&str` adapter fields: strip the
            // `Some("…")` / quote wrapping and treat `None`/empty as absent so the
            // decoder never carries a literal `None` or `""` into a display label.
            "model" => self.model = clean_opt(&rendered),
            "effort" | "variant" => self.effort = clean_opt(&rendered),
            // The `%`-formatted (Display) run-boundary fields arrive here via
            // tracing's Display wrapper; store them raw (no quote stripping — these
            // are plain strings, not `Option`/`&str` Debug forms).
            "repo" => self.repo = Some(rendered),
            "queue_labels" => self.queue_labels = Some(rendered),
            "agent" => self.agent = Some(rendered),
            "plan_agent" => self.plan_agent = Some(rendered),
            "branch_mode" => self.branch_mode = Some(rendered),
            "base" => self.base = Some(rendered),
            // The `%`-formatted (Display) steps JSON / raw plan markdown arrive here
            // as the raw string (no quote stripping — plain strings, not Debug forms).
            "steps_json" => self.steps_json = Some(rendered),
            "plan_md" => self.plan_md = Some(rendered),
            // The `%`-formatted (Display) resolved queue assignee scope (ADR-0021
            // §5) arrives here; an empty-string emission maps to `None`.
            "assignee_filter" => self.assignee_filter = clean_opt(&rendered),
            _ => {}
        }
    }
}

/// Normalize a possibly-`Debug`-wrapped adapter field to a clean display string,
/// or `None` when it is absent/empty. Strips a `Some("…")` wrapper and surrounding
/// quotes (the `?`-formatted `Option<String>` form some adapters still emit) and
/// maps `None`/`""` to `None` so a label never reads `None` or blank.
fn clean_opt(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if s == "None" || s.is_empty() {
        return None;
    }
    if let Some(inner) = s.strip_prefix("Some(").and_then(|r| r.strip_suffix(')')) {
        s = inner.trim();
    }
    let s = s.trim_matches('"').trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Lift the four breakdown fields + pricing model off an event into a [`UsageLite`].
pub(super) fn usage_from(fields: &EventFields) -> UsageLite {
    UsageLite {
        input: fields.up.unwrap_or(0),
        cache_read: fields.cr.unwrap_or(0),
        cache_creation: fields.cw.unwrap_or(0),
        output: fields.out.unwrap_or(0),
        model: fields.model.clone(),
    }
}

use std::cell::RefCell;

/// The event fields a capture cares about (#96): the message plus the two raw plan
/// snapshots and the serialized steps carried on the plan-lifecycle emissions.
///
/// #219 generalizes it into a characterization harness: `level` comes off
/// the event metadata and `all` holds EVERY field rendered as a string, so a
/// vocabulary pin can assert the exact key set and the observed encoding
/// (`%order` arrives as `a -> b`, `?blockers` as `[139]`).
#[derive(Default)]
pub(crate) struct CapturedFields {
    pub(crate) message: String,
    pub(crate) plan_md: Option<String>,
    pub(crate) steps_json: Option<String>,
    pub(crate) level: Option<tracing::Level>,
    pub(crate) all: std::collections::BTreeMap<String, String>,
}

impl CapturedFields {
    /// The sorted field names present on this event, `message` included — the
    /// shape a pin asserts so an ADDED or DROPPED field reds.
    pub(crate) fn keys(&self) -> Vec<&str> {
        self.all.keys().map(String::as_str).collect()
    }

    /// The rendered value of `name`, or `""` when the field is absent.
    pub(crate) fn get(&self, name: &str) -> &str {
        self.all.get(name).map_or("", String::as_str)
    }
}

impl tracing::field::Visit for CapturedFields {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.all.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        // `%plan_md`/`%steps_json` (Display) and the event message all arrive here.
        let rendered = format!("{value:?}");
        self.all.insert(field.name().to_string(), rendered.clone());
        match field.name() {
            "message" => self.message = rendered,
            "plan_md" => self.plan_md = Some(rendered),
            "steps_json" => self.steps_json = Some(rendered),
            _ => {}
        }
    }
}

std::thread_local! {
    /// The active capture target for THIS thread, if any. A `#[test]` runs on its own
    /// thread, so a test that sets this captures only its own run's events.
    static CAPTURE_TARGET: RefCell<Option<std::sync::Arc<std::sync::Mutex<Vec<CapturedFields>>>>> =
        const { RefCell::new(None) };
}

/// A process-global `tracing::Subscriber` that routes every event to the current
/// thread's [`CAPTURE_TARGET`] (a no-op when none is set). Installed ONCE as the
/// global default so callsite interest caches as enabled and a concurrent
/// no-subscriber `run_queue` in a sibling test can never poison it (the failure mode
/// of a bare `with_default`, whose thread-local dispatcher is not consulted when a
/// callsite is first registered on another thread). No `tracing-subscriber` dep.
struct GlobalCapture;

impl tracing::Subscriber for GlobalCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        CAPTURE_TARGET.with(|t| {
            if let Some(target) = t.borrow().as_ref() {
                let mut f = CapturedFields {
                    level: Some(*event.metadata().level()),
                    ..Default::default()
                };
                event.record(&mut f);
                target.lock().expect("lock the capture sink").push(f);
            }
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install [`GlobalCapture`] as the process's global tracing default exactly once;
/// a subsequent call (or a global default set elsewhere) is a harmless no-op.
fn install_global_capture() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a global default set elsewhere makes this fail, and the doc above calls \
                     that harmless"
        )]
        let _ = tracing::subscriber::set_global_default(GlobalCapture);
    });
}

/// Run `f` with this thread's `tracing` events captured, in order.
pub(crate) fn capture_run<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedFields>) {
    install_global_capture();
    let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = Some(sink.clone()));
    let out = f();
    CAPTURE_TARGET.with(|t| *t.borrow_mut() = None);
    let events = std::mem::take(&mut *sink.lock().expect("lock the capture sink"));
    (out, events)
}

/// Assert the `(level, message, field-key-set)` triple of `message` and hand
/// the event back for per-field value assertions. The target (the module that
/// emitted) is not pinned: the decoder ignores it.
#[track_caller]
pub(crate) fn pin<'a>(
    events: &'a [CapturedFields],
    message: &str,
    keys: &[&str],
) -> &'a CapturedFields {
    let ev = events
        .iter()
        .find(|f| f.message == message)
        .unwrap_or_else(|| {
            let seen: Vec<&str> = events.iter().map(|e| e.message.as_str()).collect();
            panic!("no `{message}` event was emitted; captured: {seen:?}")
        });
    assert_eq!(
        ev.level,
        Some(tracing::Level::INFO),
        "`{message}` must stay INFO — a WARN/ERROR decodes as a generic Notice"
    );
    assert_eq!(ev.keys(), keys, "`{message}` field set drifted");
    ev
}

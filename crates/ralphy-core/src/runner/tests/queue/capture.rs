//! Capture the `tracing` events of one run on the current thread, to pin the
//! event vocabulary a test reads.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};

/// One event: its level and every field rendered as a string, `message` included.
#[derive(Default)]
pub(super) struct CapturedEvent {
    level: Option<tracing::Level>,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The rendered value of `name`, or `""` when the field is absent.
    pub(super) fn get(&self, name: &str) -> &str {
        self.fields.get(name).map_or("", String::as_str)
    }
}

impl Visit for CapturedEvent {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.fields
            .insert(field.name().to_owned(), format!("{value:?}"));
    }
}

type Sink = Arc<Mutex<Vec<CapturedEvent>>>;

std::thread_local! {
    /// The capture target of this thread; a test runs on its own thread.
    static TARGET: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Routes every event to the target of the current thread, if it has one.
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
        TARGET.with(|target| {
            if let Some(sink) = target.borrow().as_ref() {
                let mut captured = CapturedEvent {
                    level: Some(*event.metadata().level()),
                    ..CapturedEvent::default()
                };
                event.record(&mut captured);
                sink.lock().expect("lock the capture sink").push(captured);
            }
        });
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// Install [`GlobalCapture`] once as the process default, so a callsite first
/// reached on another thread still reads as enabled.
fn install_global_capture() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // A default set elsewhere makes this fail; the capture then sees no event.
        match tracing::subscriber::set_global_default(GlobalCapture) {
            Ok(()) | Err(_) => {}
        }
    });
}

/// Run `f` and hand back its result with the events it emitted on this thread.
pub(super) fn capture_run<T>(f: impl FnOnce() -> T) -> (T, Vec<CapturedEvent>) {
    install_global_capture();
    let sink = Sink::default();
    TARGET.with(|target| *target.borrow_mut() = Some(Arc::clone(&sink)));
    let out = f();
    TARGET.with(|target| *target.borrow_mut() = None);
    let events = std::mem::take(&mut *sink.lock().expect("lock the capture sink"));
    (out, events)
}

/// The INFO event `message` with exactly the field names `keys` (plus `message`),
/// so an added or dropped field fails the test.
#[track_caller]
pub(super) fn pin<'a>(
    events: &'a [CapturedEvent],
    message: &str,
    keys: &[&str],
) -> &'a CapturedEvent {
    let event = events
        .iter()
        .find(|e| e.get("message") == message)
        .ok_or_else(|| format!("no `{message}` event was emitted"))
        .expect("this test needs the event");
    assert_eq!(
        event.level,
        Some(tracing::Level::INFO),
        "`{message}` must stay INFO"
    );
    let names: Vec<&str> = event.fields.keys().map(String::as_str).collect();
    assert_eq!(names, keys, "`{message}` field set drifted");
    event
}

//! Which watcher events change no directory listing.

use notify::event::ModifyKind;
use notify::EventKind;

/// Whether an event can change no listing, so it nudges nothing.
pub(super) fn changes_no_listing(event: &notify::Event) -> bool {
    // A READ is not a change. Linux inotify reports `IN_OPEN` /
    // `IN_CLOSE_NOWRITE` on a watched dir, and `notify`'s inotify backend
    // subscribes to them, so every `tree.list` (which opens the dir and
    // each child dir) arrived here as an `Access` event and became a
    // nudge — and the browser answers a nudge with another `tree.list`.
    // On a Linux daemon that was a self-sustaining loop (~25 nudges/s on
    // an idle repo). Windows never emits these, which is why it hid.
    if matches!(event.kind, EventKind::Access(_)) {
        return true;
    }
    // A directory's OWN modify — its mtime moved because a child was
    // written, which Windows and FSEvents both bubble to the parent's
    // watch — changes no listing: a listing is names and the dir
    // flag, and the names under the watched dir are what they were.
    // A rename (`Modify(Name)`) does change one and stays. Measured
    // on the macOS runner (2026-09-17): `unwatched/y` nudged the root
    // through `unwatched` itself.
    matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    ) && !event.paths.is_empty()
        && event.paths.iter().all(|p| p.is_dir())
}

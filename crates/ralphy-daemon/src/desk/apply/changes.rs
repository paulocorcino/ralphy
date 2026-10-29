//! One parsed [`Change`] applied to the stored desk: the checks, the cap of
//! each record kind, and the field-by-field merge. Each change kind has its
//! own function, so a rule of one kind does not sit in the branches of another.

use super::{
    Change, FenceFields, NoteFields, RecordType, SessionFields, WindowFields, DESK_CREATE_SLACK,
};
use crate::desk::{
    cap_console_name, rect_is_sane, DeskFence, DeskNote, DeskRecord, DeskRect, DeskStore, DESK_MAX,
    FENCE_MAX, NOTE_MAX,
};

fn sane(what: &str, id: &str, rect: &DeskRect) -> Result<(), String> {
    if rect_is_sane(rect) {
        Ok(())
    } else {
        Err(format!("{what} {id} has an out-of-frame rect"))
    }
}

fn checkout_name(id: &str, name: Option<&str>) -> Result<(), String> {
    match name {
        Some(n) if crate::checkout::lexical(n).is_none() => {
            Err(format!("checkout {n} on record {id} is not a valid name"))
        }
        _ => Ok(()),
    }
}

/// Set `slot` to `value`; whether it changed.
fn put<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

pub(super) fn apply_one(desk: &mut DeskStore, change: Change, now: i64) -> Result<bool, String> {
    match change {
        Change::CreateWindow(r) => create_window(desk, r, now),
        Change::CreateFence(f) => create_fence(desk, f, now),
        Change::CreateNote(n) => create_note(desk, n, now),
        Change::SetWindow { id, fields } => set_window(desk, &id, fields, now),
        Change::SetFence { id, fields } => set_fence(desk, &id, fields, now),
        Change::SetNote { id, fields } => set_note(desk, &id, fields, now),
        Change::Remove { kind, id } => Ok(remove(desk, kind, &id)),
        Change::Checkout { repo, name } => checkout(desk, repo, name),
        Change::CheckoutClear { repo, if_name } => Ok(checkout_clear(desk, &repo, &if_name)),
    }
}

fn create_window(desk: &mut DeskStore, mut r: DeskRecord, now: i64) -> Result<bool, String> {
    sane("record", &r.id, &r.rect)?;
    checkout_name(&r.id, r.checkout.as_deref())?;
    if let Some(stored) = desk.windows.iter_mut().find(|w| w.id == r.id) {
        // A page that adopted a running console sends a create for a
        // record another page already wrote: only its session is news.
        let did = put(&mut stored.session_id, r.session_id)
            | put(&mut stored.daemon_id, r.daemon_id)
            | put(&mut stored.environment, r.environment);
        if did {
            stored.ts = now;
        }
        return Ok(did);
    }
    if desk.windows.len() >= DESK_MAX + DESK_CREATE_SLACK {
        return Err(format!(
            "the desk already holds {} consoles",
            desk.windows.len()
        ));
    }
    cap_console_name(&mut r);
    r.ts = now;
    desk.windows.push(r);
    Ok(true)
}

fn create_fence(desk: &mut DeskStore, mut f: DeskFence, now: i64) -> Result<bool, String> {
    sane("fence", &f.id, &f.rect)?;
    if desk.fences.iter().any(|x| x.id == f.id) {
        return Ok(false);
    }
    if desk.fences.len() >= FENCE_MAX {
        return Err(format!("the desk already holds {FENCE_MAX} fences"));
    }
    f.ts = now;
    desk.fences.push(f);
    Ok(true)
}

fn create_note(desk: &mut DeskStore, mut n: DeskNote, now: i64) -> Result<bool, String> {
    sane("note", &n.id, &n.rect)?;
    checkout_name(&n.id, n.checkout.as_deref())?;
    if desk.notes.iter().any(|x| x.id == n.id) {
        return Ok(false);
    }
    if desk.notes.len() >= NOTE_MAX {
        return Err(format!("the desk already holds {NOTE_MAX} notes"));
    }
    n.ts = now;
    desk.notes.push(n);
    Ok(true)
}

fn set_window(
    desk: &mut DeskStore,
    id: &str,
    fields: WindowFields,
    now: i64,
) -> Result<bool, String> {
    if let Some(rect) = &fields.rect {
        sane("record", id, rect)?;
    }
    if let Some(Some(name)) = &fields.checkout {
        checkout_name(id, Some(name))?;
    }
    // Deleted elsewhere: a change must not bring it back.
    let Some(w) = desk.windows.iter_mut().find(|w| w.id == id) else {
        return Ok(false);
    };
    let did = merge_window(w, fields);
    if did {
        w.ts = now;
    }
    Ok(did)
}

fn merge_window(w: &mut DeskRecord, fields: WindowFields) -> bool {
    let mut did = false;
    if let Some(rect) = fields.rect {
        did |= put(&mut w.rect, rect);
    }
    if let Some(max) = fields.max {
        did |= put(&mut w.max, max);
    }
    if let Some(locked) = fields.locked {
        did |= put(&mut w.locked, locked);
    }
    if let Some(name) = fields.console_name {
        did |= put_console_name(w, name);
    }
    did |= put_done_mark(w, fields.done_mark);
    if let Some(checkout) = fields.checkout {
        did |= put(&mut w.checkout, checkout);
    }
    if let Some(s) = fields.session {
        did |= put_session(w, s);
    }
    did
}

/// A blank name is no name: the stored one stays.
fn put_console_name(w: &mut DeskRecord, name: String) -> bool {
    let mut probe = DeskRecord {
        console_name: Some(name),
        ..DeskRecord::default()
    };
    cap_console_name(&mut probe);
    match probe.console_name {
        Some(name) => put(&mut w.console_name, Some(name)),
        None => false,
    }
}

/// The mark that asks for a request at the next finished turn. A `set` that
/// does not name it leaves it as it is.
fn put_done_mark(w: &mut DeskRecord, on: Option<bool>) -> bool {
    on.is_some_and(|on| put(&mut w.done_mark, on))
}

fn put_session(w: &mut DeskRecord, s: SessionFields) -> bool {
    put(&mut w.session_id, s.session_id)
        | put(&mut w.daemon_id, s.daemon_id)
        | put(&mut w.environment, s.environment)
}

fn set_fence(
    desk: &mut DeskStore,
    id: &str,
    fields: FenceFields,
    now: i64,
) -> Result<bool, String> {
    if let Some(rect) = &fields.rect {
        sane("fence", id, rect)?;
    }
    let Some(f) = desk.fences.iter_mut().find(|f| f.id == id) else {
        return Ok(false);
    };
    let mut did = false;
    if let Some(rect) = fields.rect {
        did |= put(&mut f.rect, rect);
    }
    if let Some(name) = fields.name {
        did |= put(&mut f.name, name);
    }
    if let Some(locked) = fields.locked {
        did |= put(&mut f.locked, locked);
    }
    if did {
        f.ts = now;
    }
    Ok(did)
}

fn set_note(desk: &mut DeskStore, id: &str, fields: NoteFields, now: i64) -> Result<bool, String> {
    if let Some(rect) = &fields.rect {
        sane("note", id, rect)?;
    }
    if let Some(file) = &fields.file {
        checkout_name(id, file.checkout.as_deref())?;
    }
    let Some(n) = desk.notes.iter_mut().find(|n| n.id == id) else {
        return Ok(false);
    };
    let mut did = false;
    if let Some(rect) = fields.rect {
        did |= put(&mut n.rect, rect);
    }
    if let Some(locked) = fields.locked {
        did |= put(&mut n.locked, locked);
    }
    if let Some(file) = fields.file {
        did |= put(&mut n.repo, file.repo);
        did |= put(&mut n.path, file.path);
        did |= put(&mut n.checkout, file.checkout);
    }
    if did {
        n.ts = now;
    }
    Ok(did)
}

fn remove(desk: &mut DeskStore, kind: RecordType, id: &str) -> bool {
    let before = match kind {
        RecordType::Window => desk.windows.len(),
        RecordType::Fence => desk.fences.len(),
        RecordType::Note => desk.notes.len(),
    };
    let after = match kind {
        RecordType::Window => {
            desk.windows.retain(|w| w.id != id);
            desk.windows.len()
        }
        RecordType::Fence => {
            desk.fences.retain(|f| f.id != id);
            desk.fences.len()
        }
        RecordType::Note => {
            desk.notes.retain(|n| n.id != id);
            desk.notes.len()
        }
    };
    after != before
}

fn checkout(desk: &mut DeskStore, repo: String, name: String) -> Result<bool, String> {
    if crate::checkout::lexical(&name).is_none() {
        return Err(format!("checkout {name} for {repo} is not a valid name"));
    }
    if desk.checkouts.get(&repo) == Some(&name) {
        return Ok(false);
    }
    desk.checkouts.insert(repo, name);
    Ok(true)
}

fn checkout_clear(desk: &mut DeskStore, repo: &str, if_name: &str) -> bool {
    // Cleared only while it still holds the name the page saw go:
    // another device may have picked a new tree since.
    if desk.checkouts.get(repo).map(String::as_str) != Some(if_name) {
        return false;
    }
    desk.checkouts.remove(repo);
    true
}

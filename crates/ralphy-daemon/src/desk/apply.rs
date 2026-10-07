//! The desk upload as a list of **desk changes** (ADR-0050 amendment
//! 2026-10-04, changes, not the desk): the body type, the change shapes, and
//! [`apply`], which applies them to the stored desk in the order they arrive.
//!
//! Pure: the clock and the slug aliases are arguments. The browser draws its
//! pending changes with the same rules (`wb-desk-sync.ts` `applyChange`); one
//! table of cases, `ui-tests/fixtures/api-desk--apply-cases.json`, runs on both
//! sides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    cap_console_name, rect_is_sane, DeskFence, DeskNote, DeskRecord, DeskRect, DeskStore, DESK_MAX,
    FENCE_MAX, NOTE_MAX,
};

/// How many window records past [`DESK_MAX`] a `create` may still add. The
/// browser refuses a NEW console at the cap; a running console that lost its
/// record must still get one back.
pub const DESK_CREATE_SLACK: usize = 5;

/// The `PUT /api/desk` body: `{ seq, generation, changes }`.
///
/// MAP-ONLY, and that is the guard (#340). `#[derive(Deserialize)]` calls
/// `deserialize_struct`, which `serde_json` satisfies from a JSON SEQUENCE as
/// well as from an object, so `[1, 0, []]` would land as a valid body. Only
/// calling `deserialize_map` refuses it.
#[derive(Debug)]
pub struct DeskBody {
    /// The page's upload number. The daemon ignores a body whose `seq` is not
    /// higher than the last one it took from the same tab.
    pub seq: u64,
    /// The desk [`DeskStore::generation`] this page loaded.
    pub generation: u64,
    /// Each change is parsed on its own, so one bad change is refused alone.
    pub changes: Vec<serde_json::Value>,
}

impl<'de> Deserialize<'de> for DeskBody {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            seq: u64,
            #[serde(default)]
            generation: u64,
            changes: Vec<serde_json::Value>,
        }

        struct MapOnly;
        impl<'de> serde::de::Visitor<'de> for MapOnly {
            type Value = DeskBody;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a desk upload object with `seq` and `changes`")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<Self::Value, A::Error> {
                let f = Fields::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(DeskBody {
                    seq: f.seq,
                    generation: f.generation,
                    changes: f.changes,
                })
            }
        }

        d.deserialize_map(MapOnly)
    }
}

/// `Some(value)` for a key that is present, `null` included, so a `set` can
/// say "the primary tree" (`checkout: null`) and an absent key says nothing.
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The session of a window, as one unit: a reconnect changes all three.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionFields {
    #[serde(default)]
    pub session_id: Option<u64>,
    #[serde(default)]
    pub daemon_id: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
}

/// The fields a `set` may change on a window. Any other key is refused.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WindowFields {
    #[serde(default)]
    pub rect: Option<DeskRect>,
    #[serde(default)]
    pub max: Option<bool>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub console_name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub checkout: Option<Option<String>>,
    #[serde(default)]
    pub session: Option<SessionFields>,
}

/// The fields a `set` may change on a fence.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FenceFields {
    #[serde(default)]
    pub rect: Option<DeskRect>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub locked: Option<bool>,
}

/// The file of a note card, as one unit: its identity is `(repo, checkout,
/// path)` (ADR-0064 §2).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileFields {
    pub repo: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub checkout: Option<String>,
}

/// The fields a `set` may change on a note card.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NoteFields {
    #[serde(default)]
    pub rect: Option<DeskRect>,
    #[serde(default)]
    pub locked: Option<bool>,
    #[serde(default)]
    pub file: Option<FileFields>,
}

/// The three record types a change names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordType {
    Window,
    Fence,
    Note,
}

/// One parsed desk change.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    CreateWindow(DeskRecord),
    CreateFence(DeskFence),
    CreateNote(DeskNote),
    SetWindow { id: String, fields: WindowFields },
    SetFence { id: String, fields: FenceFields },
    SetNote { id: String, fields: NoteFields },
    Remove { kind: RecordType, id: String },
    Checkout { repo: String, name: String },
    CheckoutClear { repo: String, if_name: String },
}

/// Parse one change. The `op` key picks the shape; every shape refuses a key
/// it does not know.
pub fn parse(v: &serde_json::Value) -> Result<Change, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Create {
        #[serde(rename = "type")]
        kind: RecordType,
        record: serde_json::Value,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Set {
        #[serde(rename = "type")]
        kind: RecordType,
        id: String,
        fields: serde_json::Value,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Remove {
        #[serde(rename = "type")]
        kind: RecordType,
        id: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Checkout {
        repo: String,
        name: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct CheckoutClear {
        repo: String,
        if_name: String,
    }

    fn rest<T: serde::de::DeserializeOwned>(v: &serde_json::Value) -> Result<T, String> {
        let mut map = v.as_object().cloned().unwrap_or_default();
        map.remove("op");
        serde_json::from_value(serde_json::Value::Object(map)).map_err(|e| e.to_string())
    }
    fn typed<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    let op = v
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "a change needs an `op`".to_string())?;
    match op {
        "create" => {
            let c: Create = rest(v)?;
            Ok(match c.kind {
                RecordType::Window => Change::CreateWindow(typed(c.record)?),
                RecordType::Fence => Change::CreateFence(typed(c.record)?),
                RecordType::Note => Change::CreateNote(typed(c.record)?),
            })
        }
        "set" => {
            let s: Set = rest(v)?;
            Ok(match s.kind {
                RecordType::Window => Change::SetWindow {
                    id: s.id,
                    fields: typed(s.fields)?,
                },
                RecordType::Fence => Change::SetFence {
                    id: s.id,
                    fields: typed(s.fields)?,
                },
                RecordType::Note => Change::SetNote {
                    id: s.id,
                    fields: typed(s.fields)?,
                },
            })
        }
        "remove" => {
            let r: Remove = rest(v)?;
            Ok(Change::Remove {
                kind: r.kind,
                id: r.id,
            })
        }
        "checkout" => {
            let c: Checkout = rest(v)?;
            Ok(Change::Checkout {
                repo: c.repo,
                name: c.name,
            })
        }
        "checkout-clear" => {
            let c: CheckoutClear = rest(v)?;
            Ok(Change::CheckoutClear {
                repo: c.repo,
                if_name: c.if_name,
            })
        }
        other => Err(format!("unknown op {other}")),
    }
}

/// A change the daemon skipped, by its index in the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub index: usize,
    pub error: String,
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub desk: DeskStore,
    /// Whether any change altered the desk. `false` writes nothing, raises no
    /// `rev` and pushes nothing.
    pub changed: bool,
    pub refused: Vec<Refusal>,
}

/// Apply `changes` to `desk` in order, at `now`. Each change is parsed,
/// checked and applied on its own: a refused change is reported and the rest
/// still apply. Every repo key goes through `aliases` (former slug → its
/// canonical key) first. A record a change alters gets `ts = now`.
pub fn apply(
    mut desk: DeskStore,
    changes: &[serde_json::Value],
    aliases: &BTreeMap<String, String>,
    now: i64,
) -> Applied {
    let mut changed = false;
    let mut refused = Vec::new();
    for (index, raw) in changes.iter().enumerate() {
        let outcome = parse(raw).and_then(|c| apply_one(&mut desk, rekey(c, aliases), now));
        match outcome {
            Ok(did) => changed |= did,
            Err(error) => refused.push(Refusal { index, error }),
        }
    }
    Applied {
        desk,
        changed,
        refused,
    }
}

fn canonical(repo: String, aliases: &BTreeMap<String, String>) -> String {
    aliases.get(&repo).cloned().unwrap_or(repo)
}

fn rekey(change: Change, aliases: &BTreeMap<String, String>) -> Change {
    match change {
        Change::CreateWindow(mut r) => {
            r.repo = canonical(r.repo, aliases);
            Change::CreateWindow(r)
        }
        Change::CreateNote(mut n) => {
            n.repo = canonical(n.repo, aliases);
            Change::CreateNote(n)
        }
        Change::SetNote { id, mut fields } => {
            if let Some(file) = fields.file.as_mut() {
                file.repo = canonical(std::mem::take(&mut file.repo), aliases);
            }
            Change::SetNote { id, fields }
        }
        Change::Checkout { repo, name } => Change::Checkout {
            repo: canonical(repo, aliases),
            name,
        },
        Change::CheckoutClear { repo, if_name } => Change::CheckoutClear {
            repo: canonical(repo, aliases),
            if_name,
        },
        other => other,
    }
}

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

fn apply_one(desk: &mut DeskStore, change: Change, now: i64) -> Result<bool, String> {
    match change {
        Change::CreateWindow(mut r) => {
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
        Change::CreateFence(mut f) => {
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
        Change::CreateNote(mut n) => {
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
        Change::SetWindow { id, fields } => {
            if let Some(rect) = &fields.rect {
                sane("record", &id, rect)?;
            }
            if let Some(Some(name)) = &fields.checkout {
                checkout_name(&id, Some(name))?;
            }
            // Deleted elsewhere: a change must not bring it back.
            let Some(w) = desk.windows.iter_mut().find(|w| w.id == id) else {
                return Ok(false);
            };
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
            // A blank name is no name: the stored one stays.
            if let Some(name) = fields.console_name {
                let mut probe = DeskRecord {
                    console_name: Some(name),
                    ..DeskRecord::default()
                };
                cap_console_name(&mut probe);
                if let Some(name) = probe.console_name {
                    did |= put(&mut w.console_name, Some(name));
                }
            }
            if let Some(checkout) = fields.checkout {
                did |= put(&mut w.checkout, checkout);
            }
            if let Some(s) = fields.session {
                did |= put(&mut w.session_id, s.session_id);
                did |= put(&mut w.daemon_id, s.daemon_id);
                did |= put(&mut w.environment, s.environment);
            }
            if did {
                w.ts = now;
            }
            Ok(did)
        }
        Change::SetFence { id, fields } => {
            if let Some(rect) = &fields.rect {
                sane("fence", &id, rect)?;
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
        Change::SetNote { id, fields } => {
            if let Some(rect) = &fields.rect {
                sane("note", &id, rect)?;
            }
            if let Some(file) = &fields.file {
                checkout_name(&id, file.checkout.as_deref())?;
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
        Change::Remove { kind, id } => {
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
            Ok(after != before)
        }
        Change::Checkout { repo, name } => {
            if crate::checkout::lexical(&name).is_none() {
                return Err(format!("checkout {name} for {repo} is not a valid name"));
            }
            if desk.checkouts.get(&repo) == Some(&name) {
                return Ok(false);
            }
            desk.checkouts.insert(repo, name);
            Ok(true)
        }
        Change::CheckoutClear { repo, if_name } => {
            // Cleared only while it still holds the name the page saw go:
            // another device may have picked a new tree since.
            if desk.checkouts.get(&repo) != Some(&if_name) {
                return Ok(false);
            }
            desk.checkouts.remove(&repo);
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests;

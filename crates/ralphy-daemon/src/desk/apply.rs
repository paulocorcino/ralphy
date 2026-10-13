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

use super::{DeskFence, DeskNote, DeskRecord, DeskRect, DeskStore};

mod changes;
use changes::apply_one;

/// How many window records past [`DESK_MAX`](super::DESK_MAX) a `create` may still add. The
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

#[cfg(test)]
mod tests;

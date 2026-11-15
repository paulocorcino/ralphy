//! The files of an HTML document whose scripts the operator allowed
//! (ADR-0089): the tokens, the closed list of file kinds, and the policy every
//! answer carries. `routes/preview.rs` serves them.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::{confine, tree};

/// A token with no request for this long is gone (D3).
pub(crate) const IDLE_EXPIRY: Duration = Duration::from_mins(30);

/// The most tokens alive at once: the oldest one makes room for a new one.
pub(crate) const MAX_PREVIEWS: usize = 32;

/// The tokens of this daemon. One store for the process, as the command
/// slots are: the verb makes a token and the route reads it.
pub(crate) static PREVIEWS: LazyLock<Previews> = LazyLock::new(Previews::default);

struct Grant {
    token: String,
    root: PathBuf,
    last_used: Instant,
}

/// The tokens alive, oldest first. They live in memory only.
#[derive(Default)]
pub(crate) struct Previews {
    grants: Mutex<Vec<Grant>>,
}

impl Previews {
    /// A new token for the files under `root`.
    pub(crate) fn open(&self, root: PathBuf, now: Instant) -> String {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("the OS CSPRNG must be available to mint a token");
        let token = data_encoding::HEXLOWER.encode(&bytes);
        let mut grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        grants.retain(|g| now.duration_since(g.last_used) < IDLE_EXPIRY);
        if grants.len() >= MAX_PREVIEWS {
            grants.remove(0);
        }
        grants.push(Grant {
            token: token.clone(),
            root,
            last_used: now,
        });
        token
    }

    /// The root of a live token. A read counts as a request of the token.
    pub(crate) fn root_of(&self, token: &str, now: Instant) -> Option<PathBuf> {
        let mut grants = self.grants.lock().unwrap_or_else(PoisonError::into_inner);
        let grant = grants.iter_mut().find(|g| g.token == token)?;
        if now.duration_since(grant.last_used) >= IDLE_EXPIRY {
            return None;
        }
        grant.last_used = now;
        Some(grant.root.clone())
    }

    /// End a token. An unknown token is already ended.
    pub(crate) fn close(&self, token: &str) {
        self.grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|g| g.token != token);
    }
}

/// The cap of one text or font file of the route (ADR-0072 D11). It is
/// larger than [`tree::MAX_READ_BYTES`], the cap of a file an editor opens: a
/// vendored library is one file (measured: `mermaid.min.js` 11.x is 3.5 MB).
/// The file goes out as the body of its own answer, never inside a command.
pub(crate) const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

/// The media type of a text or font file the route serves, by its extension.
/// HTML has no `charset`: the document names its own.
fn plain_type(extension: &str) -> Option<&'static str> {
    match extension {
        "html" | "htm" => Some("text/html"),
        "css" => Some("text/css; charset=utf-8"),
        "js" | "mjs" => Some("text/javascript; charset=utf-8"),
        "json" => Some("application/json"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        _ => None,
    }
}

/// One file of the closed list (D3): its media type and its bytes. `None` is
/// every refusal: a kind outside the list, a path outside `root`, a file over
/// its cap, or no file.
pub(crate) fn read(root: &Path, rel: &str) -> Option<(&'static str, Vec<u8>)> {
    let extension = Path::new(rel).extension()?.to_str()?.to_ascii_lowercase();
    let Some(media_type) = plain_type(&extension) else {
        let image = tree::read_image(root, rel).ok()?;
        return Some((image.media_type, image.bytes));
    };
    let path = confine::confine(root, rel).ok()?;
    let size = std::fs::metadata(&path).ok()?.len();
    if size > MAX_PREVIEW_BYTES {
        return None;
    }
    Some((media_type, std::fs::read(&path).ok()?))
}

/// What a document may load from the web: each is an option of the daemon,
/// off by default. Never a request a script makes (`connect-src`).
#[derive(Clone, Copy, Default)]
pub(crate) struct Web {
    /// Images (ADR-0032 amendment §F).
    pub(crate) images: bool,
    /// Scripts, styles and fonts (ADR-0089, amendment of 2026-11-16).
    pub(crate) content: bool,
}

/// The policy of every answer of the route (D4). `prefix` is the address of
/// the route with its token, as the browser names it.
pub(crate) fn policy(prefix: &str, web: Web) -> String {
    let images = if web.images { " https:" } else { "" };
    let content = if web.content { " https:" } else { "" };
    format!(
        "sandbox allow-scripts; default-src 'none'; \
         script-src 'unsafe-inline' 'unsafe-eval' {prefix}{content}; \
         style-src 'unsafe-inline' {prefix}{content}; img-src data: {prefix}{images}; \
         font-src {prefix}{content}; connect-src 'none'; form-action 'none'; base-uri 'none'; \
         frame-ancestors 'self'"
    )
}

#[cfg(test)]
mod tests;

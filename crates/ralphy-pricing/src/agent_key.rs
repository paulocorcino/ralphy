//! The price-table key of an agent's model id. It lives here, not in the
//! adapter, because the daemon prices usage and imports no adapter
//! (ADR-0032 §10).

/// The suffixes `agy models` puts after a base id (ADR-0086 D4).
const ANTIGRAVITY_EFFORT_SUFFIXES: [&str; 3] = ["-low", "-medium", "-high"];

/// The base id of an Antigravity model id that ends in an effort, else `None`.
pub fn antigravity_base_id(id: &str) -> Option<&str> {
    ANTIGRAVITY_EFFORT_SUFFIXES
        .iter()
        .find_map(|suffix| id.strip_suffix(suffix))
}

/// The key a model id of `agent` prices under. Only an Antigravity id loses its
/// effort suffix: every effort of one `agy` model has one price, and the id of
/// another vendor may end in `-high` and name another model.
pub fn price_key<'a>(agent: &str, model: &'a str) -> &'a str {
    if agent != "antigravity" {
        return model;
    }
    let id = model.trim();
    antigravity_base_id(id).unwrap_or(id)
}

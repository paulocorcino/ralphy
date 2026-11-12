//! The model and effort rules of `agy` (ADR-0086 D4). The effort is a separate
//! axis of a base id (`gemini-3.8-flash` with `--effort low`), and the id
//! suffix names the same axis (`gemini-3.8-flash-low`).

/// The suffixes `agy models` puts after a base id.
const EFFORT_SUFFIXES: [&str; 3] = ["-low", "-medium", "-high"];

/// The base id of `id` when its name ends in an effort, else `None`.
fn base_of(id: &str) -> Option<&str> {
    EFFORT_SUFFIXES
        .iter()
        .find_map(|suffix| id.strip_suffix(suffix))
}

/// The level to pass as `--effort` for this model and effort word. An id that
/// already ends in an effort gets none, because `agy` refuses a pair that
/// conflicts. With no `--model`, no `--effort` either: the default model of the
/// operator's plan is kept whole.
pub(crate) fn effort_flag<'a>(model: Option<&str>, effort: Option<&'a str>) -> Option<&'a str> {
    let model = model?.trim();
    if base_of(model).is_some() {
        return None;
    }
    effort
}

/// The price-table key `id` bills under: the id without its effort suffix,
/// because every effort of one model has the same price.
pub fn price_key(id: &str) -> String {
    let id = id.trim();
    base_of(id).unwrap_or(id).to_owned()
}

#[cfg(test)]
mod tests;

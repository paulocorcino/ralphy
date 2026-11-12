//! The model list of `agy models`: one `id<TAB>label` line per model
//! (ADR-0086 D4). The list is plan-scoped, so Ralphy reads it and pins none.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used once the Agent impl lands")
)]

/// One model `agy` offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelEntry {
    /// The id `--model` takes, such as `gemini-3.8-flash-low`.
    pub(crate) id: String,
    /// The label `agy` shows, such as `Gemini 3.8 Flash (Low)`.
    pub(crate) label: String,
}

/// The models in `stdout`, in order. A line with no tab, or with an empty id
/// or label, is skipped.
pub(crate) fn parse(stdout: &str) -> Vec<ModelEntry> {
    stdout
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(id, label)| (id.trim(), label.trim()))
        .filter(|(id, label)| !id.is_empty() && !label.is_empty())
        .map(|(id, label)| ModelEntry {
            id: id.to_owned(),
            label: label.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests;

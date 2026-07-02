//! Reading a plan artifact: counting actionable steps. Pure functions over the
//! plan markdown, shared by the adapters that write plans and the core that
//! decides feasibility. Any model/tier judgment a planner emits is parsed by
//! the adapter that understands its vocabulary, never here (ADR-0002).

use regex::Regex;

/// Count open `- [ ]` checklist steps. Checked (`- [x]`) steps do not count.
pub fn count_open_steps(md: &str) -> usize {
    let re = Regex::new(r"(?m)^\s*-\s*\[ \]").expect("valid regex");
    re.find_iter(md).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_open_steps() {
        // Open steps at line start (leading indent allowed); checked steps and
        // inline `- [ ]` text do not count. Mirrors the ps1 `^\s*-\s*\[ \]` oracle.
        let md = "## Steps\n- [ ] one\n  - [ ] nested\n- [x] done\nsee - [ ] inline\n";
        assert_eq!(count_open_steps(md), 2);
    }

    #[test]
    fn no_steps_is_zero() {
        assert_eq!(count_open_steps("# Plan\n\n## Feasible: no\n"), 0);
    }
}

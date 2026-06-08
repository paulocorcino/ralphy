//! Reading a plan artifact: counting actionable steps and the optional
//! complexity judgment. Pure functions over the plan markdown, shared by the
//! adapter that writes the plan and the core that decides feasibility.

use regex::Regex;

/// Count open `- [ ]` checklist steps. Checked (`- [x]`) steps do not count.
pub fn count_open_steps(md: &str) -> usize {
    let re = Regex::new(r"(?m)^\s*-\s*\[ \]").expect("valid regex");
    re.find_iter(md).count()
}

/// The planner's `## Execution model: sonnet|opus` judgment, lowercased, if any.
pub fn recommended_model(md: &str) -> Option<String> {
    let re = Regex::new(r"(?im)^\s*##\s*Execution model:\s*(opus|sonnet)").expect("valid regex");
    re.captures(md).map(|c| c[1].to_lowercase())
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

    #[test]
    fn reads_recommended_model() {
        assert_eq!(
            recommended_model("## Execution model: Opus\nbecause").as_deref(),
            Some("opus")
        );
        assert_eq!(recommended_model("no judgment here"), None);
    }
}

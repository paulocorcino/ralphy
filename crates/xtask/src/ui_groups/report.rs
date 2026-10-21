use std::collections::{BTreeMap, BTreeSet};

use super::{Binding, Group, Shell, APP, HTML};

/// What one group holds and shares.
#[derive(Default)]
pub(super) struct Stats {
    pub(super) members: Vec<usize>,
    lines: usize,
    /// Members of this group read by code of another group, with those groups.
    pub(super) read_outside: BTreeMap<String, BTreeSet<Group>>,
    /// Members of this group written by code of another group.
    pub(super) written_outside: BTreeMap<String, BTreeSet<Group>>,
    bindings: usize,
    only_this: usize,
}

pub(super) fn stats(shell: &Shell, bindings: &[Binding]) -> BTreeMap<Group, Stats> {
    let mut by: BTreeMap<Group, Stats> =
        Group::ALL.iter().map(|&g| (g, Stats::default())).collect();
    let index: BTreeMap<&str, usize> = shell
        .members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    for (n, m) in shell.members.iter().enumerate() {
        let s = by.entry(shell.group(n)).or_default();
        s.members.push(n);
        s.lines += m.last + 1 - m.from;
    }
    for a in &shell.accesses {
        let Some(&target) = index.get(a.name.as_str()) else {
            continue;
        };
        let (owner, from) = (shell.group(target), shell.group(a.within));
        if owner == from {
            continue;
        }
        let s = by.entry(owner).or_default();
        let list = if a.write {
            &mut s.written_outside
        } else {
            &mut s.read_outside
        };
        list.entry(a.name.clone()).or_default().insert(from);
    }
    for b in bindings {
        let groups = binding_groups(shell, &index, b);
        for &g in &groups {
            let s = by.entry(g).or_default();
            s.bindings += 1;
            if groups.len() == 1 {
                s.only_this += 1;
            }
        }
    }
    by
}

fn binding_groups(shell: &Shell, index: &BTreeMap<&str, usize>, b: &Binding) -> BTreeSet<Group> {
    b.names
        .iter()
        .filter_map(|n| index.get(n.as_str()))
        .map(|&m| shell.group(m))
        .collect()
}

pub(super) fn render(shell: &Shell, bindings: &[Binding], verbose: bool) -> String {
    let by = stats(shell, bindings);
    let index: BTreeMap<&str, usize> = shell
        .members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect();
    let named = bindings.iter().filter(|b| !b.names.is_empty()).count();
    let mut mixed = 0;
    let mut mixed_features = Vec::new();
    for b in bindings {
        let groups = binding_groups(shell, &index, b);
        if groups.len() >= 2 {
            mixed += 1;
        }
        if groups.iter().filter(|&&g| g != Group::Core).count() >= 2 {
            mixed_features.push(b);
        }
    }

    let mut o = String::new();
    let mut line = |text: String| {
        o.push_str(&text);
        o.push('\n');
    };
    line(format!(
        "ui-groups — the members of shell() in {APP}, by the section heading they sit under\n"
    ));
    line(format!(
        "{} members in {} sections; {HTML}: {} bindings in shell() scope, {named} name a member\n",
        shell.members.len(),
        shell.sections.len(),
        bindings.len()
    ));
    line(format!(
        "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
        "group", "members", "lines", "read outside", "written outside", "bindings", "only this"
    ));
    for (g, s) in &by {
        line(format!(
            "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
            g.label(),
            s.members.len(),
            s.lines,
            s.read_outside.len(),
            s.written_outside.len(),
            s.bindings,
            s.only_this
        ));
    }
    line(format!(
        "\nbindings that mix groups: {mixed}; that mix two feature groups: {}",
        mixed_features.len()
    ));
    for b in &mixed_features {
        let names: Vec<&str> = b.names.iter().map(String::as_str).collect();
        line(format!("  index.html:{} [{}]", b.line, names.join(", ")));
    }

    line("\nsections:".to_string());
    for (k, s) in shell.sections.iter().enumerate() {
        let count = shell.members.iter().filter(|m| m.section == k).count();
        line(format!(
            "  {:>5}  {:<9} {:>4} members  {}",
            s.line,
            s.group.short(),
            count,
            s.name
        ));
    }

    let named_list = |list: &BTreeMap<String, BTreeSet<Group>>| -> String {
        list.iter()
            .map(|(n, gs)| {
                let gs: Vec<&str> = gs.iter().map(|g| g.short()).collect();
                format!("{n} [{}]", gs.join(", "))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (g, s) in &by {
        line(format!(
            "\n## {} ({} members, {} lines)",
            g.label(),
            s.members.len(),
            s.lines
        ));
        let names: Vec<&str> = s
            .members
            .iter()
            .map(|&n| shell.members[n].name.as_str())
            .collect();
        line(format!("members: {}", names.join(", ")));
        line(format!(
            "read from outside ({}): {}",
            s.read_outside.len(),
            named_list(&s.read_outside)
        ));
        line(format!(
            "written from outside ({}): {}",
            s.written_outside.len(),
            named_list(&s.written_outside)
        ));
        if verbose {
            for &n in &s.members {
                let m = &shell.members[n];
                line(format!(
                    "  {:>5}-{:<5} {:<7} {:<32} {}",
                    m.first,
                    m.last,
                    format!("{:?}", m.kind).to_lowercase(),
                    m.name,
                    shell.sections[m.section].name
                ));
            }
        }
    }
    o
}

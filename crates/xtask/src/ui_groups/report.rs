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
    let index = member_index(shell);
    count_members(shell, &mut by);
    count_outside_accesses(shell, &index, &mut by);
    count_bindings(shell, &index, bindings, &mut by);
    by
}

/// The position of each member of `shell()`, by name.
fn member_index(shell: &Shell) -> BTreeMap<&str, usize> {
    shell
        .members
        .iter()
        .enumerate()
        .map(|(n, m)| (m.name.as_str(), n))
        .collect()
}

/// The members of each group and the lines they span.
fn count_members(shell: &Shell, by: &mut BTreeMap<Group, Stats>) {
    for (n, m) in shell.members.iter().enumerate() {
        let s = by.entry(shell.group(n)).or_default();
        s.members.push(n);
        s.lines += m.last + 1 - m.from;
    }
}

/// The members of each group that code of another group reads or writes.
fn count_outside_accesses(
    shell: &Shell,
    index: &BTreeMap<&str, usize>,
    by: &mut BTreeMap<Group, Stats>,
) {
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
}

/// The bindings that name a member of each group, and those that name no
/// other group.
fn count_bindings(
    shell: &Shell,
    index: &BTreeMap<&str, usize>,
    bindings: &[Binding],
    by: &mut BTreeMap<Group, Stats>,
) {
    for b in bindings {
        let groups = binding_groups(shell, index, b);
        for &g in &groups {
            let s = by.entry(g).or_default();
            s.bindings += 1;
            if groups.len() == 1 {
                s.only_this += 1;
            }
        }
    }
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
    let mut o = String::new();
    render_summary(&mut o, shell, bindings, &by);
    render_sections(&mut o, shell);
    for (g, s) in &by {
        render_group(&mut o, shell, *g, s, verbose);
    }
    o
}

fn line(o: &mut String, text: String) {
    o.push_str(&text);
    o.push('\n');
}

/// The number of bindings that mix groups, and the bindings that mix two
/// feature groups.
fn mixed_bindings<'b>(shell: &Shell, bindings: &'b [Binding]) -> (usize, Vec<&'b Binding>) {
    let index = member_index(shell);
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
    (mixed, mixed_features)
}

/// The head of the report: the counts, one row per group, and the bindings
/// that mix two feature groups.
fn render_summary(
    o: &mut String,
    shell: &Shell,
    bindings: &[Binding],
    by: &BTreeMap<Group, Stats>,
) {
    let named = bindings.iter().filter(|b| !b.names.is_empty()).count();
    let (mixed, mixed_features) = mixed_bindings(shell, bindings);
    line(
        o,
        format!(
            "ui-groups — the members of shell() in {APP}, by the section heading they sit under\n"
        ),
    );
    line(
        o,
        format!(
            "{} members in {} sections; {HTML}: \
             {} bindings in shell() scope, {named} name a member\n",
            shell.members.len(),
            shell.sections.len(),
            bindings.len()
        ),
    );
    line(
        o,
        format!(
            "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
            "group", "members", "lines", "read outside", "written outside", "bindings", "only this"
        ),
    );
    for (g, s) in by {
        line(
            o,
            format!(
                "{:<26}{:>8}{:>7}{:>16}{:>19}{:>10}{:>11}",
                g.label(),
                s.members.len(),
                s.lines,
                s.read_outside.len(),
                s.written_outside.len(),
                s.bindings,
                s.only_this
            ),
        );
    }
    line(
        o,
        format!(
            "\nbindings that mix groups: {mixed}; that mix two feature groups: {}",
            mixed_features.len()
        ),
    );
    for b in &mixed_features {
        let names: Vec<&str> = b.names.iter().map(String::as_str).collect();
        line(o, format!("  index.html:{} [{}]", b.line, names.join(", ")));
    }
}

/// Each section heading with its group and its number of members.
fn render_sections(o: &mut String, shell: &Shell) {
    line(o, "\nsections:".to_string());
    for (k, s) in shell.sections.iter().enumerate() {
        let count = shell.members.iter().filter(|m| m.section == k).count();
        line(
            o,
            format!(
                "  {:>5}  {:<9} {:>4} members  {}",
                s.line,
                s.group.short(),
                count,
                s.name
            ),
        );
    }
}

/// Each name of `list` with the groups it is shared with.
fn named_list(list: &BTreeMap<String, BTreeSet<Group>>) -> String {
    list.iter()
        .map(|(n, gs)| {
            let gs: Vec<&str> = gs.iter().map(|g| g.short()).collect();
            format!("{n} [{}]", gs.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The part of one group: its members and what other groups read and write
/// of it; with `verbose`, one line per member.
fn render_group(o: &mut String, shell: &Shell, g: Group, s: &Stats, verbose: bool) {
    line(
        o,
        format!(
            "\n## {} ({} members, {} lines)",
            g.label(),
            s.members.len(),
            s.lines
        ),
    );
    let names: Vec<&str> = s
        .members
        .iter()
        .map(|&n| shell.members[n].name.as_str())
        .collect();
    line(o, format!("members: {}", names.join(", ")));
    line(
        o,
        format!(
            "read from outside ({}): {}",
            s.read_outside.len(),
            named_list(&s.read_outside)
        ),
    );
    line(
        o,
        format!(
            "written from outside ({}): {}",
            s.written_outside.len(),
            named_list(&s.written_outside)
        ),
    );
    if verbose {
        render_member_lines(o, shell, s);
    }
}

/// One line per member of a group: its lines, kind, name and section.
fn render_member_lines(o: &mut String, shell: &Shell, s: &Stats) {
    for &n in &s.members {
        let m = &shell.members[n];
        line(
            o,
            format!(
                "  {:>5}-{:<5} {:<7} {:<32} {}",
                m.first,
                m.last,
                format!("{:?}", m.kind).to_lowercase(),
                m.name,
                shell.sections[m.section].name
            ),
        );
    }
}

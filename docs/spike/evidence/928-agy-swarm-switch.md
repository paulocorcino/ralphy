# Live probe: a switch that takes `swarm` out of the skill list (#928)

Item 5 of #928. In the #784 capstone, `agy -p /skills` still listed `swarm`
in the owned root, although `config/config.json` held
`{"plugins":{"swarm":{"enabled":false}}}`
([784-antigravity-capstone-live.md](./784-antigravity-capstone-live.md)).
This probe looks for a switch that does take it out. The raw log is
[`antigravity-928-swarm.log`](../live/antigravity-928-swarm.log) (`<home>` is
the user folder, `<R>` a temp root).

Host: Windows 11 Pro 10.0.26300, `agy` 1.5.8, date 2026-11-13. Every call ran
with `--gemini_dir=<a new temp folder>`, so the operator's roots were not
touched. `/skills` makes no model call. Two model turns with
`gemini-3.8-flash-low` were used, of the three allowed. Every temp root was
deleted after the probe. WSL was not probed.

## What `swarm` is

- A fresh root extracts two things named swarm: the plugin
  `antigravity-cli/builtin/plugins/swarm` (a `plugin.json` and the sidecar
  `swarm_board`, no skill) and the built-in skill
  `antigravity-cli/builtin/skills/native_swarm/SKILL.md`, whose front matter
  is `name: swarm`. The skill is what `/skills` lists, so the plugin switch
  of `config.json` does not reach it.
- The first `/skills` of a fresh root sometimes misses `swarm` (three of
  nine first runs, and the second run of one root); every run after a
  model turn or a listed `swarm` lists it. Each candidate was applied
  after two seed runs and judged on two runs after it.

## Commands and results

- `plugin --help`: the subcommands are `list`, `import`, `install`,
  `uninstall`, `enable`, `disable`, `validate`, `link`. They have no
  `--help` of their own: each reads `--help` as its argument.
- `plugin list --json`: `[]`. The built-in `swarm` plugin is not in the list.
- `plugin disable swarm`: `Error: plugin "swarm" not found or invalid`,
  exit 1; `swarm` still listed. No command switch.
- `config/config.json` with `plugins.swarm.enabled` set to `false`: `swarm`
  still listed (the #784 result again).
- `config/plugins.json` with `{"exclude": ["swarm"]}`: `swarm` still listed.
- `antigravity-cli/builtin/plugins/swarm` removed: `swarm` still listed;
  `agy` did not extract the folder again.
- `config/skills.json` with `{"exclude": ["swarm"]}`: `swarm` still listed.
- `config/skills.json` with `{"exclude": ["native_swarm"]}`: `swarm` is gone
  from `/skills` on both runs after it; the other skills stay. The same with
  `{"exclude": ["native_swarm", "swarm"]}`. `agy` matches the pattern
  against the skill's folder name, not against its `name:`, as the built-in
  doc `agy-customizations/docs/json_configs.md` says for a top-level
  `exclude`.
- Binary scan (read only, 1 MB chunks): no setting named for swarm; the
  strings name `plugins.json`, `skills.json`, `enabledPlugins` (the import
  of another agent's settings) and `builtin_plugin_names`.
- Model turn in the root with `native_swarm` excluded, asked to list its
  skills: `agy-customizations`, `antigravity-guide`, `swarm_monitor`. The
  list is partial, so it does not show whether the swarm tools are still
  offered to the model; that stays unmeasured.

Result: switch found: `config/skills.json` in the root with `{"exclude": ["native_swarm"]}`

## What changed in Ralphy

`root::ensure` adds `native_swarm` to the top-level `exclude` of the owned
root's `config/skills.json` and keeps every other key, next to the existing
`plugins.swarm.enabled: false` of `config.json` (kept: it turns off the
`swarm_board` sidecar plugin, which `/skills` does not show). The test is
`prepare_root_takes_swarm_out_of_the_skill_list`. The charter rule of
`overlay.antigravity.md` ("no delegation, no background task") stays: the
other built-in skills have no switch, and the swarm tools were not measured.

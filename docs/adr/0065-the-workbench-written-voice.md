# The workbench's written voice: one casing, one verb per act, and rules a lint can read

Status: **accepted** (2026-09-24). Decided in the style-guide session (#424) of
PRD #422, working from the inventory that #423 built. The rules that a machine
can check are in [`docs/ui-copy-rules.json`](../ui-copy-rules.json). The copy
lint (#425) reads that file, and the editorial passes (#426–#430) apply this
ADR.

[ADR-0035](./0035-daemon-ui-visual-language.md) decides how the workbench
looks. Nothing decided how it speaks. The inventory
(`cargo run -p xtask -- ui-copy`, measured at `53be8d22`) found 662 texts in
13 files, and they do not agree with each other:

- **Casing is split.** Buttons and dialogs mostly start with a capital
  (dialogs: 44 upper, 2 lower). Tooltips mostly do not (tooltips set from
  JavaScript: 2 upper, 47 lower). A bare `close` is the tooltip of 16
  controls.
- **Verbs compete.** Delete, Remove and Discard (12, 16, 10 texts) are used
  for overlapping acts. `app.js:2022` says "Delete this plan" and
  `app.js:2031` says "Discard this plan?" for the same act.
- **Errors take three shapes.** `could not load projects from the daemon`,
  `Cannot save as {p.encoding}` and `Repo unreachable. Cannot switch branch.`
  all report a failure.
- **Words drift from the glossary.** The UI says "fences on the plane", but
  [CONTEXT.md](../../CONTEXT.md) calls that surface the **Stage**.

This ADR decides one convention for each of these. English is the only
language. A translation catalog was rejected by the PRD (#422), and that
decision is not reopened here.

## Decision

### 1. The rules live in two places, and they change together

This ADR holds the rules as prose, with a worked example for each one.
`docs/ui-copy-rules.json` holds the subset that a lint can check without
reading prose: the casing rule and its exemptions, the proper nouns, the ban
list, the banned punctuation, the sentence length limit and the plain-word
list (§10), the extra copy helpers, and the verb table (the
verb table is for people; the lint does not check verb meaning). A change to
a rule changes both files in the same commit. When the two disagree, this ADR
is correct and the JSON is fixed.

ADR-0035 stays about the visual language only.

### 2. Casing: sentence case everywhere

Every text starts with a capital letter. After the first word, only proper
nouns are capitalized. There is no Title Case. This includes buttons, tabs,
headings, column headers, menu items, dialog titles, placeholders, `title`
tooltips and `aria-label`s.

| Before | After | Where |
|---|---|---|
| `close` | `Close` | `index.html:1804` (and 15 more) |
| `dismiss` | `Dismiss` | `wb-console.js:245` |
| `Staged Changes` | `Staged changes` | `index.html:515` |
| `kind` | `Kind` | `wb-spend.js:335` (ledger column) |
| `stop` | `Stop` | `index.html:1613` (a button) |

A text that starts with a `{placeholder}`, a quote, a bracket or a symbol is
judged by its first word.

Three kinds of text stay lowercase, and each is listed in the JSON:

- **Terminal notices** (kind `js:term-notice`): `[session closed]`. They are
  printed inside the terminal, next to the program's own output.
- **State words** shown alone in a chip or a badge (`state_words`): `idle`,
  `primary`, `waking`, `lost`. They describe a state. They are not a label that
  starts a sentence. When the same word is a button, it is capitalized:
  `current` on the branch-mode button (`index.html:2514`) becomes `Current`.
- **Key names** on the key bar (`key_names`): `esc`, `tab`, `ctrl`, `sel`.
  They copy what is printed on a keyboard key. The tooltip of the same key is
  capitalized (`Escape`).

The proper nouns are listed in `proper_nouns` (GitHub, Git, HEAD, TOTP,
UTF-8…). So `Branch & Git` (`wb-settings.js:189`) is correct, because Git is a
proper noun.

### 3. The product name is Ralphy

The name is a proper noun, with a capital R in every position.

| Before | After | Where |
|---|---|---|
| `About ralphy` | `About Ralphy` | `index.html:2150` |
| `ralphy · detached fence` | `Ralphy · detached fence` | `detached-fence.html:10` |
| `Name the consoles ralphy opens` | `Name the consoles Ralphy opens` | `wb-settings.js:323` |

A path (`<repo>/.ralphy/settings.json`) or a command the operator types
(`ralphy update`) is code, not the name, and stays as the code is written.
The lint reports these, and each one gets an exemption with its reason (#425).

### 4. One verb per act

| Verb | Means | Example |
|---|---|---|
| **Delete** | The thing leaves the disk and cannot be restored. | `Delete permanently` (`wb-changes.js:285`) |
| **Remove** | The thing leaves a list. Nothing on the disk changes. | `Remove project` … `Files on disk are kept.` (`app.js:719-720`) |
| **Discard** | Unsaved or uncommitted work is thrown away. | `Discard changes` (`wb-changes.js:291`) |
| **Refresh** | A list or a panel is read again from the daemon. | `Refresh projects` (`index.html:168`) |
| **Reload** | An open file is read again from the disk, replacing the copy on screen. | `changed on disk — reload` (`wb-viewer.js:408`) |
| **Close** | A window, panel, tab, dialog or card goes away. | `Close mirror` (`app.js:4796`) |
| **Dismiss** | A notice, toast, feed or error line goes away, and nothing else happens. | `dismiss the output feed` (`index.html:1658`) |
| **Cancel** | The negative button of a dialog, and nothing else. | `Cancel` (`wb-console.js:161`) |
| **Open** | Start to view a thing that exists on its own. | `Open the plan for the next run` (`index.html:1346`) |
| **Show / Hide** | Toggle a part that is already on screen. | `Show the unpriced rows` (`index.html:1174`) |

**Discard** is the verb of CONTEXT.md's **Working-tree operations**. This
ADR extends it to other unsaved work: a plan (`app.js:2022` "Delete this
plan" becomes "Discard this plan") and unsaved edits in a tab
(`wb-viewer.js:743`).

A **worktree** is deleted, not removed, because its directory leaves the
disk: `Cannot remove worktree {w.name}` (`app.js:1357`) and `remove this
worktree` (`wb-console.js:1180`) take **Delete**. The CLI command is still
`ralphy worktree remove`. This rule is about what the workbench says, and the
command name is code.

The lint cannot check the meaning of a verb. The editorial passes check it by
hand against this table.

### 5. A tooltip is a short phrase with no final period

A tooltip says what the control does. When the control is an action, the
tooltip starts with the verb. It has no final period. When a tooltip needs a
second sentence, each sentence ends in a period, but a shorter tooltip is
better.

| Before | After | Where |
|---|---|---|
| `draw a fence, or jump to one` | `Draw a fence, or jump to one` | `index.html:765` |
| `uncommitted changes` | `Uncommitted changes` | `wb-console.js:1170` (a state, so no verb) |

Today 127 of 128 tooltips have no final period, so this rule mostly records
what is already true.

### 6. A failure names the act and the cause, and the remedy when there is one

In a panel or a dialog, a failure reads `Could not <act>: <cause>.` When
there is something the operator can do, or something the workbench did
instead, a second sentence says so. "Unable to" is not used. "Cannot" is kept
for an act that is not allowed, never for an act that failed.

| Before | After | Where |
|---|---|---|
| `could not load projects from the daemon` | `Could not load projects: the daemon did not answer.` | `app.js:406` |
| `Cannot save as {p.encoding}` | `Could not save as {p.encoding}: …` | `app.js:5337` |
| `Repo unreachable. Cannot switch branch.` | `Could not switch branch: the project is unreachable.` | `wb-project.js:65` |
| `Could not refresh the file list ({reason}). Showing the last known list.` | `Could not refresh the file list: {reason}. Showing the last known list.` | `app.js:3668` |

A terminal notice reads `[<act> refused — <cause>]` or
`[<act> failed — <cause>]`: lowercase, in brackets, with an em dash.
`[paste refused: daemon not connected]` (`wb-console.js:4406`) becomes
`[paste refused — daemon not connected]`, like its three neighbours.

Text that the daemon writes and the UI shows as it is (verb errors, gate
refusals) is out of scope. It gets its own follow-up issue from #431.

### 7. Punctuation

- **Ellipsis.** Always `…`, never `...`. Today the inventory has 25 of the
  first and none of the second. The JSON bans `...`.
- **A label that opens a dialog or asks for more input ends in `…`**:
  `Rename…`, `Move to…`, `New file…` (`app.js:4753-4769`). A label that acts
  at once does not: `Delete`.
- **A colon introduces a cause or a detail**: `Could not read runs: {reason}.`
- **An em dash** is used in terminal notices (§6). It does not join two
  ideas: two ideas are two sentences (§10). It is not the separator between
  an error and its cause.
- **A middle dot `·`** separates the parts of a title or a chip:
  `Ralphy · detached file`, `Plan · #{issue}`.
- **Curly quotes `“…”`** surround a name the operator chose:
  `Delete “{name}”?` (`wb-changes.js:282`).

### 8. Words from the glossary

The ban list in the JSON is curated by hand. It is not derived from the
`_Avoid_` lines of CONTEXT.md: those lines forbid words such as `stage`,
`sync` and `diff` in one sense only, and a derived list would flag correct
text all over the Changes panel.

- **Stage, not plane.** CONTEXT.md calls the surface that console windows,
  fences and notes live on the **Stage**. `fences on the plane`
  (`index.html:781`) becomes `Fences on the stage`, and `write a note on the
  plane` (`index.html:807`) becomes `Write a note on the stage`. `plane` is
  banned.
- **Backlog, in one sense.** The Kanban column `Backlog` (`wb-kanban.js:79`)
  holds open issues that carry no queue label. It is the opposite of the
  **Queue label**, not a name for it, so it stays. `Backlog` never names
  `ready-for-agent`.
- **Project, not repo, for the thing in the sidebar.** The sidebar lists
  projects. `repo` is used only when the text is about the git repository
  itself: `the repo root` (`app.js:5081`), or a path such as `<repo>/…`.
  `Repo unreachable` (`wb-project.js:65`) and `home (no repo selected)`
  (`index.html:701`) take `project`.
- **Worktree** is the operator's word (CONTEXT.md → **Checkout**). The UI
  keeps it. `checkout` is the name of the code family, not of the thing on
  screen.
- **Spend and cost.** **Spend** names the view and what a project spent.
  `cost` is the measure of one item: `cost per delivery`, `total cost`
  (`wb-spend.js:90`). Both stay.

### 9. What the lint reads, and what it must see

The lint (#425) checks the casing rule with its exemptions, the ban list, the
banned punctuation, the sentence length limit and the plain-word list. It reports concatenated sentences (83 at `53be8d22`)
as a separate, informational class. The editorial passes turn each one into a
small template function in the shape of `verbTitle` and `rowActTitle`.

A lint that cannot see a text reports zero violations for it, and that zero
is wrong. So the extraction that #423 built grows in #425 to cover the three
gaps the inventory found:

- a `const` whose value is prose (a string with a space): `wb-agents.js:8-9`;
- a function that returns copy but whose name has no copy suffix, listed by
  name in `copy_helpers`: `note()` in `wb-file-search.js:49`;
- an `innerHTML` string that holds several buttons, split into one text per
  element instead of one merged row: `wb-viewer.js:408`, `823`, `906`.

An exemption from a rule is allowed only with a written reason. #425 decides
where exemptions are recorded.

### 10. Plain English, for readers whose first language is not English

Many operators read English as a second language. The workbench text is
written for them. This extends the repo's "Plain English, no idioms" rule
([CLAUDE.md](../../CLAUDE.md)) to every text the workbench shows:

- **Common words, literal meaning.** No idiom, metaphor or figure of speech.
  A word means what a dictionary says it means.
- **One idea per sentence.** Two ideas are two short sentences, not one
  sentence joined by a dash or a semicolon.
- **Active voice, and say who acts** when it is not the operator: "The daemon
  did not answer", not "No answer was received".
- **No abbreviation** in a sentence: "for example", not "e.g.". Key names on
  the key bar are not sentences (§2).
- **Git words only as the name of the act.** `Stage`, `Commit`, `Push` are
  the names of buttons and stay. A git state is described in plain words:
  "uncommitted changes", not "a dirty tree".
- **A glossary term that is a metaphor gets a literal tooltip.** `Retry burn`
  (`wb-spend.js:116`) is a CONTEXT.md term and stays, but its tooltip says
  what it measures in literal words.

| Before | After | Where |
|---|---|---|
| `this card's file is gone — aim the card at another one` | `The file for this card is missing. Choose another file.` | `wb-notes.js:712` |
| `this note is hidden — the eye in the head shows it` | `This note is hidden. Click the eye icon to show it.` | `wb-notes.js:1904` |
| `Uncommitted changes here — a run refuses a dirty tree and a checkout may fail against them.` | `There are uncommitted changes here. A run does not start while they exist, and a branch switch can fail.` | `index.html:2257` |
| `e.g. @me or a github login` | `For example, @me or a GitHub login` | `wb-settings.js:174` |

The JSON carries the checkable part: `sentence_max_words` (20; the longest
sentence today has 26 words, at `wb-viewer.js:893`) and the `plain` list of
curated words and phrases with their literal replacement. The lint reports
both. It cannot judge whether a sentence is clear, so the editorial passes
read every text in their area against this section.

### 11. What is never rewritten

Terminal and agent output, git and gh messages, and user content (issue
titles, note text, file names) are not copy. The daemon's own messages are
out of scope for this ADR (§6).

## Rejected alternatives

- **A translation catalog with keys.** Rejected by PRD #422: it adds
  indirection with no second caller, and a catalog built to remove duplicate
  English strings conflicts with the one-key-per-context shape that a real
  translation needs.
- **A ban list derived from CONTEXT.md `_Avoid_`.** Rejected in §8: the
  `_Avoid_` lines are about one sense of a word, and a list derived from them
  would flag correct text.
- **Lowercase tooltips as a second casing rule.** Rejected. It matches most
  tooltips today, but it gives the lint and the writer two rules where one is
  enough, and it leaves `close` next to `Refresh projects` in the same bar.
- **Title Case for labels.** Rejected. Only 18 texts use it today, and it
  needs a list of small words that stay lowercase.
- **An amendment to ADR-0035.** Rejected. That ADR is about the visual
  language, and a lint that cites "the visual language ADR" for a casing rule
  is harder to follow.
- **The rules as a code block inside this ADR.** Rejected. The lint would have
  to parse markdown to find them.

## Consequences

- The editorial passes (#426–#430) have one answer for every casing, verb and
  punctuation question. What this ADR does not cover is decided by hand in the
  pass and then added here, so the next pass has the same answer.
- Almost no Rust assertion pins UI copy (14 of 662 texts), so the passes
  rarely change a Rust test. The `pinned` column of the inventory names the
  ones that do.
- A new text written after this ADR is checked by the lint when #431 turns
  enforcement on. Until then, the lint only reports.

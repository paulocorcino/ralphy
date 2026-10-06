# The workbench demo (archived)

The workbench had a static demo: open
`crates/ralphy-daemon/assets/ui/index.html` from disk, with no daemon, and the
page showed sample data. It left the source tree when the workbench script
moved to ES modules, because a browser does not load an ES module from a
`file://` page ([ADR-0075](../adr/0075-the-workbench-script-is-written-in-typescript.md)
D8). This note keeps what is needed to run it or to bring it back.

## How to run it

The annotated tag `workbench-demo-archive` points at the last commit where
the demo runs (`c33e806d`, 2026-10-06).

```sh
git fetch origin tag workbench-demo-archive
git worktree add ../ralphy-demo workbench-demo-archive
```

Then open `../ralphy-demo/crates/ralphy-daemon/assets/ui/index.html` in a
browser. Checked on 2026-10-06 with Chromium (Playwright): the page boots with
the four sample projects and no page error.

## What it showed

The demo read five seed files from `crates/ralphy-daemon/assets/ui-demo/`,
outside the tree that the daemon embeds:

| Seed file | What it filled |
|---|---|
| `wb-seed-projects.js` | four projects in the sidebar |
| `wb-seed-runs.js` | runs with their plans, and the ⚡ button that moved a run one step |
| `wb-seed-kanban.js` | the board |
| `wb-seed-files.js` | the Files tree, and the text of a file or a diff (`fakeContent`) |
| `wb-seed-agents.js` | the agent roster of the console menu |

It did not show consoles, hosts, devices or notes: they need the daemon. The
seeds were last changed on 2026-09-07 (`989182d7`), so the demo already
showed less than the workbench of that day.

## How it worked

One file, `wb-mode.js`, set `window.WBMode`. A page under `file:` was the
demo; a page under `http:` or `https:` was served by the daemon.

- `isDemo()` and `seedAllowed()` were true only in the demo. The seeds loaded
  only then, through an inline script in `index.html`.
- `isDaemon()` was the opposite. About 40 failure paths in `app.js`,
  `wb-console.js`, `wb-viewer.js` and `wb-release-dialogs.js` showed an error
  only when it was true; in the demo they kept the seed.
- `pageUrl()` opened a torn-off window at its file name in the demo, and at
  its route (`popup`, `fence`) under the daemon.
- A `file:` page has no origin, so the demo sent `postMessage` to `"*"`
  between the main page and a torn-off window.
- The login form checked the code and the password in the browser when the
  daemon did not answer.
- `applyRunEvent` and `demoTick` in `app.js` moved a seed run forward in the
  browser.

The archive pull request removed all of these. The last three were weaker than
the daemon path, and they existed only for the demo.

## Ways to bring a demo back

None of these is decided. Each one starts from the same fact: the demo mode
must come from an explicit flag, not from the protocol of the page.

| Way | How | Cost |
|---|---|---|
| Hosted demo | CI publishes the built tree with the seeds to GitHub Pages over https, where ES modules load | A workflow for Pages, and seeds kept up to date |
| `ralphy demo` | The binary serves the same tree with the seeds on loopback, with no store | A small subcommand, and the same seeds |
| The real daemon with sample data | `ralphy demo` starts a daemon with a scratch store and sample repositories. No seed is needed, and consoles work | Runs and the board need sample forge data, because they read GitHub today |
| One file for `file://` | A bundler joins the modules into one classic script in a `demo.html` | A bundler in the build, which ADR-0075 avoids |

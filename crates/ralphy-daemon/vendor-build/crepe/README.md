# The vendored Crepe bundle

The note card's editor (ADR-0064 §6): Milkdown **Crepe**, built lean into one
JS file and one CSS file under `../../assets/ui/vendor/crepe/`.

```sh
npm ci && node build.mjs
```

That is the whole procedure, and it is run **by hand** when the pin in
`package.json` moves. It is not wired into `cargo build` and not into CI: the
artefacts are committed, exactly as every other vendored asset is
(ADR-0057) — the difference is only that this one is *built* rather than
copied, which is why the recipe exists at all.

## Why the recipe lives here and not beside the artefact

`src/lib.rs` embeds `assets/ui/` wholesale with `include_dir!` and the router
serves every path under it. A `build/` directory next to `crepe.js` would put
`package.json`, `node_modules/` and this README inside the binary and on the
wire. There is no exclusion mechanism, so the recipe lives outside the embedded
tree — beside `ui-tests/`, which is not embedded for the same reason.

## What is in the bundle, and what is not

`entry.js` names the seven features and says why each is there. The four left
out are CodeMirror (Monaco is this workbench's one editor engine, #308), LaTeX,
image-block and the top bar; `entry.css` mirrors that list partial for partial,
and deliberately does not import Crepe's own light palette — the card sets every
`--crepe-*` token from ADR-0035's in `styles/13-notes.css`.

The first line of each artefact is a provenance header naming the Crepe version,
the esbuild version and the feature list. A Rust test (`lib.rs`,
`vendored_crepe_states_its_recipe`) pins it against these files, so an artefact
built by something else — or a recipe changed without a rebuild — reds.

Measured at 7.22.1: **crepe.js 706 KB, crepe.css 20 KB** (≈237 KB gzipped
together). The build prints both sizes; a bump that doubles them is a decision,
not a detail.

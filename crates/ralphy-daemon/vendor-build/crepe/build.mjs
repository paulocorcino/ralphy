// Build the vendored Crepe artefacts (ADR-0064 §6, ADR-0057).
//
// `npm ci && node build.mjs`, by hand, when the pin moves. This NEVER runs in
// CI and is not on any Rust build path: `assets/ui/` is embedded with
// `include_dir!`, so the artefact is committed and the recipe lives OUTSIDE
// that tree (this directory) — otherwise the daemon would serve `node_modules`
// and a `package.json` to the browser.
//
// The first line of `crepe.js` is a provenance header a Rust test pins
// (`vendored_crepe_states_its_recipe`): a bundle whose header does not match
// this script's constants was built by something else.
import { build } from 'esbuild';
import { readFileSync, copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '../../assets/ui/vendor/crepe');

const pkg = JSON.parse(readFileSync(join(HERE, 'package.json'), 'utf8'));
const CREPE = pkg.devDependencies['@milkdown/crepe'];
const ESBUILD = pkg.devDependencies.esbuild;
const FEATURES = [
  'block-edit',
  'cursor',
  'link-tooltip',
  'list-item',
  'placeholder',
  'table',
  'toolbar',
  // Not a Crepe feature: OUR node view, which draws a ```mermaid fence
  // (ADR-0064 §15). It is in the header because it is in the bundle.
  'mermaid-view',
].join(',');
const HEADER = `/* crepe ${CREPE} · esbuild ${ESBUILD} · features: ${FEATURES} */`;

mkdirSync(OUT, { recursive: true });

await build({
  entryPoints: [join(HERE, 'entry.js')],
  outfile: join(OUT, 'crepe.js'),
  bundle: true,
  minify: true,
  format: 'iife',
  target: ['es2022'],
  banner: { js: HEADER },
  // The workbench is served with a CSP that has no `'unsafe-eval'`; a bundle
  // carrying a sourcemap comment would 404 in devtools and nothing else.
  sourcemap: false,
  legalComments: 'none',
  loader: { '.woff': 'dataurl', '.woff2': 'dataurl', '.ttf': 'dataurl', '.svg': 'dataurl' },
});

await build({
  entryPoints: [join(HERE, 'entry.css')],
  outfile: join(OUT, 'crepe.css'),
  bundle: true,
  minify: true,
  banner: { css: HEADER },
  loader: { '.woff': 'dataurl', '.woff2': 'dataurl', '.ttf': 'dataurl', '.svg': 'dataurl' },
});

copyFileSync(join(HERE, 'node_modules/@milkdown/crepe/LICENSE'), join(OUT, 'LICENSE'));

// The sizes are the ADR's numbers: printed so a bump that doubles the bundle
// is noticed at the moment it is made, not at the next page load.
const size = (name) => (readFileSync(join(OUT, name)).length / 1024).toFixed(0);
console.log(`${HEADER}\ncrepe.js ${size('crepe.js')} KB · crepe.css ${size('crepe.css')} KB`);

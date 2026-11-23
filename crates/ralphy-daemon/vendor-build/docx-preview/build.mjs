// Build the vendored docx-preview module (ADR-0091 D8, amendment 2026-11-23).
//
// `npm ci && node build.mjs`, by hand, when a pin moves. Never in CI. The
// recipe lives outside `assets/ui/` for the reason in ../crepe/README.md.
//
// One ES module holds docx-preview and jszip: docx-preview imports jszip by
// its bare name, which the page cannot resolve, and the UMD builds would
// register on the Monaco loader's AMD `define`. The first line names the
// versions; `vendored_docx_preview_states_its_recipe` checks it.
import { build } from 'esbuild';
import { readFileSync, copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '../../assets/ui/vendor/docx-preview');

const pins = JSON.parse(readFileSync(join(HERE, 'package.json'), 'utf8')).devDependencies;
const HEADER =
  `/* docx-preview ${pins['docx-preview']} · jszip ${pins.jszip} · esbuild ${pins.esbuild} */`;

mkdirSync(OUT, { recursive: true });

await build({
  entryPoints: [join(HERE, 'entry.js')],
  outfile: join(OUT, 'docx-preview.js'),
  bundle: true,
  minify: true,
  format: 'esm',
  platform: 'browser',
  target: ['es2022'],
  banner: { js: HEADER },
  sourcemap: false,
  legalComments: 'none',
});

copyFileSync(join(HERE, 'node_modules/docx-preview/LICENSE'), join(OUT, 'LICENSE'));
copyFileSync(join(HERE, 'node_modules/jszip/LICENSE.markdown'), join(OUT, 'LICENSE-jszip'));

const size = (name) => (readFileSync(join(OUT, name)).length / 1024).toFixed(0);
console.log(`${HEADER}\ndocx-preview.js ${size('docx-preview.js')} KB`);

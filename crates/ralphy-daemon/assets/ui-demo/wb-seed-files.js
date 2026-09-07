// Demo seed: file contents — a file's "bytes" synthesised from its name, so
// every viewer feature is demonstrable with no backend.
//
// Lives OUTSIDE `assets/ui` on purpose: `lib.rs` embeds that directory whole via
// `include_dir!`. Loaded only by the `file://` demo, so a daemon neither ships
// nor serves any of it (#300, ADR-0036).
//
// `fakeContent` is a global function declaration, exactly as it was in
// wb-viewer.js — the call sites in app.js and wb-viewer.js are unchanged. Off
// `file://` the name is now UNDEFINED rather than merely unreachable, which is
// the #202 posture sharpened: a daemon-mode path that somehow reaches for
// synthetic bytes throws where it used to quietly succeed.

/* ---------------------------------------------------------------------------
   Seed file contents — used only in the static demo with no backend, so a
   file's "bytes" are synthesised from its name so every viewer feature is
   demonstrable. A daemon-backed build fetches the actual file instead.
--------------------------------------------------------------------------- */
function fakeContent(path, ftype) {
  const base = path.split("/").pop();
  const e = base.toLowerCase().includes(".") ? base.toLowerCase().split(".").pop() : "";
  if (ftype === "markdown") return fakeMarkdown(base);
  if (ftype === "image") return fakeImage(base);
  const gen = {
    ts: fakeTs, tsx: fakeTsx, js: fakeTs, mjs: fakeTs,
    rs: fakeRs, json: fakeJson, css: fakeCss, toml: fakeToml,
    prisma: fakePrisma, py: fakePy,
  }[e];
  return gen ? gen(base) : `// ${path}\n// (demo) source for ${base}\n\nexport const answer = 42;\n`;
}

// The image pane's demo bytes: a placeholder SVG naming the file, so the pane is
// demonstrable with no daemon to read the real one. Returned as a `data:` URL
// because that is exactly what the pane consumes in daemon mode.
function fakeImage(name) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="300">
  <rect width="480" height="300" fill="#14110f"/>
  <rect x="8" y="8" width="464" height="284" fill="none" stroke="#e8d9a8" stroke-dasharray="6 6"/>
  <text x="240" y="140" fill="#e8d9a8" font-family="ui-monospace, monospace" font-size="18" text-anchor="middle">${name}</text>
  <text x="240" y="172" fill="#8a8175" font-family="ui-monospace, monospace" font-size="13" text-anchor="middle">(demo) no daemon — placeholder image</text>
</svg>`;
  return "data:image/svg+xml;base64," + btoa(svg);
}

function fakeMarkdown(name) {
  const title = name.replace(/\.(md|markdown)$/i, "");
  return `# ${title}

A rendered Markdown tab — the outline on the left jumps between headings, the
toolbar's **Find** searches this page, and **Edit** flips to the raw source.

## Architecture

The workbench is an intent surface: gestures become events; a backend does the
real work.

\`\`\`mermaid
flowchart LR
  U[User gesture] --> UI[Workbench UI]
  UI -- workbench:action --> BE[Backend engine]
  BE --> FS[(Filesystem)]
  BE --> GH[(GitHub)]
\`\`\`

## Usage

1. Open a project in the sidebar.
2. Double-click a file to open it here.
3. Right-click for rename / copy path / delete.

### Notes

- Images open in their own pane; other binaries refuse to open.
- Markdown always opens **rendered**, with mermaid support.
- Source files open with syntax highlighting.

## A table

| Kind     | Viewer        | Editable |
| -------- | ------------- | -------- |
| \`.md\`    | rendered      | yes      |
| \`.rs\`    | Monaco        | yes      |
| \`.png\`   | image pane    | no       |
| \`.pdf\`   | (refused)     | no       |

## Code sample

\`\`\`ts
export function greet(name: string) {
  return \`hello, \${name}\`;
}
\`\`\`

> Editing here emits a \`save\` intent — the demo never writes to disk.
`;
}

function fakeTs(name) {
  return `// ${name}
import { useEffect, useState } from "react";

export interface Session {
  id: number;
  repo: string;
  agent: "claude" | "codex" | "opencode";
}

export function useSessions(): Session[] {
  const [sessions, setSessions] = useState<Session[]>([]);
  useEffect(() => {
    fetch("/api/sessions")
      .then((r) => r.json())
      .then(setSessions)
      .catch(() => setSessions([]));
  }, []);
  return sessions;
}
`;
}

function fakeTsx(name) {
  return `// ${name}
import { useSessions } from "../lib/sessions";

export default function Sidebar() {
  const sessions = useSessions();
  return (
    <aside className="side">
      <h2>Sessions</h2>
      <ul>
        {sessions.map((s) => (
          <li key={s.id}>
            {s.repo} · {s.agent}
          </li>
        ))}
      </ul>
    </aside>
  );
}
`;
}

function fakeRs(name) {
  return `// ${name}
use std::collections::HashMap;

/// A registered repository the daemon can launch agents into.
#[derive(Debug, Clone)]
pub struct Repo {
    pub slug: String,
    pub path: std::path::PathBuf,
    pub reachable: bool,
}

impl Repo {
    pub fn new(slug: impl Into<String>, path: impl Into<std::path::PathBuf>) -> Self {
        Self { slug: slug.into(), path: path.into(), reachable: true }
    }
}

pub fn index(repos: &[Repo]) -> HashMap<&str, &Repo> {
    repos.iter().map(|r| (r.slug.as_str(), r)).collect()
}
`;
}

function fakeJson(name) {
  return `{
  "name": "${name.replace(/\.json$/, "")}",
  "version": "0.1.0",
  "private": true,
  "scripts": {
    "dev": "next dev",
    "build": "next build",
    "test": "vitest"
  },
  "dependencies": {
    "next": "15.0.0",
    "react": "19.0.0"
  }
}
`;
}

function fakeCss(name) {
  return `/* ${name} */
:root {
  --bg: #14110f;
  --text: #e8e2d9;
  --accent: #e8d9a8;
}
body {
  background: var(--bg);
  color: var(--text);
  font-family: ui-monospace, monospace;
}
.btn {
  border: 1px solid var(--accent);
  border-radius: 4px;
  padding: 0.3rem 0.7rem;
}
`;
}

function fakeToml(name) {
  return `# ${name}
[package]
name = "ralphy"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
`;
}

function fakePrisma(name) {
  return `// ${name}
datasource db {
  provider = "postgresql"
  url      = env("DATABASE_URL")
}

model User {
  id    Int    @id @default(autoincrement())
  email String @unique
  name  String?
}
`;
}

function fakePy(name) {
  return `# ${name}
from dataclasses import dataclass


@dataclass
class Repo:
    slug: str
    path: str
    reachable: bool = True


def index(repos: list[Repo]) -> dict[str, Repo]:
    return {r.slug: r for r in repos}
`;
}

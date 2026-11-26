#!/usr/bin/env python3
"""Give every test file of the repo its kind, and every pin test its marker state.

Modes (run from the repo root, Python 3 stdlib only):
  --write                 write docs/TESTING-KINDS.md
  --check                 fail when docs/TESTING-KINDS.md differs from what --write writes
  --selftest              classify the built-in snippets and compare with their expected kind
  --pins <path>...        list the pin tests of the files (a path is a file, a folder or a glob)
  --unmarked-pins <path>... [--exclude <folder>]...
                          list the pins with no `// pin:` line above them; the last line
                          is `unmarked pins: <n>`

The kinds, in the order they are decided: ratchet (under crates/xtask/tests/ratchets/),
browser (under tests/browser/), pin (a test that reads source text), integration (under
crates/*/tests/), behavior (the rest). A file's kind is the kind of most of its tests.
"""
from __future__ import annotations

import glob
import re
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOC = ROOT / "docs" / "TESTING-KINDS.md"
KINDS = ("behavior", "integration", "pin", "ratchet", "browser")
MARKER = "// pin:"

# The baseline runs, written by hand after each measure (the script cannot run them).
MEASURED = """\
Date: 2026-11-26. Machine: Windows 11 Pro (10.0.26300), the phase tree of
`test/961-measure` at `deb106c5`, a warm `target/` (no compile in the runs),
nothing else building. Wall time is the time of the whole command, by Python's
`time.monotonic()` before and after it.

| Suite | Command | Tests | Wall time |
| --- | --- | --- | --- |
| Rust (nextest), run 1 | `cargo nextest run --locked --workspace --exclude xtask --no-fail-fast` | 3777 run, 14 skipped | 115.1 s (nextest: 112.0 s) |
| Rust (nextest), run 2 | the same command | 3777 run, 14 skipped | 115.5 s (nextest: 112.3 s) |
| xtask | `cargo nextest run --locked -p xtask` | 305 run | 4.0 s (nextest: 3.0 s) |
| ui-tests | `node --test crates/ralphy-daemon/ui-tests` | 1755 | 91.4 s (node: 91.3 s) |
"""

# ---------------------------------------------------------------- lexing


def strip_code(text: str, js: bool) -> str:
    """`text` with comments and string contents blanked, so braces count right."""
    out, i, n = [], 0, len(text)
    while i < n:
        c, nxt = text[i], text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            out.append(" " * (j - i))
            i = j
        elif c == "/" and nxt == "*":
            j = text.find("*/", i + 2)
            j = n if j < 0 else j + 2
            out.append(re.sub(r"[^\n]", " ", text[i:j]))
            i = j
        elif not js and c == "r" and re.match(r'r#*"', text[i:]) and not _ident(text, i - 1):
            hashes = re.match(r"r(#*)\"", text[i:]).group(1)
            end = text.find('"' + hashes, i + len(hashes) + 2)
            j = n if end < 0 else end + 1 + len(hashes)
            out.append(_blank(text[i:j]))
            i = j
        elif js and c == "/" and _regex_may_start(out):
            j = _regex_end(text, i)
            out.append(_blank(text[i:j]))
            i = j
        elif c == '"' or (js and c in "'`"):
            j = _string_end(text, i, c)
            out.append(_blank(text[i:j]))
            i = j
        elif not js and c == "'" and re.match(r"'(\\.|[^\\'])'", text[i:]):
            j = i + len(re.match(r"'(\\.|[^\\'])'", text[i:]).group(0))
            out.append(_blank(text[i:j]))
            i = j
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _ident(text: str, i: int) -> bool:
    return i >= 0 and (text[i].isalnum() or text[i] == "_")


def _blank(s: str) -> str:
    return s[0] + re.sub(r"[^\n]", " ", s[1:-1]) + s[-1] if len(s) >= 2 else s


def _string_end(text: str, i: int, quote: str) -> int:
    """A `'` or `"` string ends at its quote or at the end of the line; a template may span lines."""
    j = i + 1
    while j < len(text) and text[j] != quote and (quote == "`" or text[j] != "\n"):
        j += 2 if text[j] == "\\" else 1
    return min(j + 1, len(text)) if j < len(text) and text[j] == quote else j


def _regex_may_start(out: list) -> bool:
    """A `/` starts a JS regex literal after an operator, an open bracket or a keyword."""
    before = "".join(out[-40:]).rstrip()
    if not before or before[-1] in "(,=:[!&|?{};+-*%<>~^":
        return True
    return re.search(r"\b(return|typeof|of|in)$", before) is not None


def _regex_end(text: str, i: int) -> int:
    j, in_class = i + 1, False
    while j < len(text) and text[j] != "\n":
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == "/" and not in_class:
            return j + 1
        in_class = (in_class or text[j] == "[") and text[j] != "]"
        j += 1
    return i + 1


def matching(code: str, start: int, open_ch: str, close_ch: str) -> int:
    """The index after the bracket that closes the first `open_ch` at or after `start`."""
    i = code.find(open_ch, start)
    if i < 0:
        return len(code)
    depth = 0
    for j in range(i, len(code)):
        if code[j] == open_ch:
            depth += 1
        elif code[j] == close_ch:
            depth -= 1
            if depth == 0:
                return j + 1
    return len(code)


# ---------------------------------------------------------------- source reads

RUST_INCLUDE = re.compile(r"include_str!\s*\(")
RUST_UI_DIR = re.compile(r"\bUI\s*\.\s*get_(file|dir)\b")
# An included file with one of these extensions, or under a fixtures folder, is test data.
DATA_EXT = re.compile(r"\.(json|jsonl|err|log|txt|webmanifest|sql|bin|zip|gz|csv|snap)\"")
RUST_FS_READ = re.compile(r"\b(read_to_string|fs::read)\s*\(")
SOURCE_PATH = re.compile(r'"[^"\n]*\b(src|assets)\b[^"\n]*"')


def includes_source(raw: str) -> bool:
    for m in RUST_INCLUDE.finditer(raw):
        args = raw[m.end(): raw.find(")", m.end()) + 1]
        if "fixtures" not in args and not DATA_EXT.search(args):
            return True
    return False
JS_READ = re.compile(r"\breadFileSync\s*\(")
JS_HELPERS = ("partMarkup", "assembledPage")
JS_FN_TEXT = re.compile(r"\bString\(\s*([A-Za-z_$][\w$]*)\s*\)|\b([A-Za-z_$][\w$]*)\.toString\(\)")


def reads_source(raw: str, js: bool, functions: set[str]) -> bool:
    """True when the code `raw` reads source text itself."""
    if js:
        for m in JS_READ.finditer(raw):
            line = raw[max(0, m.start() - 40): raw.find("\n", m.start())]
            if not any(w in line for w in ("fixtures", "JSON.parse", ".json")):
                return True
        if any(re.search(rf"\b{h}\s*\(", raw) for h in JS_HELPERS):
            return True
        return any((m.group(1) or m.group(2)) in functions for m in JS_FN_TEXT.finditer(raw))
    if includes_source(raw) or RUST_UI_DIR.search(raw):
        return True
    return bool(RUST_FS_READ.search(raw)) and bool(SOURCE_PATH.search(raw))


# ---------------------------------------------------------------- tests in a file


@dataclass
class Test:
    name: str
    line: int
    kind: str
    marked: bool


@dataclass
class TestFile:
    path: str
    tests: list[Test] = field(default_factory=list)

    @property
    def kind(self) -> str:
        counts = Counter(t.kind for t in self.tests)
        return max(KINDS, key=lambda k: (counts[k], -KINDS.index(k)))


RUST_TEST = re.compile(r"#\[(?:[\w:]+::)?test\b[^\]]*\]")
RUST_FN = re.compile(r"\bfn\s+(\w+)")
RUST_DEF = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(fn|const|static)\s+(\w+)", re.M)
JS_TEST = re.compile(r"^\s*(?:test|it)\s*\(\s*(['\"`])(.*?)\1", re.M)
JS_DEF = re.compile(
    r"^(?:export\s+)?(?:async\s+)?(?:function\s*\*?\s*([\w$]+)|(?:const|let)\s+([\w$]+)\s*=)", re.M
)


def test_region(text: str, rel: str) -> int:
    """Where the tests of a `src` file start: the `#[cfg(test)]` line followed by `mod`."""
    if "/tests/" in rel or rel.endswith("/tests.rs") or "/src/" not in rel:
        return 0
    m = re.search(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s", text)
    return m.start() if m else len(text)


def rust_defs(raw: str, code: str) -> dict[str, str]:
    defs = {}
    for m in RUST_DEF.finditer(code):
        if RUST_TEST.search(re.split(r"[;{}]", code[max(0, m.start() - 300): m.start()])[-1]):
            continue  # a test is never called, so it is no helper
        end = matching(code, m.end(), "{", "}") if m.group(1) == "fn" else code.find(";", m.end())
        defs[m.group(2)] = raw[m.start(): end + 1]
    return defs


def js_defs(raw: str, code: str) -> dict[str, str]:
    defs = {}
    for m in JS_DEF.finditer(code):
        end = code.find("\n}", m.end()) if m.group(1) else code.find(";\n", m.end())
        end = len(code) if end < 0 else end + 2
        defs[m.group(1) or m.group(2)] = raw[m.start(): end]
    return defs


def source_names(defs: dict[str, str], js: bool) -> set[str]:
    """The names whose definition reads source text, directly or through another name."""
    functions = {n for n, body in defs.items() if re.match(r"\s*(export\s+)?(async\s+)?function", body)}
    names = {n for n, body in defs.items() if reads_source(body, js, functions)}
    if js:
        return names  # a JS helper that reads source to run it (loadShell) is no pin
    words = {n: idents(body) for n, body in defs.items()}
    grew = True
    while grew:
        grew = False
        for n in defs:
            if n not in names and words[n] & (names - {n}):
                names.add(n)
                grew = True
    return names


def idents(code: str) -> set[str]:
    return set(re.findall(r"[A-Za-z_$][\w$]*", code))


def marked(lines: list[str], line: int) -> bool:
    """True when the comment or attribute block right above line `line` (1-based) has a marker."""
    i = line - 2
    while i >= 0:
        s = lines[i].strip()
        if s.startswith(MARKER):
            return True
        if not (s.startswith("//") or s.startswith("#[") or s.startswith("/*") or s.startswith("*")):
            return False
        i -= 1
    return False


def location_kind(rel: str) -> str | None:
    if rel.startswith("crates/xtask/tests/ratchets/"):
        return "ratchet"
    if rel.startswith("tests/browser/"):
        return "browser"
    return None


def classify(rel: str, text: str, helpers: set[str] | None = None) -> TestFile:
    """The tests of the file `rel` (a repo-relative path) whose content is `text`."""
    tf = TestFile(rel)
    fixed = location_kind(rel)
    if rel.endswith(".py"):
        tf.tests.append(Test(Path(rel).stem, 1, fixed or "browser", False))
        return tf
    js = rel.endswith((".mjs", ".js", ".ts"))
    code = strip_code(text, js)
    lines = text.splitlines()
    region = 0 if js else test_region(code, rel)
    defs = js_defs(text, code) if js else rust_defs(text[region:], code[region:])
    names = set(helpers or ()) | source_names(defs, js)
    functions = {n for n, b in defs.items() if re.match(r"\s*(export\s+)?(async\s+)?function", b)}
    other = "integration" if re.match(r"crates/[^/]+/tests/", rel) else "behavior"
    for name, start, body in (js_tests if js else rust_tests)(text, code, region):
        line = text.count("\n", 0, start) + 1
        pin = reads_source(body, js, functions) or bool(idents(body) & names)
        kind = fixed or ("pin" if pin else other)
        tf.tests.append(Test(name, line, kind, kind == "pin" and marked(lines, line)))
    return tf


def rust_tests(raw: str, code: str, region: int):
    for m in RUST_TEST.finditer(code, region):
        fn = RUST_FN.search(code, m.end())
        if not fn:
            continue
        end = matching(code, fn.end(), "{", "}")
        yield fn.group(1), m.start(), raw[fn.start(): end]


def js_tests(raw: str, code: str, _region: int):
    for m in JS_TEST.finditer(raw):
        if code[m.start(): m.end()].strip() == "":
            continue
        end = matching(code, m.start(), "(", ")")
        yield m.group(2), m.start() + len(m.group(0)) - len(m.group(0).lstrip()), raw[m.start(): end]


# ---------------------------------------------------------------- the repo


def rel_of(p: Path) -> str:
    return p.resolve().relative_to(ROOT).as_posix()


def test_files() -> list[Path]:
    found = set()
    for p in ROOT.glob("crates/*/src/**/*.rs"):
        if "#[test]" in p.read_text(encoding="utf-8", errors="replace") or re.search(
            r"#\[[\w:]+::test", p.read_text(encoding="utf-8", errors="replace")
        ):
            found.add(p)
    for p in ROOT.glob("crates/*/tests/**/*.rs"):
        if "/fixtures/" not in rel_of(p):
            found.add(p)
    found.update(ROOT.glob("crates/ralphy-daemon/ui-tests/*.test.mjs"))
    found.update(ROOT.glob("tests/browser/**/*.py"))
    return sorted(found, key=rel_of)


def crate_helpers() -> dict[str, set[str]]:
    """Per crate, the names in its test code that read source text (the daemon's `tests.rs`
    helpers are used by every file of `src/tests/`)."""
    out: dict[str, dict[str, str]] = {}
    for p in [*ROOT.glob("crates/*/src/**/*.rs"), *ROOT.glob("crates/*/tests/**/*.rs")]:
        rel = rel_of(p)
        if "/fixtures/" in rel:
            continue
        text = p.read_text(encoding="utf-8", errors="replace")
        code = strip_code(text, False)
        region = test_region(code, rel)
        out.setdefault(rel.split("/")[1], {}).update(rust_defs(text[region:], code[region:]))
    return {c: source_names(d, False) for c, d in out.items()}


def classify_all(paths: list[Path] | None = None) -> list[TestFile]:
    files = test_files()
    helpers = crate_helpers()
    chosen = files if paths is None else paths
    result = []
    for p in chosen:
        rel = rel_of(p)
        crate = rel.split("/")[1] if rel.startswith("crates/") and rel.endswith(".rs") else ""
        tf = classify(rel, p.read_text(encoding="utf-8", errors="replace"), helpers.get(crate))
        if tf.tests:
            result.append(tf)
    return result


def expand(args: list[str]) -> list[Path]:
    out = []
    for a in args:
        hits = [Path(h) for h in glob.glob(str(ROOT / a), recursive=True)] or [ROOT / a]
        for h in hits:
            if h.is_dir():
                out.extend(f for f in h.rglob("*") if f.suffix in (".rs", ".mjs") and f.is_file())
            elif h.is_file():
                out.append(h)
    return sorted(set(out), key=rel_of)


# ---------------------------------------------------------------- the document


def render(files: list[TestFile]) -> str:
    kinds = Counter()
    tests = Counter()
    for f in files:
        kinds[f.kind] += 1
        tests.update(t.kind for t in f.tests)
    out = [
        "# Test kinds",
        "",
        "Generated by `python docs/testing/test_kinds.py --write`; do not edit by hand.",
        "`--check` fails when this file is out of date. The kinds and the pin rule are in",
        "[TESTING.md](./TESTING.md).",
        "",
        "## Totals",
        "",
        "| Kind | Files | Tests |",
        "| --- | --- | --- |",
    ]
    out += [f"| {k} | {kinds[k]} | {tests[k]} |" for k in KINDS]
    out += [f"| all | {len(files)} | {sum(tests.values())} |", "", "## Files", ""]
    out += [
        "A file's kind is the kind of most of its tests. *Pins* counts its pin tests, and",
        "*Unmarked* the pins with no `// pin:` line above them.",
        "",
        "| File | Kind | Tests | Pins | Unmarked |",
        "| --- | --- | --- | --- | --- |",
    ]
    for f in files:
        pins = [t for t in f.tests if t.kind == "pin"]
        unmarked = sum(not t.marked for t in pins)
        out.append(f"| `{f.path}` | {f.kind} | {len(f.tests)} | {len(pins)} | {unmarked} |")
    out += ["", "## Measured", "", MEASURED.rstrip("\n"), ""]
    return "\n".join(out)


# ---------------------------------------------------------------- selftest

SNIPPETS = [
    ("crates/a/src/x.rs", "#[cfg(test)]\nmod tests {\n#[test]\nfn adds() { assert_eq!(1 + 1, 2); }\n}",
     "behavior"),
    ("crates/a/tests/x.rs", "#[test]\nfn runs() { run(); }", "integration"),
    ("crates/xtask/tests/ratchets/x.rs", '#[test]\nfn r() { include_str!("../src/a.rs"); }',
     "ratchet"),
    ("tests/browser/board/wb_x.py", "page.goto(url)\n", "browser"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n#[test]\nfn p() {\n'
     '    assert!(include_str!("../assets/ui/a.ts").contains("x"));\n}\n}', "pin"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n#[test]\nfn p() {\n    let t = std::fs::'
     'read_to_string(root.join("src/lib.rs")).unwrap();\n}\n}', "pin"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n#[test]\nfn p() {\n    let t = std::fs::'
     'read_to_string(dir.path().join("out.txt")).unwrap();\n}\n}', "behavior"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\nfn page() -> &\'static str '
     '{ include_str!("../assets/ui/a.html") }\nfn shell() -> String { page().to_string() }\n'
     '#[test]\nfn p() { assert!(shell().contains("{")); }\n}', "pin"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n#[test]\nfn d() {\n    parse(include_str!('
     '"../tests/fixtures/a.json"));\n}\n}', "behavior"),
    ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n#[test]\nfn d() {\n    parse(include_str!('
     '"data/reply.jsonl"));\n}\n}', "behavior"),
    ("crates/a/src/x.rs", 'fn prod() { include_str!("a.txt"); }\n#[cfg(test)]\nmod tests {\n'
     '#[test]\nfn b() { prod(); }\n}', "behavior"),
    ("crates/a/ui-tests/x.test.mjs", 'test("p", () => {\n  const t = readFileSync(join(UI, "a.ts"),'
     ' "utf8");\n});', "pin"),
    ("crates/a/ui-tests/x.test.mjs", 'test("f", () => {\n  const t = JSON.parse(readFileSync(join('
     'HERE, "fixtures", "a.json"), "utf8"));\n});', "behavior"),
    ("crates/a/ui-tests/x.test.mjs", 'test("p", () => {\n  assert.match(partMarkup("a.part.html"),'
     ' /x/);\n});', "pin"),
    ("crates/a/ui-tests/x.test.mjs", 'function f() {}\ntest("p", () => {\n  assert.match(String(f),'
     ' /x/);\n});', "pin"),
    ("crates/a/ui-tests/x.test.mjs", 'function f() {}\ntest("p", () => {\n  assert.match(f.toString'
     '(), /x/);\n});', "pin"),
    ("crates/a/ui-tests/x.test.mjs", 'test("s", () => {\n  assert.equal(String(42), "42");\n});',
     "behavior"),
    ("crates/a/ui-tests/x.test.mjs", 'const PAGE = readFileSync(join(UI, "a.html"), "utf8");\n'
     'test("p", () => {\n  assert.match(PAGE, /x/);\n});', "pin"),
    ("crates/a/ui-tests/x.test.mjs", 'test("p", () => {\n  const m = /="([^"]*)"/;\n  partMarkup('
     '"a.part.html");\n});\ntest("b", () => {\n  assert.equal(1, 1);\n});',
     ["pin", "behavior"]),
    ("crates/a/ui-tests/x.test.mjs", 'const R = /`/;\ntest("p", () => {\n  partMarkup("a.part.html"'
     ');\n});', "pin"),
]
MARKED = ("crates/a/src/x.rs", '#[cfg(test)]\nmod tests {\n// pin: the shell is wired — no node '
          'test loads it\n#[test]\nfn p() { include_str!("../assets/ui/a.ts"); }\n#[test]\nfn q() '
          '{ include_str!("../assets/ui/b.ts"); }\n}')


def selftest() -> int:
    bad = []
    for rel, text, want in SNIPPETS:
        got = [t.kind for t in classify(rel, text).tests]
        if got != (want if isinstance(want, list) else [want]):
            bad.append(f"{rel}: want [{want}], got {got}\n{text}")
    marks = [t.marked for t in classify(*MARKED).tests]
    if marks != [True, False]:
        bad.append(f"marker: want [True, False], got {marks}")
    for b in bad:
        print("FAIL", b)
    print(f"selftest: {len(SNIPPETS) + 1 - len(bad)} of {len(SNIPPETS) + 1} cases pass")
    return 1 if bad else 0


# ---------------------------------------------------------------- main


def pins_of(args: list[str], excludes: list[str]) -> list[tuple[str, Test]]:
    ex = [e.replace("\\", "/").rstrip("/") + "/" for e in excludes]
    paths = [p for p in expand(args) if not any(rel_of(p).startswith(e) for e in ex)]
    return [(f.path, t) for f in classify_all(paths) for t in f.tests if t.kind == "pin"]


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    mode, rest = (argv[0], argv[1:]) if argv else ("--help", [])
    if mode == "--write":
        DOC.write_text(render(classify_all()), encoding="utf-8", newline="\n")
        return 0
    if mode == "--check":
        want = render(classify_all())
        have = DOC.read_text(encoding="utf-8") if DOC.is_file() else ""
        if have != want:
            print("docs/TESTING-KINDS.md is out of date: run python docs/testing/test_kinds.py --write")
            return 1
        return 0
    if mode == "--selftest":
        return selftest()
    if mode in ("--pins", "--unmarked-pins"):
        excludes = [rest[i + 1] for i, a in enumerate(rest) if a == "--exclude" and i + 1 < len(rest)]
        args = [a for i, a in enumerate(rest) if a != "--exclude" and (i == 0 or rest[i - 1] != "--exclude")]
        pins = pins_of(args, excludes)
        if mode == "--unmarked-pins":
            pins = [(p, t) for p, t in pins if not t.marked]
        for path, t in pins:
            print(f"{path}:{t.line}: {t.name}")
        print(f"{'unmarked pins' if mode == '--unmarked-pins' else 'pins'}: {len(pins)}")
        return 0
    print(__doc__)
    return 0 if mode in ("--help", "-h") else 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

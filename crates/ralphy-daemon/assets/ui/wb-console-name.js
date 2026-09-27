/* ---------------------------------------------------------------------------
   Console names (ADR-0066 §§2–4, CONTEXT.md → *Console name*) as pure
   functions of their arguments.

   Every console has a short name a person reads: `<prefix> #<n>` by default,
   renamed by the operator. It is a label, never an identity — the record `id`
   stays the key and `_deskAgent` stays the matching key.

   Nothing here reads the DOM, the store or a module-scope binding:
   `wb-console.js` holds the desk and the windows, feeds them through these
   functions, and paints the answer. Same shape as `wb-columns.js`. The caller
   turns a repo ref into its slug (the routing head removed) before
   `prefixOf`, so this module never reads `WBFleet`.

   Load order: BEFORE `wb-console.js`, on BOTH documents (`index.html` and
   `detached-fence.html`).
   --------------------------------------------------------------------------- */
window.WBConsoleName = (function () {
  // The prefix of a console with no repo (`~`).
  const HOME = "home";
  // The longest name, in code points. The daemon cuts a longer one too.
  const NAME_MAX = 40;

  // The last segment of a repo slug; `home` for no repo.
  function prefixOf(slug) {
    if (slug == null || slug === "" || slug === "~") return HOME;
    const last = String(slug).replace(/\/+$/, "").split("/").pop();
    return last || HOME;
  }

  function escapeRegExp(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }

  // `<prefix> #<n>` with the lowest `n` that no name of that exact form uses.
  function defaultName(prefix, names) {
    const form = new RegExp(`^${escapeRegExp(prefix)} #([1-9][0-9]*)$`);
    const used = new Set();
    for (const name of names || []) {
      const m = form.exec(name || "");
      if (m) used.add(Number(m[1]));
    }
    let n = 1;
    while (used.has(n)) n += 1;
    return `${prefix} #${n}`;
  }

  // What a rename stores: trimmed, cut to NAME_MAX code points, and the
  // default name again when empty. `names` excludes the renamed console.
  function renameValue(raw, prefix, names) {
    const cut = Array.from(String(raw ?? "").trim()).slice(0, NAME_MAX).join("");
    const trimmed = cut.trim();
    return trimmed || defaultName(prefix, names);
  }

  // The two boxes of the title: the name, and the label in parentheses. The
  // title keeps them apart so the label shrinks first.
  function labelParts(name, label) {
    return { name, tag: `(${label})` };
  }

  // The one text that names a console on every surface.
  function consoleLabel(name, label) {
    const p = labelParts(name, label);
    return `${p.name} ${p.tag}`;
  }

  // The title tooltip, one fact per line: the full repo ref, the environment,
  // the Claude session name. An empty fact is left out.
  function tooltipLines(repo, environment, sessionName) {
    return [repo === "~" ? null : repo, environment, sessionName].filter(Boolean);
  }

  // Give every unnamed record a default name, in desk order. Returns new
  // records and never mutates the input, so running it twice changes nothing.
  function nameDesk(records, prefixOfRepo) {
    const names = records.map((r) => r.consoleName).filter(Boolean);
    return records.map((r) => {
      if (r.consoleName) return r;
      const consoleName = defaultName(prefixOfRepo(r.repo), names);
      names.push(consoleName);
      return { ...r, consoleName };
    });
  }

  return {
    NAME_MAX,
    prefixOf,
    defaultName,
    renameValue,
    labelParts,
    consoleLabel,
    tooltipLines,
    nameDesk,
  };
})();

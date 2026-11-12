/* ---------------------------------------------------------------------------
   The console's "new worktree" prompt: the name, the base branch and the
   answer. `createCheckoutSwitcher` (`wb-console-checkout.ts`) asks it and
   makes the worktree.
   --------------------------------------------------------------------------- */
import { setMarkup } from "./wb-html.ts";
import { WBProject } from "./wb-project.ts";
import type { Listing } from "./wb-project.ts";

// What the new worktree prompt is opened with, and what it answers.
export type WorktreeAsk = {
  base: string;
  branches: string[];
  listing: Listing | null | undefined;
  error?: string;
  name?: string;
};
export type WorktreeAnswer = { name: string; base: string } | null;

// The "new worktree" prompt: a name (worktree AND branch, ADR-0063 §2) and
// the base branch. The name gate is `WBProject.worktreeCreateRow`; a refusal
// the daemon DID send (`error`) re-opens with the message under the field.
// Resolves `{name, base}` or `null` on cancel.
export function askWorktree(
  document: Document,
  { base, branches, listing, error = "", name = "" }: WorktreeAsk,
): Promise<WorktreeAnswer> {
  const scrim = document.createElement("div");
  scrim.className = "modal-scrim wb-confirm";
  const modal = document.createElement("div");
  modal.className = "modal confirm-modal wb-worktree";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-label", "New worktree");
  const head = document.createElement("div");
  head.className = "modal-head";
  setMarkup(head, '<i class="bi bi-folder-plus"></i>');
  const heading = document.createElement("span");
  heading.className = "modal-title";
  heading.textContent = "New worktree";
  head.append(heading);
  const form = document.createElement("div");
  form.className = "wb-worktree-form";
  const nameLabel = document.createElement("label");
  nameLabel.textContent = "Name";
  const nameInput = document.createElement("input");
  nameInput.className = "prompt-input";
  nameInput.placeholder = "Worktree and branch name";
  nameInput.value = name;
  const baseLabel = document.createElement("label");
  baseLabel.textContent = "From";
  const baseInput = baseField(document, base, branches);
  const note = document.createElement("p");
  note.className = "wb-worktree-note";
  note.textContent =
    `${WBProject.CARRY_OVER_NOTE} The console restarts in the new worktree. ` +
    "You lose the text in this console.";
  const err = document.createElement("p");
  err.className = "prompt-error";
  err.textContent = error;
  err.hidden = !error;
  form.append(nameLabel, nameInput, baseLabel, baseInput, note, err);
  const foot = document.createElement("div");
  foot.className = "modal-foot";
  const cancel = document.createElement("button");
  cancel.className = "btn";
  cancel.type = "button";
  cancel.textContent = "Cancel";
  const go = document.createElement("button");
  go.className = "btn accent";
  go.type = "button";
  go.textContent = "Create & restart";
  foot.append(cancel, go);
  modal.append(head, form, foot);
  scrim.append(modal);
  document.body.append(scrim);
  nameInput.focus();
  nameInput.select();

  return new Promise((resolve) => {
    let settled = false;
    const done = (value: WorktreeAnswer) => {
      if (settled) return;
      settled = true;
      document.removeEventListener("keydown", onKey, true);
      scrim.remove();
      resolve(value);
    };
    const problem = () =>
      WBProject.worktreeNameProblem(listing ?? { worktrees: [] }, nameInput.value);
    const submit = () => {
      const row = WBProject.worktreeCreateRow(
        listing ?? { worktrees: [] },
        baseInput.value,
        nameInput.value,
      );
      if (!row) {
        err.textContent = problem() || "Choose a branch to start from.";
        err.hidden = false;
        nameInput.focus();
        return;
      }
      done({ name: row.name, base: row.base });
    };
    // The mask runs on every edit, typed or pasted: a refused character
    // never shows, and there is no message. The caret stays after the
    // last kept character. The create stays disabled, without a message,
    // while the name is still one the daemon would refuse.
    const mask = WBProject.maskWorktreeName;
    nameInput.addEventListener("input", () => {
      const raw = nameInput.value;
      const masked = mask(raw);
      if (masked !== raw) {
        const caret = mask(raw.slice(0, nameInput.selectionStart ?? raw.length)).length;
        nameInput.value = masked;
        nameInput.setSelectionRange(caret, caret);
      }
      err.textContent = "";
      err.hidden = true;
      go.disabled = problem() !== "";
    });
    go.disabled = problem() !== "";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        done(null);
      } else if (e.key === "Enter" && modal.contains(document.activeElement)) {
        e.stopPropagation();
        if (document.activeElement === cancel) done(null);
        else submit();
      }
    };
    document.addEventListener("keydown", onKey, true);
    cancel.addEventListener("click", () => {
      done(null);
    });
    go.addEventListener("click", submit);
  });
}

// The prompt's base branch field: a list of the known branches with `base`
// chosen, or free text when no branch could be read.
function baseField(document: Document, base: string, branches: string[]) {
  if (branches.length === 0) {
    const input = document.createElement("input");
    input.className = "prompt-input";
    input.value = base || "";
    input.placeholder = "Branch to start from";
    return input;
  }
  const select = document.createElement("select");
  select.className = "prompt-input";
  for (const b of branches) {
    const opt = document.createElement("option");
    opt.value = b;
    opt.textContent = b;
    select.append(opt);
  }
  select.value = branches.includes(base) ? base : (branches[0] ?? "");
  return select;
}

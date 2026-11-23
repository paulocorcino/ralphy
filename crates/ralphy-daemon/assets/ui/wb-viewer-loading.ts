// The wait of a viewer that reads a whole file before it draws: a note in the
// pane's body, so the pane is never an empty dark area while it loads.

export const LOADING_TEXT = "Loading the file…";

/** Show the loading note in `body`; the returned function removes it. */
export function showLoading(document: Document, body: HTMLElement): () => void {
  const note = document.createElement("p");
  note.className = "empty-note viewer-loading";
  note.textContent = LOADING_TEXT;
  body.append(note);
  return () => {
    note.remove();
  };
}

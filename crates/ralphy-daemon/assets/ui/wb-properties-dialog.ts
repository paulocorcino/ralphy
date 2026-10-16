/* ---------------------------------------------------------------------------
   The Properties dialog of the file tree: an Alpine component around its modal.

   The files send `workbench:properties-open` with the rel path of a row ("" is
   the project root). The dialog asks the daemon that hosts the project, in the
   checkout the tree shows: `file.stat` answers the facts at once, and for a
   folder `tree.size` counts what is inside, which can take seconds, so those
   rows show a spinner until the reply comes. A number per opening drops a
   reply that arrives after the dialog was closed or opened for another row.

   It reaches `shell()` only through the names in its `uses` (ADR-0073 D4).
   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { WBFail } from "./wb-fail.ts";
import { fullPath } from "./wb-file-paths.ts";
import { folderContents, folderSize, formatBytes, modifiedText, typeText, yesNo, type SizeReply } from "./wb-properties-format.ts";

/** One line of the dialog. `pending` shows the spinner; `retry` offers a new try. */
type Row = { label: string; value: string; pending?: boolean; retry?: boolean };

/** The dialog's state: the row it was opened for and what the daemon said. */
type Props = {
  open: boolean;
  path: string;
  isFolder: boolean;
  stat: ReplyOf<"file.stat"> | null;
  statPending: boolean;
  statError: string;
  size: SizeReply | null;
  sizePending: boolean;
  sizeError: string;
  copied: boolean;
};

const closedProps = (): Props => ({
  open: false,
  path: "",
  isFolder: false,
  stat: null,
  statPending: false,
  statError: "",
  size: null,
  sizePending: false,
  sizeError: "",
  copied: false,
});

/** The words of a refusal. An older daemon does not know the verb, and a second
 * count is refused while one runs. */
function refusal(reply: DaemonReply | null, fallback: string) {
  if (reply?.message === "unknown verb") return "This host runs an older daemon. Update it to see properties.";
  if (reply?.reason === "busy") return "Another size is being calculated. Try again.";
  return WBFail.failed(reply, fallback);
}

export function wbPropertiesDialog() {
  return component([], {
    props: closedProps(),
    _propsSeq: 0,

    // On `workbench:properties-open`, which the files send with the rel path of
    // the row. A folder also asks for its size.
    openProperties(detail: { path: string; isFolder: boolean }) {
      const seq = ++this._propsSeq;
      this.props = {
        ...closedProps(),
        open: true,
        path: detail.path,
        isFolder: detail.isFolder || detail.path === "",
        statPending: true,
      };
      this.propsLoadStat(seq);
      if (this.props.isFolder) this.propsLoadSize(seq);
    },

    // The daemon that hosts the open project, in the checkout the tree shows
    // (#406), as every read of the tree does.
    propsAsk<V extends "file.stat" | "tree.size">(verb: V) {
      const slug = this.$store.projects.openSlug;
      const payload = WBDaemon.withCheckout({ repo: slug, path: this.props.path }, this.$store.checkouts.checkoutOf(slug));
      return WBDaemon.observe(verb, payload).catch(() => null);
    },

    async propsLoadStat(seq: number) {
      const reply = await this.propsAsk("file.stat");
      if (seq !== this._propsSeq) return;
      this.props.statPending = false;
      if (!reply || WBFail.isError(reply)) {
        this.props.statError = refusal(reply, "Could not read the properties: the daemon gave no reason.");
        return;
      }
      this.props.stat = reply;
    },

    async propsLoadSize(seq: number) {
      this.props.sizePending = true;
      this.props.sizeError = "";
      const reply = await this.propsAsk("tree.size");
      if (seq !== this._propsSeq) return;
      this.props.sizePending = false;
      if (!reply || WBFail.isError(reply)) {
        this.props.sizeError = refusal(reply, "Could not count the folder: the daemon gave no reason.");
        return;
      }
      this.props.size = reply;
    },

    propsRetrySize() {
      this.propsLoadSize(this._propsSeq);
    },

    propsClose() {
      this._propsSeq++;
      this.props.open = false;
    },

    // The name of the row; the project root has no name of its own.
    propsName() {
      const { path } = this.props;
      if (path) return path.slice(path.lastIndexOf("/") + 1);
      return `${this.$store.projects.projectLabel(this.$store.projects.openSlug)} (project root)`;
    },

    propsRoot() {
      return this.$store.projects.projects.find((p) => p.slug === this.$store.projects.openSlug)?.root || "";
    },

    // The lines of the dialog, in order. The facts of the path come from
    // `file.stat`; the size of a folder comes from `tree.size`.
    propsRows(): Row[] {
      const { path, stat, statError } = this.props;
      const root = this.propsRoot();
      const rows: Row[] = [{ label: "Name", value: this.propsName() }];
      if (path) rows.push({ label: "Path", value: path });
      if (root) rows.push({ label: "Full path", value: fullPath(root, path) });
      if (statError) return [...rows, { label: "Facts", value: statError }];
      rows.push(...this.propsFactRows(stat));
      return rows;
    },

    // Until `file.stat` answers, each fact reads `…`.
    propsFactRows(stat: Props["stat"]): Row[] {
      const s: Partial<ReplyOf<"file.stat">> = stat ?? {};
      const wait = (value: string) => (stat ? value : "…");
      const modified = { label: "Modified", value: wait(modifiedText(s.modified)) };
      const ignored = { label: "Ignored by git", value: wait(yesNo(s.ignored)) };
      if (this.props.isFolder) return [modified, ignored, ...this.propsSizeRows()];
      return [
        { label: "Size", value: wait(formatBytes(s.size ?? 0)) },
        { label: "Type", value: wait(typeText(s.type)) },
        modified,
        { label: "Read-only", value: wait(yesNo(s.readonly)) },
        ignored,
      ];
    },

    // The slow rows of a folder: a spinner until the count is back, then the
    // numbers; a lower bound says so; a refusal says why and offers a new try.
    propsSizeRows(): Row[] {
      const { size, sizePending, sizeError } = this.props;
      if (sizePending) return ["Size", "Contains"].map((label) => ({ label, value: "Calculating…", pending: true }));
      if (sizeError || !size) return [{ label: "Size", value: sizeError || "Unknown", retry: true }];
      const rows: Row[] = [
        { label: "Size", value: folderSize(size) },
        { label: "Contains", value: folderContents(size) },
        { label: "Ignored contents", value: formatBytes(size.ignored_bytes ?? 0) },
      ];
      if (size.truncated) rows.push({ label: "Note", value: "The count stopped after 5 s, so these numbers are a minimum." });
      return rows;
    },

    // What a screen reader hears while the dialog waits and when it is done.
    propsStatus() {
      if (this.props.sizePending) return "Calculating the size of the folder…";
      return this.props.open && this.props.isFolder && this.props.size ? "The size of the folder is ready." : "";
    },

    propsBusy() {
      return this.props.statPending || this.props.sizePending;
    },

    // The visible lines as `Label: value`, one per line. `navigator.clipboard` is
    // undefined on an insecure non-loopback origin, so the call is optional-chained.
    propsCopy() {
      const lines = this.propsRows()
        .filter((r) => !r.pending)
        .map((r) => `${r.label}: ${r.value}`);
      navigator.clipboard?.writeText(lines.join("\n")).catch(() => {});
      this.props.copied = true;
    },
  });
}

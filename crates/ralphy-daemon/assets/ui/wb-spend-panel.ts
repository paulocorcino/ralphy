/* ---------------------------------------------------------------------------
   The Spend tab of the open project, with its Overview and Ledger panes: an
   Alpine component on the `.spend-tab` element (#621; ADR-0073 D2).

   It owns the spend document, the ledger rows, the period and the pane. The
   figures are the daemon's (`/api/spend`, `/api/usage`); `WBSpend` folds only
   which state each pane is in.

   It reads the canvas tabs in the Alpine store `tabs` (wb-tabs-store.ts), and
   asks for its tab with an event, which the store hears. The page reaches
   it only through `workbench:*` events, which the markup and `init()` hear.

   `main.ts` registers it (ADR-0075 D5).
   --------------------------------------------------------------------------- */
import { component } from "./wb-alpine.ts";
import { apiFetch } from "./wb-api.ts";
import { sendWindow } from "./wb-events.ts";
import type { LedgerMissing, LedgerRecord, SpendDoc } from "./wb-shell-types.d.ts";

/** One read of the ledger: the rows, or why there are none. */
type LedgerRead = {
  records: LedgerRecord[];
  interactive: LedgerRecord[];
  missing: LedgerMissing[];
  daemonId: string | null;
  error: string;
};

// `GET /api/usage` for one project and period. A failed read is an error,
// never an empty ledger.
async function readLedger(project: string, period: string): Promise<LedgerRead> {
  const failed = { records: [], interactive: [], missing: [], daemonId: null, error: "Could not load the ledger: the daemon did not answer." };
  try {
    const r = await apiFetch("GET /api/usage", { query: { project, period } });
    if (!r.ok) return failed;
    const data = await r.json();
    return {
      records: Array.isArray(data.records) ? data.records : [],
      interactive: Array.isArray(data.interactive) ? data.interactive : [],
      missing: Array.isArray(data.missing) ? data.missing : [],
      daemonId: data.daemon_id || null,
      error: "",
    };
  } catch {
    return failed;
  }
}

export function wbSpend() {
  // Every `shell()` member this component's code or markup reads or calls.
  return component([], {
    // --- spend (the Spend tab, #358) ---------------------------------------
    // A canvas tab (ADR-0037 amendment: a closable tab may be a daemon view),
    // scoped to the open project. Everything numeric is rendered by the daemon
    // (`/api/spend`); `WBSpend` folds only which state the pane is in.
    spend: { loading: false, error: "", doc: null as SpendDoc, slug: null as string | null },
    // The window the operator picked, echoed back by the daemon; the pane
    // renders the document's own key, never this one.
    spendPeriod: "all",
    // The tab's view model. `openSlug` is read HERE and not stashed, so closing
    // a project drops the pane to its empty state.
    spendView() {
      // Titles ride whatever the board ALREADY holds; never a load
      // (`loadBoard` spawns a throttled tracker CLI). They are read from the
      // store `board`.
      return window.WBSpend.state({
        project: this.$store.projects.openSlug,
        loading: this.spend.loading,
        error: this.spend.error,
        // A document for a project no longer open is stale by definition.
        doc: this.spend.slug === this.$store.projects.openSlug ? this.spend.doc : null,
        period: this.spendPeriod,
        issues: this.$store.board.openIssues(),
      });
    },
    // The window is a server-side filter: assign, then re-read.
    setSpendPeriod(key: string) {
      if (this.spendPeriod === key) return;
      this.spendPeriod = key;
      this.loadSpend();
      // The Ledger is scoped to the SAME window: a period change invalidates
      // its rows too.
      this.ledger.slug = null;
      if (this.spendPane === "ledger") this.loadLedger();
    },
    openSpend() {
      // Bootstrap's cash-stack: the strip renders `t.icon` as a class.
      sendWindow(window, "workbench:tab-open", {
        tab: { id: "spend", kind: "spend", title: "Spend", icon: "bi bi-cash-stack", closable: true },
      });
      this.loadSpend();
    },
    async loadSpend() {
      const slug = this.$store.projects.openSlug;
      // No project open is not a failure: the pane says so itself.
      if (!slug) {
        this.spend = { loading: false, error: "", doc: null, slug: null };
        return;
      }
      this.spend = { ...this.spend, loading: true, error: "" };
      let doc: SpendDoc = null;
      let error = "";
      try {
        const r = await apiFetch("GET /api/spend", { query: { project: slug, period: this.spendPeriod || "all" } });
        if (r.ok) doc = await r.json();
        else error = "Could not load spend: the daemon did not answer.";
      } catch {
        error = "Could not load spend: the daemon did not answer.";
      }
      // The project changed while in flight: one cost under another's name.
      if (this.$store.projects.openSlug !== slug) return;
      this.spend = { loading: false, error, doc, slug };
    },
    // Re-read on activation and when the accordion opens or closes a project.
    refreshSpend() {
      if (!this.$store.tabs.tabs.some((t) => t.id === "spend")) return;
      // Dropping the slug makes the next switch to the Ledger re-read. The
      // unpriced filter goes with it: it answered the PREVIOUS project's gap.
      this.ledger.slug = null;
      this.spendLedgerUnpricedOnly = false;
      this.loadSpend();
      if (this.spendPane === "ledger") this.loadLedger();
    },

    // --- the Ledger pane (#360) --------------------------------------------
    // `Overview | Ledger` inside the ONE Spend tab: two readings of the same
    // project. The Ledger is the raw per-phase grid with the daemon's unpriced
    // verdict per row (`/api/usage?project=`).
    spendPane: "overview",
    // Set by the click-through from the Overview's unpriced figure.
    spendLedgerUnpricedOnly: false,
    ledger: {
      loading: false,
      error: "",
      records: [] as LedgerRecord[],
      interactive: [] as LedgerRecord[],
      missing: [] as LedgerMissing[],
      daemonId: null as string | null,
      slug: null as string | null,
    },
    // What the Spend tab shows, computed ONCE per change by the `x-effect` on
    // `.spend-tab` and on `.ledger-pane`; the markup reads these, never the
    // methods. Alpine caches nothing: each of the ~40 reads ran the whole view
    // again, and the tab is `x-show`, so a board refresh paid it with the tab
    // hidden (wb_perf_487.py: 43 calls, 12 ms per refresh on a real ledger).
    spendModel: window.WBSpend.state(),
    ledgerModel: window.WBSpend.ledger(),
    ledgerView() {
      // Rows for a project no longer open are stale (as `spendView()`); the
      // peer banner is gated with them.
      const fresh = this.ledger.slug === this.$store.projects.openSlug;
      return window.WBSpend.ledger({
        project: this.$store.projects.openSlug,
        loading: this.ledger.loading,
        error: this.ledger.error,
        records: fresh ? this.ledger.records : [],
        interactive: fresh ? this.ledger.interactive : [],
        missing: fresh ? this.ledger.missing : [],
        daemonId: this.ledger.daemonId,
        unpricedOnly: this.spendLedgerUnpricedOnly,
      });
    },
    // Deferred to the first switch: the ledger is the one response that grows
    // with the project's history.
    setSpendPane(key: string) {
      this.spendPane = key;
      if (key === "ledger") this.loadLedger();
    },
    // The Overview's unpriced figure drills into the offending rows (#355).
    showUnpricedLedger() {
      this.spendLedgerUnpricedOnly = true;
      this.setSpendPane("ledger");
    },
    async loadLedger() {
      const slug = this.$store.projects.openSlug;
      // With no project open there are still PEERS to report: an empty
      // `project=` scopes the rows to none while the daemon answers `missing`.
      const want = slug || "";
      // `loading` stops a second click duplicating the heaviest request on the
      // page; `slug` is written only when the fetch lands.
      if (this.ledger.loading) return;
      if (this.ledger.slug === want) return;
      this.ledger = { ...this.ledger, loading: true, error: "" };
      const { records, interactive, missing, daemonId, error } = await readLedger(want, this.spendPeriod || "all");
      // The operator switched projects while this was in flight.
      if ((this.$store.projects.openSlug || "") !== want) {
        this.ledger = { ...this.ledger, loading: false };
        return;
      }
      this.ledger = {
        loading: false,
        error,
        records,
        interactive,
        missing,
        daemonId,
        slug: slug || null,
      };
    },

    init() {
      // The sidebar button opens the tab (`workbench:spend-open`, in the markup).
      // The open project changed or the accordion moved: read the tab again.
      window.addEventListener("workbench:spend-refresh", () => this.refreshSpend());
      // The tab came to the front: its subject can change while it sits in the
      // background.
      window.addEventListener("workbench:spend-shown", () => {
        if (this.spend.slug !== this.$store.projects.openSlug) this.refreshSpend();
      });
      // The panels whose facts have no push: read again on visible and after
      // login while they are open (fact index: usage 3, 4).
      window.addEventListener("workbench:panels-reread", () => {
        if (this.$store.tabs.tabs.some((t) => t.id === "spend")) this.loadSpend();
      });
    },
  });
}

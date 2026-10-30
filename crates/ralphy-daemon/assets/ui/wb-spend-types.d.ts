/* ---------------------------------------------------------------------------
   The documents `wb-spend.ts` folds, and the records of its Ledger pane, as the
   daemon sends them. Types only: the build skips a `.d.ts` file, so a module
   imports from here with `import type`.
   --------------------------------------------------------------------------- */

// The documents this module folds, as the daemon sends them (spend.rs and its
// modules). Every field is optional: a daemon of an older build sends fewer,
// and each read here has its default.
/** The `/api/spend` summary document (`SpendSummary`). */
export type SpendDoc = {
  project?: string;
  usd?: number | null;
  floor?: boolean;
  total?: string;
  tokens?: Tokens;
  unpriced?: Unpriced;
  period?: { key?: string; label?: string; since?: string | null };
  deliveries?: Delivery[];
  deliveries_truncated?: number;
  overhead?: Overhead;
  kpis?: Kpis;
  models?: ModelRow[];
  activity?: ActivityDay[];
};
/** The summary's token meter (`TokenMeter`). */
type Tokens = {
  total?: number;
  label?: string;
  meter?: string;
  parts?: {
    key: string;
    glyph?: string;
    name?: string;
    tokens: number;
    label?: string;
    share?: number;
    share_label?: string;
  }[];
};
/** The `unpriced` part of the summary (`Unpriced`). */
type Unpriced = {
  tokens?: number;
  label?: string;
  share?: number;
  share_label?: string;
  priced?: number;
  priced_label?: string;
  priced_share?: number;
  priced_share_label?: string;
  causes?: { key: string; tokens?: number; label?: string; share?: number; share_label?: string }[];
  unmetered_sessions?: number;
};
/** One delivery row (`DeliveryRow`). */
type Delivery = {
  issue: number;
  total?: string;
  floor?: boolean;
  attempts?: number;
  tokens_label?: string;
  share?: number;
  share_label?: string;
};
/** What the summary spent outside the deliveries (`Overhead`). */
type Overhead = {
  deliveries_total?: string;
  deliveries_floor?: boolean;
  interactive_total?: string;
  interactive_floor?: boolean;
  interactive_sessions?: number;
  consolidation_total?: string;
  consolidation_floor?: boolean;
};
/** The headline figures (`Kpis`). */
type Kpis = {
  deliveries?: number;
  cost_per_delivery_median_label?: string;
  cost_per_delivery_mean_label?: string;
  cost_per_delivery_floor?: boolean;
  retry_burn_label?: string;
  retry_burn_floor?: boolean;
  cache_hit_label?: string;
};
/** One model row (`ModelRow`). */
type ModelRow = {
  model: string;
  total?: string;
  floor?: boolean;
  share?: number;
  share_label?: string;
  tokens_label?: string;
  priced?: boolean;
};
/** One day of the activity band (`ActivityDay`). */
type ActivityDay = {
  date: string;
  usd: number;
  usd_label?: string;
  usd_share?: number;
  deliveries: number;
  deliveries_share?: number;
};
/** One run record of the `/api/usage` ledger, or one interactive record. */
export type SpendRecord = {
  daemon_id?: string;
  issue?: number;
  phase?: string;
  agent?: string;
  model?: string;
  outcome?: string;
  actor_name?: string;
  actor_email?: string;
  ralphy_version?: string;
  ts?: string;
  first_ts?: string;
  last_ts?: string;
  /** The four counts, or `null` when the vendor keeps no count. */
  tokens?: { [part: string]: number | null | undefined } | null;
  lower_bound?: boolean;
  unpriced_cause?: string;
};
/** A peer whose usage did not arrive (`/api/usage` `missing`). */
export type MissingUsage = { daemon_id: string; environment: string; why: string };
/** An issue of the board, for a delivery's title. */
export type TitledIssue = { number: number; title?: string };

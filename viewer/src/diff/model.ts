/* Diff model. This module owns what the PR diff view derives once from a frozen diff: the steps of each side with
   their change kind, the cluster runs of each side, the Before | After rows of the overview with their step counts,
   the matched step slots of an open row, and the numbered lines of one step diff. It never reads the DOM.
   Clusters only split a long call stack. Runs with the same label share a row, but a row is layout only: every
   change is a step fact.
   # Sections
   model, runs and rows, slots, lines */
import type { ChangeKind, DiffArtifact, DiffLine, DiffRow, FlowArtifact, StepLedger } from "../data/models.ts";

// ============ model ============

export type Side = "before" | "after";

/** The kinds that count as a change, in summary order. */
export const KINDS = ["new", "removed", "changed", "moved"] as const;
export type Counted = typeof KINDS[number];

/** One step of one side. `index` counts from 1 in that side's flow order. `row` is null for an unchanged step that the diff has no row for. */
export interface SideStep { side: Side; id: string; index: number; title: string; ledger: StepLedger; cluster: string | null; kind: ChangeKind; row: DiffRow | null }

/** Consecutive steps of one side with the same cluster. */
export interface Run { cluster: string | null; label: string; steps: SideStep[] }

/** One overview row: a Before run, an After run, or both. `continues` marks an After run whose label came before.
 *  `entry` is the After step that the deck opens for the row. */
export interface OverviewRow {
  before: Run | null; after: Run | null; continues: boolean;
  counts: Record<Counted, number>; added: number; removed: number; changed: boolean;
  notes: Record<Side, string | null>; entry: number;
}

export interface DiffModel {
  name: string;
  before: SideStep[];
  after: SideStep[];
  rows: OverviewRow[];
  totals: Record<Counted, number>;
  /** The row whose After run holds an After step index, or -1. */
  rowOfAfter(index: number): number;
  /** The After step that a Before step became, or null for a removed step. */
  afterOf(step: SideStep): SideStep | null;
}

/** Returns the moved-file text of a moved step, or null. */
export function movedText(row: DiffRow | null): string | null {
  return row?.kind === "moved" && row.baseSpan && row.headSpan ? `moved from ${row.baseSpan.path} to ${row.headSpan.path}` : null;
}

/** Derives the diff model. A step that the diff has no row for is unchanged. A Before step takes the kind of its pair. */
export function buildDiffModel(diff: DiffArtifact): DiffModel {
  const byAfter = new Map<string, DiffRow>(), byBefore = new Map<string, DiffRow>();
  for (const row of diff.rows) {
    if (row.afterStep) byAfter.set(row.afterStep, row);
    if (row.beforeStep) byBefore.set(row.beforeStep, row);
  }
  const before = sideSteps(diff.base.flow, "before", byBefore), after = sideSteps(diff.head.flow, "after", byAfter);
  const afterById = new Map(after.map(step => [step.id, step]));
  const pairOf = new Map(diff.pairs.map(pair => [pair.before, pair.after]));
  const afterOf = (step: SideStep) => (step.side === "after" ? step : afterById.get(pairOf.get(step.id) ?? "") ?? null);
  const notes = new Map(diff.notes.map(note => [`${note.side}\n${note.clusterKey}`, note.text]));
  const rows = alignRuns(runsOf(before, diff.base.flow), runsOf(after, diff.head.flow)).map(({ before: b, after: a }) =>
    describeRow(b, a, notes, afterOf));
  const seen = new Set<string>();
  rows.forEach(row => { if (row.after) { row.continues = seen.has(row.after.label); seen.add(row.after.label); } });
  // A row with no After run opens at the step after it in the After flow.
  rows.forEach((row, i) => {
    if (row.entry) return;
    row.entry = rows.slice(i + 1).find(next => next.after)?.after!.steps[0]!.index ?? after.length;
  });
  const totals = zero();
  for (const step of [...after, ...before]) if (counted(step.kind) && (step.side === "after" || step.kind === "removed")) totals[step.kind]++;
  return {
    name: diff.name, before, after, rows, totals, afterOf,
    rowOfAfter: index => rows.findIndex(row => row.after?.steps.some(step => step.index === index)),
  };
}

function sideSteps(flow: FlowArtifact, side: Side, rows: ReadonlyMap<string, DiffRow>): SideStep[] {
  return flow.intent.steps.map((step, i) => {
    const row = rows.get(step.id) ?? null;
    return { side, id: step.id, index: i + 1, title: step.title, ledger: step.ledger, cluster: step.clusterKey, kind: row?.kind ?? "unchanged", row };
  });
}

const zero = (): Record<Counted, number> => ({ new: 0, removed: 0, changed: 0, moved: 0 });
const counted = (kind: ChangeKind): kind is Counted => kind !== "unchanged";

// ============ runs and rows ============

/** Splits the steps of one side into runs of the same cluster. A step with no cluster forms runs of its own. */
export function runsOf(steps: readonly SideStep[], flow: FlowArtifact): Run[] {
  const labels = new Map(flow.intent.clusters.map(cluster => [cluster.key, cluster.label]));
  const runs: Run[] = [];
  for (const step of steps) {
    const last = runs[runs.length - 1];
    if (last && last.cluster === step.cluster) last.steps.push(step);
    else runs.push({ cluster: step.cluster, label: (step.cluster && labels.get(step.cluster)) || "No cluster", steps: [step] });
  }
  return runs;
}

/** Puts Before runs and After runs in one row list: runs with the same label share a row (longest common subsequence). */
export function alignRuns(b: readonly Run[], a: readonly Run[]): { before: Run | null; after: Run | null }[] {
  return lcs(b, a, (x, y) => x.label === y.label).map(([before, after]) => ({ before, after }));
}

/** Counts the changed steps and lines of one row. The After run counts its new, changed, and moved steps; the Before run
 *  counts its removed steps. Two steps that show the same function count its lines once. */
function describeRow(b: Run | null, a: Run | null, notes: ReadonlyMap<string, string>, afterOf: (step: SideStep) => SideStep | null): OverviewRow {
  const counts = zero(), seen = new Set<string>();
  let added = 0, removed = 0;
  const steps = [...(a?.steps ?? []), ...(b?.steps ?? []).filter(step => step.kind === "removed")];
  for (const step of steps) {
    if (!counted(step.kind) || !step.row) continue;
    counts[step.kind]++;
    const span = step.row.headSpan ?? step.row.baseSpan, key = span ? `${span.path}:${span.start}` : `${step.side}:${step.id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    added += step.row.added; removed += step.row.removed;
  }
  const note = (side: Side, run: Run | null) => (run?.cluster ? notes.get(`${side}\n${run.cluster}`) ?? null : null);
  const firstAfter = (run: Run | null) => {
    const steps = run?.steps.map(afterOf).filter((step): step is SideStep => !!step) ?? [];
    return (steps.find(step => step.kind !== "unchanged") ?? steps[0])?.index ?? 0;
  };
  return {
    before: b, after: a, continues: false, counts, added, removed, changed: KINDS.some(kind => counts[kind] > 0),
    notes: { before: note("before", b), after: note("after", a) },
    entry: firstAfter(a) || firstAfter(b),
  };
}

// ============ slots ============

/** One line of an open row: a Before step and an After step that are the same step, or one of them alone. */
export interface Slot { before: SideStep | null; after: SideStep | null }

/** Matches the Before steps and the After steps of one row by the agent pairs, in flow order. */
export function slotsOf(model: DiffModel, row: OverviewRow): Slot[] {
  const same = (b: SideStep, a: SideStep) => model.afterOf(b) === a;
  return lcs(row.before?.steps ?? [], row.after?.steps ?? [], same).map(([before, after]) => ({ before, after }));
}

/** Aligns two lists by the longest common subsequence of a match. Unmatched items get a null partner, in list order. */
function lcs<B, A>(b: readonly B[], a: readonly A[], match: (x: B, y: A) => boolean): [B | null, A | null][] {
  const L = Array.from({ length: b.length + 1 }, () => new Array<number>(a.length + 1).fill(0));
  for (let i = b.length - 1; i >= 0; i--) for (let j = a.length - 1; j >= 0; j--) {
    L[i]![j] = match(b[i]!, a[j]!) ? L[i + 1]![j + 1]! + 1 : Math.max(L[i + 1]![j]!, L[i]![j + 1]!);
  }
  const out: [B | null, A | null][] = [];
  let i = 0, j = 0;
  while (i < b.length || j < a.length) {
    if (i < b.length && j < a.length && match(b[i]!, a[j]!)) out.push([b[i++]!, a[j++]!]);
    else if (j >= a.length || (i < b.length && L[i + 1]![j]! >= L[i]![j + 1]!)) out.push([b[i++]!, null]);
    else out.push([null, a[j++]!]);
  }
  return out;
}

// ============ lines ============

/** One numbered diff line. `no` is the new line number, or the old one for a removed line. */
export interface NumberedLine { op: DiffLine["op"]; text: string; no: number | null }

/** Numbers the lines of one step diff. A context line moves both counters, a removed line the old one, and an added line
 *  the new one. The old counter starts at the base span, the new counter at the head span. */
export function numberLines(row: DiffRow): NumberedLine[] {
  let old = row.baseSpan?.start ?? null, next = row.headSpan?.start ?? null;
  return row.lines.map(line => {
    const no = line.op === "removed" ? old : next ?? old;
    if (line.op !== "added" && old !== null) old++;
    if (line.op !== "removed" && next !== null) next++;
    return { op: line.op, text: line.text, no };
  });
}

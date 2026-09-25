/* Flow plan. This module owns the step bookkeeping that one flow needs: visits, runs, flow rows,
   neighbor rows, pinned paths, and stack order. It reads rows and deck data; it never reads the DOM.
   # Sections
   plan, runs and visits, stack order */
import type { DeckCall, DeckCluster, DeckFlow } from "../data/models.ts";
import type { Row } from "./tree.ts";

// ============ plan ============

export interface Run { cluster: string; start: number; end: number }

export interface FlowPlan {
  /** Every visited function and every member of a visited cluster. Each is a named row with its cluster marker. */
  flowFns: Set<string>;
  /** Every other function with a static call to or from a flow function. Each is a plain named row. */
  flowNear: Set<string>;
  /** The files that hold a flow function, and the directories above them. A closed directory still shows them. */
  pinned: Set<Row>;
  /** The directories and files on the path to a named row. The flow opens them once, at its start. */
  openRows: Row[];
  /** Step indexes for each function. */
  visits: Map<string, number[]>;
  /** Runs of consecutive steps with the same effective cluster. */
  runs: Run[];
}

/** An empty plan, for a codebase with no flow. */
export const EMPTY_PLAN: FlowPlan = { flowFns: new Set(), flowNear: new Set(), pinned: new Set(), openRows: [], visits: new Map(), runs: [] };

/** Returns the cluster that lifts at a step: its labeled cluster when that cluster has a member in the graph, else null. */
export function effectiveCluster(flow: DeckFlow, clusters: ReadonlyMap<string, DeckCluster>, index: number): string | null {
  const step = flow.steps[index - 1];
  if (!step) return null;
  const cluster = clusters.get(step.cluster);
  return cluster && cluster.members.length ? cluster.id : null;
}

/** Builds the plan of one flow over the rows of the current graph. */
export function buildPlan(flow: DeckFlow, calls: readonly DeckCall[], rowById: ReadonlyMap<string, Row>): FlowPlan {
  const clusters = new Map(flow.clusters.map(cluster => [cluster.id, cluster]));
  const visits = new Map<string, number[]>();
  for (const step of flow.steps) {
    if (step.missing) continue;
    if (!visits.has(step.node)) visits.set(step.node, []);
    visits.get(step.node)!.push(step.index);
  }
  const runs: Run[] = [];
  for (const step of flow.steps) {
    const cluster = effectiveCluster(flow, clusters, step.index) ?? "";
    const last = runs[runs.length - 1];
    if (last && last.cluster === cluster && last.end === step.index - 1) last.end = step.index;
    else runs.push({ cluster, start: step.index, end: step.index });
  }
  const flowFns = new Set<string>(visits.keys());
  for (const step of flow.steps) for (const id of clusters.get(step.cluster)?.members ?? []) flowFns.add(id);
  const flowNear = new Set<string>();
  for (const call of calls) {
    const a = flowFns.has(call.source), b = flowFns.has(call.target);
    if (a && !b) flowNear.add(call.target); else if (b && !a) flowNear.add(call.source);
  }
  const pinned = new Set<Row>(), open = new Set<Row>();
  for (const id of flowFns) {
    const row = rowById.get(id);
    if (!row) continue;
    pinned.add(row.parent!);
    for (const dir of row.dirs) pinned.add(dir);
  }
  for (const id of [...flowFns, ...flowNear]) {
    const row = rowById.get(id);
    if (!row) continue;
    open.add(row.parent!);
    for (const dir of row.dirs) open.add(dir);
  }
  return { flowFns, flowNear, pinned, openRows: [...open], visits, runs };
}

// ============ runs and visits ============

/** Returns the run that holds a step, or null. */
export const runAt = (plan: FlowPlan, index: number) => plan.runs.find(run => index >= run.start && index <= run.end) ?? null;

/** Returns the step ranges of a cluster, for example "1–4, 15–20, 37". */
export const rangesOf = (plan: FlowPlan, cluster: string) => plan.runs.filter(run => run.cluster === cluster)
  .map(run => run.start === run.end ? `${run.start}` : `${run.start}–${run.end}`).join(", ");

/** Returns the visit of a function nearest to the current step, preferring visits in the current pass. */
export function nearestStep(plan: FlowPlan, id: string, current: number): number | null {
  const visits = plan.visits.get(id) ?? [];
  if (!visits.length) return null;
  const run = runAt(plan, current);
  const inRun = run ? visits.filter(i => i >= run.start && i <= run.end) : [];
  const pool = inRun.length ? inRun : visits;
  return pool.reduce((a, b) => (Math.abs(b - current) < Math.abs(a - current) ? b : a));
}

// ============ stack order ============

/** Stack order: members visited in the current pass first, in step order; then the rest by first visit. */
export function dockOrder(plan: FlowPlan, cluster: DeckCluster, current: number): string[] {
  const run = runAt(plan, current), inRun = run && run.cluster === cluster.id ? run : null;
  const key = (id: string) => {
    const visits = plan.visits.get(id) ?? [];
    const first = inRun ? visits.find(i => i >= inRun.start && i <= inRun.end) : undefined;
    return [first === undefined ? 1e9 : first, visits.length ? visits[0] : 1e9];
  };
  return cluster.members.slice().sort((a, b) => { const ka = key(a), kb = key(b); return ka[0] - kb[0] || ka[1] - kb[1]; });
}

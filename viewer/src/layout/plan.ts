/* Flow plan. This module owns the step bookkeeping of one flow: visits, runs, the steps of each file,
   and member order. It reads deck data; it never reads the DOM.
   # Sections
   plan, runs and visits, members and files */
import type { DeckCluster, DeckFlow } from "../data/models.ts";

// ============ plan ============

export interface Run { cluster: string; start: number; end: number }

export interface FlowPlan {
  /** Step indexes for each function. */
  visits: Map<string, number[]>;
  /** Runs of consecutive steps with the same effective cluster. */
  runs: Run[];
  /** Step indexes for each file that holds a step function, in step order. */
  flowFiles: Map<string, number[]>;
}

/** An empty plan, for a codebase with no flow. */
export const EMPTY_PLAN: FlowPlan = { visits: new Map(), runs: [], flowFiles: new Map() };

/** Returns the cluster that lifts at a step: its labeled cluster when that cluster has a member in the graph, else null. */
export function effectiveCluster(flow: DeckFlow, clusters: ReadonlyMap<string, DeckCluster>, index: number): string | null {
  const step = flow.steps[index - 1];
  if (!step) return null;
  const cluster = clusters.get(step.cluster);
  return cluster && cluster.members.length ? cluster.id : null;
}

/** Builds the plan of one flow. `fileOf` returns the file of a function in the current graph. */
export function buildPlan(flow: DeckFlow, fileOf: (key: string) => string | undefined): FlowPlan {
  const clusters = new Map(flow.clusters.map(cluster => [cluster.id, cluster]));
  const visits = new Map<string, number[]>(), flowFiles = new Map<string, number[]>();
  for (const step of flow.steps) {
    if (step.missing) continue;
    if (!visits.has(step.node)) visits.set(step.node, []);
    visits.get(step.node)!.push(step.index);
    const file = fileOf(step.node);
    if (file === undefined) continue;
    if (!flowFiles.has(file)) flowFiles.set(file, []);
    flowFiles.get(file)!.push(step.index);
  }
  const runs: Run[] = [];
  for (const step of flow.steps) {
    const cluster = effectiveCluster(flow, clusters, step.index) ?? "";
    const last = runs[runs.length - 1];
    if (last && last.cluster === cluster && last.end === step.index - 1) last.end = step.index;
    else runs.push({ cluster, start: step.index, end: step.index });
  }
  return { visits, runs, flowFiles };
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

// ============ members and files ============

/** Members in first-visit order. A member that no step visits goes last, in assignment order. */
export function memberOrder(plan: FlowPlan, cluster: DeckCluster): string[] {
  const first = (id: string) => plan.visits.get(id)?.[0] ?? Number.MAX_SAFE_INTEGER;
  return cluster.members.map((id, i) => ({ id, i })).sort((a, b) => first(a.id) - first(b.id) || a.i - b.i).map(m => m.id);
}

/** The file's next step after the current one, wrapping to its first, or null when the file has none. */
export function nextStepOfFile(plan: FlowPlan, file: string, current: number): number | null {
  const steps = plan.flowFiles.get(file);
  if (!steps?.length) return null;
  return steps.find(i => i > current) ?? steps[0];
}

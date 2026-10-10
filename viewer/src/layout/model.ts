/* Deck model. This module owns what the deck derives once from a graph and a flow, and the mutable state of one deck:
   cluster membership and colors, the model maps, the rest state, and the file that the file box shows.
   It never reads the DOM.
   # Sections
   model, state */
import type { ClusterColor, DeckCluster, DeckFlow, DeckFunction, DeckGraph, DeckStep, SourceView } from "../data/models.ts";
import { deckSize as deckSizeOf } from "./deck-geometry.ts";
import { buildExplorer, openTo, type ExplorerRow } from "./explorer.ts";
import { fileBox } from "./file-box.ts";
import { buildPlan, effectiveCluster, EMPTY_PLAN, memberOrder, type FlowPlan } from "./plan.ts";

// ============ model ============

export type ClusterView = DeckCluster & { cv: string };

/** Everything the deck derives once from the graph and the flow. It never changes while the deck lives. */
export interface DeckModel {
  flow: DeckFlow | null;
  graph: DeckGraph;
  clusters: ClusterView[];
  CL: Map<string, ClusterView>;
  FN: Map<string, DeckFunction>;
  steps: DeckStep[];
  N: number;
  plan: FlowPlan;
  explorer: ExplorerRow[];
  rowByPath: Map<string, ExplorerRow>;
  fnsByFile: Map<string, DeckFunction[]>;
  /** The clusters that each flow file holds, in cluster order. */
  clustersByFile: Map<string, string[]>;
  /** Member keys of each cluster, in first-visit order. */
  members: Map<string, string[]>;
  deckSize: { width: number; maxHeight: number };
  /** The widest file box right edge over every flow file, compact on, for the starting zoom. */
  fileRight: number;
  sources: ReadonlyMap<string, SourceView>;
  /** The step at an index, or null at rest. */
  stepAt(index: number): DeckStep | null;
  /** The graph function of the step at an index, or null at rest and for a step function that the graph lost. */
  fnAt(index: number): DeckFunction | null;
  /** The cluster that lifts at a step, or null. */
  stepCluster(index: number): string | null;
}

/** Returns the CSS color of a cluster. */
const colorVar = (color: ClusterColor) => `var(--cl-${color})`;

/** Derives the deck model from a graph, a flow, and the step sources. */
export function buildModel(sourceGraph: DeckGraph, flow: DeckFlow | null, sources: ReadonlyMap<string, SourceView>): DeckModel {
  const graph = withMembership(sourceGraph, flow);
  const clusters: ClusterView[] = (flow?.clusters ?? []).map(cluster => ({ ...cluster, cv: colorVar(cluster.color) }));
  const CL = new Map(clusters.map(cluster => [cluster.id, cluster]));
  const FN = new Map(graph.functions.map(fn => [fn.id, fn]));
  const plan = flow ? buildPlan(flow, key => FN.get(key)?.file) : EMPTY_PLAN;
  const fnsByFile = new Map<string, DeckFunction[]>();
  for (const fn of graph.functions) { if (!fnsByFile.has(fn.file)) fnsByFile.set(fn.file, []); fnsByFile.get(fn.file)!.push(fn); }
  const clustersByFile = new Map<string, string[]>();
  for (const [file, steps] of plan.flowFiles) {
    const held = new Set(steps.map(i => (flow ? effectiveCluster(flow, CL, i) : null)).filter((c): c is string => !!c));
    clustersByFile.set(file, clusters.map(c => c.id).filter(id => held.has(id)));
  }
  const members = new Map(clusters.map(cluster => [cluster.id, memberOrder(plan, cluster)]));
  const deckSize = deckSizeOf([...members.values()].map(keys => keys.map(key => ({ name: FN.get(key)!.name, file: FN.get(key)!.file, steps: plan.visits.get(key) ?? [] }))));
  const fileRight = Math.max(0, ...[...plan.flowFiles.keys()].map(file => fileBox({
    fns: fnsByFile.get(file) ?? [], visits: plan.visits, currentCluster: "", currentKey: null, compact: true, folded: false,
  }).right));
  const explorer = buildExplorer(graph.tree);
  const steps = flow?.steps ?? [];
  const stepAt = (index: number) => steps[index - 1] ?? null;
  return {
    flow, graph, clusters, CL, FN, steps, N: steps.length, plan, explorer,
    rowByPath: new Map(explorer.map(row => [row.path, row])), fnsByFile, clustersByFile, members, deckSize, fileRight, sources,
    stepAt,
    fnAt: index => { const step = stepAt(index); return step && !step.missing ? FN.get(step.node) ?? null : null; },
    stepCluster: index => (flow ? effectiveCluster(flow, CL, index) : null),
  };
}

/** Returns a copy of the graph where each function carries the flow cluster that holds it. */
export function withMembership(graph: DeckGraph, flow: DeckFlow | null): DeckGraph {
  const clusterOf = new Map<string, string>();
  for (const cluster of flow?.clusters ?? []) for (const id of cluster.members) if (!clusterOf.has(id)) clusterOf.set(id, cluster.id);
  return { ...graph, functions: graph.functions.map(fn => ({ ...fn, cluster: clusterOf.get(fn.id) ?? null })) };
}

// ============ state ============

/** The mutable state of one deck. */
export interface DeckState {
  /** The current step; 0 is the rest state. */
  cur: number;
  /** Compact starts on at every load. Only the compact button and `c` turn it off. */
  compact: boolean;
  zoom: number;
  full: boolean;
  /** Open directories. */
  open: Set<string>;
  /** Files whose rows off the flow are folded. */
  folded: Set<string>;
  /** The file whose box shows at rest after an explorer click, or null. */
  browseFile: string | null;
  /** The file node's screen anchor after the last place(). */
  anchor: { x: number; y: number } | null;
}

/** Returns the rest state of a new deck. The path to every flow file starts open. */
export function initialState(model: DeckModel): DeckState {
  const open = new Set<string>();
  for (const file of model.plan.flowFiles.keys()) {
    const row = model.rowByPath.get(file);
    if (row) openTo(open, row);
  }
  return { cur: 0, compact: true, zoom: 1, full: false, open, folded: new Set(), browseFile: null, anchor: null };
}

/** Returns the file whose box shows, with the cluster and the function that the box marks as current.
 *  A browsed file wins over the step and marks neither. A step with no function in the graph shows no file. */
export function shownFile(model: DeckModel, state: DeckState): { file: string; cluster: string | null; key: string | null } | null {
  if (state.browseFile) return { file: state.browseFile, cluster: null, key: null };
  const fn = model.fnAt(state.cur);
  return fn ? { file: fn.file, cluster: model.stepCluster(state.cur), key: fn.id } : null;
}

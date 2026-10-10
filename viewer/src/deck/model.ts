/* Deck model. This module owns what the deck modules share for one codebase and one flow:
   the DOM helpers, the derived model, the mutable state, the part interfaces, and the context.
   `deck.ts` builds one context; every other deck module reads and writes through it.
   # Sections
   DOM helpers, storage, model, state, parts, context */
import type { DeckCluster, DeckFlow, DeckFunction, DeckGraph, DeckStep, SourceView } from "../data/models.ts";
import { withMembership } from "../data/adapt.ts";
import { deckSize as deckSizeOf } from "../layout/deck-geometry.ts";
import { buildExplorer, openTo, type ExplorerRow } from "../layout/explorer.ts";
import { fileBox } from "../layout/file-box.ts";
import { buildPlan, effectiveCluster, EMPTY_PLAN, memberOrder, type FlowPlan } from "../layout/plan.ts";
import { colorVar } from "./markup.ts";

// ============ DOM helpers ============

export const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector(selector) as T;

// ============ storage ============

/** Per-viewer conveniences. Blocked storage keeps the in-memory state. */
export const store = {
  get(key: string) { try { return localStorage.getItem(key); } catch { return null; } },
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* storage is blocked */ } },
};
export const KEY_VARIANT = "tekdek.deck.variant";

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
  /** The cluster that lifts at a step, or null. */
  stepCluster(index: number): string | null;
}

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
  return {
    flow, graph, clusters, CL, FN, steps: flow?.steps ?? [], N: flow?.steps.length ?? 0, plan, explorer,
    rowByPath: new Map(explorer.map(row => [row.path, row])), fnsByFile, clustersByFile, members, deckSize, fileRight, sources,
    stepCluster: index => (flow ? effectiveCluster(flow, CL, index) : null),
  };
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
  const step = state.cur ? model.steps[state.cur - 1] : undefined;
  const fn = step && !step.missing ? model.FN.get(step.node) : undefined;
  return fn ? { file: fn.file, cluster: model.stepCluster(state.cur), key: fn.id } : null;
}

// ============ parts ============

/** Created by `explorer.ts`. */
export interface Explorer {
  /** Marks the current file, the current cluster's files, and the open state in the rows. */
  sync(): void;
  /** Scrolls a long tree so the current file's row sits inside the canvas frame's height. */
  reveal(): void;
  /** The row element that the guide starts from, or null when no file shows. */
  anchorEl(): HTMLElement | null;
  destroy(): void;
}

/** Created by `file-box.ts`. */
export interface FileBoxView {
  render(): void;
  /** The height of the shown box, from its top edge to the bottom of its last row. */
  height(): number;
  nodeEl(): HTMLElement | null;
  /** The current function's row node, or null when no row is current. */
  slotEl(): HTMLElement | null;
  /** The largest right edge of the rows on screen. */
  rowsRight(): number;
  destroy(): void;
}

/** Created by `cluster-deck.ts`. */
export interface ClusterDeck {
  /** Replaces the deck with the deck of a cluster, already raised, or removes it for null. */
  show(cluster: string | null): void;
  /** Marks the current box and shows the file path on the boxes of other files. */
  mark(): void;
  /** Puts the deck at its screen spot while the canvas scales by k around the anchor. */
  pin(spot: { x: number; y: number }, anchor: { x: number; y: number }, k: number): void;
  /** The lid of the current step's box, or null when the deck has none. */
  currentLid(): HTMLElement | null;
  /** The right edge of the deck's title band, or 0 without a deck. */
  deckRight(): number;
  destroy(): void;
}

/** Created by `links.ts`. */
export interface Links { draw(): void; destroy(): void }

/** Declared here and created by `code.ts`. */
export interface CodeColumn { render(prev: number): void; destroy(): void }
/** Declared here and created by `bar.ts`. */
export interface Bar { render(clusterChanged: boolean): void; destroy(): void }
/** Declared here and created by `tools.ts`. */
export interface Tools {
  sync(): void; layoutTop(): void; setFull(on: boolean): void; toggleCompact(): void; setZoom(k: number): void;
  keyOpen(): boolean; setKey(open: boolean, returnFocus?: boolean): void; destroy(): void;
}
/** Declared here and created by `input.ts`. */
export interface Input { destroy(): void }

/** The deck modules. `deck.ts` fills this object after it creates every module. */
export interface DeckParts {
  explorer: Explorer; fileBox: FileBoxView; clusterDeck: ClusterDeck; links: Links;
  code: CodeColumn; bar: Bar; tools: Tools; input: Input;
}

// ============ context ============

/** Options that the page gives a deck. */
export interface DeckOptions {
  graph: DeckGraph;
  flow: DeckFlow | null;
  /** The source of each step function, by function key. Empty when the flow has none. */
  sources: ReadonlyMap<string, SourceView>;
  /** Number of flows in the codebase. The flow badge is a picker when it is more than one. */
  flowCount: number;
  onFlowPick?: (anchor: HTMLElement) => void;
  onStep?: (step: number) => void;
}

/** The shared context of one deck. */
export interface DeckContext {
  options: DeckOptions;
  model: DeckModel;
  state: DeckState;
  parts: DeckParts;
  /** Aborts every listener of the deck on teardown. */
  signal: AbortSignal;
  /** True when the viewer prefers reduced motion. */
  reducedMotion: boolean;
  /** Goes to a step. */
  go(step: number): void;
  /** Places the file box and the deck, then draws the connectors. */
  place(): void;
  /** Draws the connectors on every frame for a while (layout settles, fonts load), then once at rest. */
  kick(ms: number): void;
}

/* Deck context. This module owns what the deck modules share for one codebase and one flow:
   the DOM helpers, the derived model, the mutable state, and the late-bound module parts.
   `deck.ts` builds one context; every other deck module reads and writes through it.
   # Sections
   DOM helpers, storage, model, state, context */
import type { DeckCall, DeckCluster, DeckFlow, DeckFunction, DeckGraph } from "../data/models.ts";
import { withMembership } from "../data/adapt.ts";
import { buildRows, restOpenState, type Opener, type Row } from "../layout/tree.ts";
import { buildPlan, effectiveCluster, EMPTY_PLAN, type FlowPlan } from "../layout/flow-plan.ts";
import { viewOf, type Cam, type View } from "../layout/camera.ts";
import { colorVar } from "./panels.ts";
import type { Connectors } from "./connectors.ts";
import type { Controls } from "./controls.ts";
import type { Input } from "./input.ts";
import type { Panels } from "./panels.ts";
import type { Sidebar } from "./sidebar.ts";
import type { Viewport } from "./viewport.ts";
import type { World } from "./world.ts";

// ============ DOM helpers ============

export const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector(selector) as T;
export const mk = (tag: string, cls?: string, html?: string) => {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (html != null) el.innerHTML = html;
  return el;
};
const SVGNS = "http://www.w3.org/2000/svg";
export const svgEl = (tag: string, cls?: string) => {
  const el = document.createElementNS(SVGNS, tag);
  if (cls) el.setAttribute("class", cls);
  return el;
};

// ============ storage ============

/** Per-viewer conveniences. Blocked storage keeps the in-memory state. */
export const store = {
  get(key: string) { try { return localStorage.getItem(key); } catch { return null; } },
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* storage is blocked */ } },
};
export const KEY_VARIANT = "tekdek.deck.variant";
export const KEY_ANN = "tekdek.deck.annotation";

// ============ model ============

export type ClusterView = DeckCluster & { cv: string };

/** A tree row with its DOM nodes and its lift geometry. */
export interface RowView extends Row {
  el: HTMLDivElement; nodeEl: HTMLDivElement; elbowEl?: HTMLDivElement; noteEl?: HTMLElement | null; foldEl?: HTMLButtonElement | null;
  liftEl?: HTMLDivElement; stepnoEl?: HTMLElement; cfoldEl?: HTMLButtonElement; tmainEl?: HTMLButtonElement; lidEl?: HTMLDivElement;
  tetherEl?: SVGGElement; tetherLine?: SVGPathElement; tetherDot?: SVGCircleElement;
  restX: number; upX: number; sinkUntil: number;
  sl?: HTMLButtonElement; sfn?: HTMLElement | null;
}

/** Everything the deck derives once from the graph and the flow. It never changes while the deck lives. */
export interface DeckModel {
  flow: DeckFlow | null;
  graph: DeckGraph;
  clusters: ClusterView[];
  CL: Map<string, ClusterView>;
  FN: Map<string, DeckFunction>;
  steps: DeckFlow["steps"];
  N: number;
  rows: RowView[];
  rowById: Map<string, RowView>;
  plan: FlowPlan;
  /** Calls between two members of each cluster. */
  internalOf: Map<string, DeckCall[]>;
  /** Calls to and from each function. */
  callsOf: Map<string, DeckCall[]>;
  fnsByFile: Map<string, DeckFunction[]>;
  clustersByFile: Map<string, Set<string>>;
  memberRows: RowView[];
  sideRows: RowView[];
  /** Widest lifted box of the flow. */
  maxLw: number;
  /** The cluster that lifts at a step, or null. */
  stepCluster(index: number): string | null;
}

/** Derives the deck model from a graph and a flow. */
export function buildModel(sourceGraph: DeckGraph, flow: DeckFlow | null): DeckModel {
  const graph = withMembership(sourceGraph, flow);
  const clusters: ClusterView[] = (flow?.clusters ?? []).map(cluster => ({ ...cluster, cv: colorVar(cluster.color) }));
  const CL = new Map(clusters.map(cluster => [cluster.id, cluster]));
  const FN = new Map<string, DeckFunction>(graph.functions.map(fn => [fn.id, fn]));
  const built = buildRows(graph);
  const rows = built.rows as RowView[];
  const rowById = built.rowById as Map<string, RowView>;
  for (const row of rows) { row.restX = 0; row.upX = 0; row.sinkUntil = 0; }
  const plan = flow ? buildPlan(flow, graph.calls, rowById) : EMPTY_PLAN;
  const internalOf = new Map<string, DeckCall[]>();
  for (const cluster of clusters) {
    const members = new Set(cluster.members);
    internalOf.set(cluster.id, graph.calls.filter(call => members.has(call.source) && members.has(call.target)));
  }
  const callsOf = new Map<string, DeckCall[]>();
  for (const call of graph.calls) for (const id of [call.source, call.target]) {
    if (!callsOf.has(id)) callsOf.set(id, []);
    callsOf.get(id)!.push(call);
  }
  const fnsByFile = new Map<string, DeckFunction[]>(), clustersByFile = new Map<string, Set<string>>();
  for (const fn of graph.functions) {
    if (!fnsByFile.has(fn.file)) fnsByFile.set(fn.file, []);
    fnsByFile.get(fn.file)!.push(fn);
    if (fn.cluster) {
      if (!clustersByFile.has(fn.file)) clustersByFile.set(fn.file, new Set());
      clustersByFile.get(fn.file)!.add(fn.cluster);
    }
  }
  const memberRows = rows.filter(row => row.kind === "fn" && row.fn!.cluster);
  return {
    flow, graph, clusters, CL, FN, steps: flow?.steps ?? [], N: flow?.steps.length ?? 0, rows, rowById, plan,
    internalOf, callsOf, fnsByFile, clustersByFile, memberRows,
    sideRows: rows.filter(row => row.kind !== "fn"),
    maxLw: memberRows.length ? Math.max(...memberRows.map(row => row.lw)) : 0,
    stepCluster: index => (flow ? effectiveCluster(flow, CL, index) : null),
  };
}

// ============ state ============

/** The mutable state of one deck. */
export interface DeckState {
  /** The current step; 0 is the rest state. */
  cur: number;
  /** The flow layout is active: set at flow start, cleared at clear. */
  flowOn: boolean;
  /** The cluster whose rows are ghost slots. */
  layoutCluster: string | null;
  /** The cluster whose boxes are up. */
  liftedCluster: string | null;
  annOpen: boolean;
  treeW: number;
  treeH: number;
  /** Open directories and files, each with its opener. Nothing here is saved, so each load starts collapsed. */
  open: Map<string, Opener>;
  /** Row id to y of the flow layout. A user fold sets a new baseline. */
  flowYs: Map<string, number> | null;
  /** The deck column. The flow layout sets it at flow start and after a user fold. */
  deckX: number;
  cam: Cam;
  view: View;
}

/** Returns the rest state of a new deck. */
export function initialState(model: DeckModel, annOpen: boolean): DeckState {
  return {
    cur: 0, flowOn: false, layoutCluster: null, liftedCluster: null, annOpen,
    treeW: 0, treeH: 0, open: restOpenState(model.rows), flowYs: null, deckX: 0,
    cam: { k: 1, tilt: 0, Fx: 0, Fy: 0 },
    view: viewOf(innerWidth, innerHeight, 262, 320, innerHeight - 150),
  };
}

// ============ context ============

/** Geometry tokens read from :root, so CSS faces and script geometry agree. */
export interface Tokens { tilt: number; deckZ: number; boxH: number; focusDz: number }

/** The deck modules. `deck.ts` fills this object after it creates every module. */
export interface DeckParts {
  viewport: Viewport; world: World; connectors: Connectors; sidebar: Sidebar; panels: Panels; controls: Controls; input: Input;
}

/** Options that the page gives a deck. */
export interface DeckOptions {
  graph: DeckGraph;
  flow: DeckFlow | null;
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
  tokens: Tokens;
  parts: DeckParts;
  /** Aborts every listener of the deck on teardown. */
  signal: AbortSignal;
  /** True when the viewer prefers reduced motion. */
  reducedMotion: boolean;
  /** A duration, or 0 with reduced motion. */
  D(ms: number): number;
  /** Runs a function after a delay. `clearTimers` cancels every pending one. */
  later(ms: number, fn: () => void): void;
  clearTimers(): void;
  /** Goes to a step. */
  go(step: number): void;
}

/** Reads the geometry tokens from :root. */
export function readTokens(): Tokens {
  const css = getComputedStyle(document.documentElement);
  const num = (name: string, fallback: number) => { const v = parseFloat(css.getPropertyValue(name)); return isNaN(v) ? fallback : v; };
  return { tilt: num("--tilt", 15), deckZ: num("--deck-z", 170), boxH: num("--box-h", 28), focusDz: num("--focus-dz", 30) };
}

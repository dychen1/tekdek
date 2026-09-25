/* Deck tree rows and layout. This module owns row identity, widths, open state rules, and row positions.
   Inputs: deck graph with flow membership, the flow plan, and the open state. Outputs: row metrics.
   The module never reads or writes the DOM.
   # Sections
   geometry, rows, open state, layout, deck rectangle */
import type { DeckFunction, DeckGraph } from "../data/models.ts";
import type { FlowPlan } from "./flow-plan.ts";

// ============ geometry ============

export const INDENT = 26, PAD = 30, GH = 32, DECK_IN = 26;
/** A flow row holds one box depth and a gap, so boxes never overlap on a deck. Its node is centered on the box. */
export const FLOW_PITCH = GH + 8, FLOW_H = 22, FLOW_TOP = (GH - FLOW_H) / 2;
export const MODES = ["full", "named", "stub", "ghost", "hidden"] as const;
export type RowMode = typeof MODES[number];
/** Longest shown name of a function row outside every flow cluster, in characters. */
export const NAME_CAP = 40;
/** JetBrains Mono advances 0.6 em per character, so text widths are exact. */
export const mono = (n: number, px: number) => n * 0.6 * px;

// ============ rows ============

export interface Row {
  kind: "dir" | "file" | "fn"; id: string; name: string; depth: number; x: number;
  parent: Row | null; children: Row[]; dirs: Row[];
  file?: string; fn?: DeckFunction; note?: string;
  /** Widths: `w` open, `wClosed` closed directory, `lw` lifted box, `gw` ghost slot. */
  w: number; wClosed: number; lw: number; gw: number;
  /** Layout output. */
  y: number; h: number; top: number; mode: RowMode; buried: boolean; cw: number; blockEnd: number;
}

/** Builds the rows in tree order: directories, files, and the functions of each file. */
export function buildRows(graph: DeckGraph): { rows: Row[]; rowById: Map<string, Row> } {
  const rows: Row[] = [], rowById = new Map<string, Row>();
  const fnsByFile = new Map<string, DeckFunction[]>();
  for (const fn of graph.functions) {
    if (!fnsByFile.has(fn.file)) fnsByFile.set(fn.file, []);
    fnsByFile.get(fn.file)!.push(fn);
  }
  const add = (part: Pick<Row, "kind" | "id" | "name" | "depth" | "parent"> & Partial<Row>) => {
    const row: Row = { children: [], dirs: [], x: PAD + part.depth * INDENT, w: 0, wClosed: 0, lw: 0, gw: 0,
      y: 0, h: 0, top: 0, mode: "full", buried: false, cw: 0, blockEnd: 0, ...part };
    if (row.parent) row.parent.children.push(row);
    rows.push(row); rowById.set(row.id, row);
    return row;
  };
  const walk = (tree: DeckGraph["tree"], path: string, depth: number, parent: Row | null) => {
    for (const key of Object.keys(tree)) {
      if (key.endsWith("/")) {
        const row = add({ kind: "dir", id: "dir:" + path + key, name: key, depth, parent });
        walk(tree[key] ?? {}, path + key, depth + 1, row);
      } else {
        const file = path + key;
        const row = add({ kind: "file", id: "file:" + file, name: key, file, depth, parent });
        for (const fn of fnsByFile.get(file) ?? []) add({ kind: "fn", id: fn.id, name: fn.name, fn, file, depth: depth + 1, parent: row });
      }
    }
  };
  walk(graph.tree, "", 0, null);
  // r.dirs: the directories above a row, root first.
  for (const row of rows) for (let a = row.parent; a; a = a.parent) if (a.kind === "dir") row.dirs.unshift(a);
  const counts = new Map<Row, { files: number; fns: number }>();
  for (const row of rows) for (const dir of row.dirs) {
    const count = counts.get(dir) ?? { files: 0, fns: 0 };
    if (row.kind === "file") count.files++;
    if (row.kind === "fn") count.fns++;
    counts.set(dir, count);
  }
  for (const row of rows) {
    if (row.kind === "dir") {
      const count = counts.get(row) ?? { files: 0, fns: 0 };
      row.note = `${count.files} ${count.files === 1 ? "file" : "files"} · ${count.fns} fn`;
      row.w = Math.round(47 + mono(row.name.length, 10.5));
      row.wClosed = row.w + Math.round(8 + mono(row.note.length, 10.5));
    } else if (row.kind === "file") {
      row.w = Math.round(34 + mono(row.name.length, 11));
    } else {
      // A row outside every flow cluster shows at most NAME_CAP characters; the box details show the full name.
      row.w = Math.round(20 + (row.fn!.cluster ? 12 : 0) + mono(row.fn!.cluster ? row.name.length : Math.min(row.name.length, NAME_CAP), 10.5));
      if (row.fn!.cluster) {
        row.lw = Math.round(54 + mono(row.name.length, 11.5));
        row.gw = Math.round(26 + mono(row.name.length, 11));
      }
    }
  }
  return { rows, rowById };
}

// ============ open state ============

/** Who opened a directory or a file: the user (the root by default, or a click) or the flow start. */
export type Opener = "user" | "flow";

/** At rest, a lone root directory is open. Every other directory and every file is closed.
 *  A codebase with several top-level entries starts with all of them closed, so the stack stays short. */
export function restOpenState(rows: readonly Row[]): Map<string, Opener> {
  const roots = rows.filter(row => !row.parent);
  return new Map(roots.length === 1 && roots[0].kind === "dir" ? [[roots[0].id, "user" as Opener]] : []);
}

/** The flow start opens the path to every named row once. A row that the user opened stays a user choice. */
export function openFlowPath(open: Map<string, Opener>, plan: FlowPlan) {
  for (const row of plan.openRows) if (!open.has(row.id)) open.set(row.id, "flow");
}

/** The flow end closes what the flow start opened. User choices stay. */
export function closeFlowPath(open: Map<string, Opener>) {
  for (const [id, by] of open) if (by === "flow") open.delete(id);
}

/** A chevron click toggles one directory or file as a user choice. */
export function toggleOpen(open: Map<string, Opener>, row: Row) {
  if (open.has(row.id)) open.delete(row.id); else open.set(row.id, "user");
}

/** A sidebar click opens the path to a row, and the row itself when it is a directory or a file.
 *  Returns true when the open state changes. */
export function openPath(open: Map<string, Opener>, row: Row): boolean {
  let changed = false;
  for (const target of row.kind === "fn" ? row.dirs : [...row.dirs, row]) {
    if (open.get(target.id) === "user") continue;
    open.set(target.id, "user"); changed = true;
  }
  return changed;
}

// ============ layout ============

export interface LayoutInput {
  /** True while a flow is on a step. The flow layout is the same for every step. */
  flow: boolean;
  /** The cluster whose rows are ghost slots, or null. */
  layoutCluster: string | null;
  open: ReadonlyMap<string, Opener>;
  plan: FlowPlan;
}

interface Metric { pitch: number; h: number; top?: number; mode: RowMode }

/** Returns the metrics of one row. Order matters: closed directories hide rows off the flow path first,
 *  then flow rows take their fixed slot, then closed files hide the other function rows. */
function metrics(row: Row, input: LayoutInput): Metric {
  if (row.buried) return { pitch: 0, h: 0, mode: "hidden" };
  if (row.kind === "dir") return input.flow ? { pitch: 20, h: 16, mode: "full" } : { pitch: 22, h: 18, mode: "full" };
  if (row.kind === "file") return { pitch: 26, h: 20, mode: "full" };
  if (input.flow && input.plan.flowFns.has(row.id)) {
    return { pitch: FLOW_PITCH, h: FLOW_H, top: FLOW_TOP, mode: row.fn!.cluster && row.fn!.cluster === input.layoutCluster ? "ghost" : "named" };
  }
  if (!input.open.has(row.parent!.id)) return { pitch: 0, h: 0, mode: "hidden" };
  if (!input.flow) return { pitch: 20, h: 16, mode: "full" };
  if (input.plan.flowNear.has(row.id)) return { pitch: 21, h: 16, mode: "named" };
  return { pitch: 7, h: 3, mode: "stub" };
}

/** A row is buried when a closed directory holds it and it is not on the flow path. */
function buried(row: Row, input: LayoutInput): boolean {
  const pin = input.flow ? input.plan.pinned : null;
  if (pin && (pin.has(row) || (row.kind === "fn" && pin.has(row.parent!)))) return false;
  return row.dirs.some(dir => !input.open.has(dir.id));
}

/** Lays out every row from the top of the tree. Returns the tree height.
 *  A function row in a closed file is hidden unless it is a flow row. */
export function layoutRows(rows: readonly Row[], input: LayoutInput): number {
  let y = PAD + 8;
  for (const row of rows) {
    row.buried = buried(row, input);
    const m = metrics(row, input);
    row.y = y; row.h = m.h; row.top = m.top ?? 0; row.mode = m.mode; y += m.pitch;
  }
  const treeH = y + PAD;
  for (const row of rows) {
    row.cw = row.mode === "ghost" ? row.gw
      : row.mode === "stub" ? Math.max(22, Math.round(row.w * 0.55))
        : row.kind === "file" && !input.open.has(row.id) ? row.w + 70
          : row.kind === "dir" && !input.open.has(row.id) ? row.wClosed : row.w;
  }
  const side = rows.filter(row => row.kind !== "fn");
  for (let i = 0; i < side.length; i++) side[i].blockEnd = i + 1 < side.length ? side[i + 1].y : treeH;
  return treeH;
}

/** Returns the elbow of a row: it joins the node center to the last shown sibling above, or to the parent's bottom edge. */
export function elbow(row: Row): { top: number; height: number } | null {
  const parent = row.parent;
  if (!parent) return null;
  const index = parent.children.indexOf(row);
  let sibling: Row | null = null;
  for (let i = index - 1; i >= 0; i--) if (!parent.children[i].buried) { sibling = parent.children[i]; break; }
  const anchor = sibling ? sibling.y + sibling.top + sibling.h / 2 : parent.y + parent.top + parent.h;
  const mid = row.y + row.top + row.h / 2;
  return { top: anchor - row.y, height: row.buried ? 0 : Math.max(0, mid - anchor) };
}

/** Returns the note of a file row: its function count at rest, or its folded count during a flow. */
export function fileNote(row: Row, input: LayoutInput): string {
  if (input.open.has(row.id)) return "";
  const fns = row.children.filter(child => !(input.flow && input.plan.flowFns.has(child.id))).length;
  if (!fns) return "";
  return input.flow ? `${fns} folded` : `${fns} fn`;
}

// ============ deck rectangle ============

/** Returns the deck column: right of every shown row in the files that hold cluster members.
 *  A member row counts with its wider mode (named or ghost), so the column does not change between clusters. */
export function deckColumn(rows: readonly Row[]): number {
  const memberFiles = new Set(rows.filter(row => row.kind === "fn" && row.fn!.cluster).map(row => row.file));
  const edges = rows.filter(row => row.mode !== "hidden" && row.file !== undefined && memberFiles.has(row.file))
    .map(row => row.x + (row.kind === "fn" && row.fn!.cluster ? Math.max(row.gw, row.w) : row.cw));
  return (edges.length ? Math.max(...edges) : PAD + 240) + 48;
}

/** Space from the deck's top edge to the first member row: a 36 px title band and the focus box rise. */
export const DECK_TOP = 62;

/** The deck spans the member slots, with a title band above the first box. */
export function deckRect(members: readonly Row[], deckX: number): { x: number; y: number; w: number; h: number } {
  const x1 = deckX + DECK_IN + Math.max(250, ...members.map(row => row.lw)) + 64;
  // The title band sits clear of the focus box, which floats up to 25 px above its row with its step badge.
  const y0 = Math.min(...members.map(row => row.y)) - DECK_TOP;
  const y1 = Math.max(...members.map(row => row.y + GH)) + 26;
  return { x: deckX, y: y0, w: x1 - deckX, h: y1 - y0 };
}

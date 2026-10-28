/* File box rows. This module owns the rows of the current file's box for one step: mode, place, and width.
   Inputs: the file's functions in source order, the flow visits, and the step. It never reads the DOM. */
import type { DeckFunction } from "../data/models.ts";
import { FS_CODE, mono, plural } from "./util.ts";

/** Row height and pitch for each kind of row. */
const ROW = { flow: [20, 26], named: [16, 22], stub: [3, 7] } as const;
/** Longest shown name of a row outside the flow, in characters. */
const NAME_CAP = 30;
/** The first row sits below the file node. */
const FILE_TOP = 28;
/** The narrowest node of a flow row: its mark and about three characters before the ellipsis. */
const MIN_FLOW_NODE = 48;

export interface FileRow {
  key: string; name: string; short: string; steps: number[]; cluster: string | null;
  /** slot: a visited member of the current cluster. named: a flow row of another cluster, or any row when compact is off. stub: off the flow while compact is on. */
  mode: "slot" | "named" | "stub";
  flow: boolean; current: boolean; y: number; h: number; w: number;
  /** The narrowest node width: in less room the name ends in an ellipsis. */
  min: number;
  /** The right edge of the row and its step pills, from the box's left edge. */
  right: number;
}

export interface FileBoxInput {
  fns: readonly DeckFunction[];
  visits: ReadonlyMap<string, readonly number[]>;
  /** Null at rest. */
  currentCluster: string | null;
  currentKey: string | null;
  compact: boolean;
  folded: boolean;
}

export interface FileBox {
  rows: FileRow[]; total: number; offFlow: number; height: number;
  /** The right edge of the widest row or the file node, from the box's left edge. */
  right: number;
  /** The same edge when every node is at its narrowest width. */
  min: number;
}

/** Lays out the rows of one file for one step. */
export function fileBox(input: FileBoxInput): FileBox {
  const rows: FileRow[] = [];
  // the file node: its border and padding, the fold button, and the function count in bold code text
  const head = 31 + mono(plural(input.fns.length, "function").length, FS_CODE);
  let y = FILE_TOP, height = 20, right = head, min = head, offFlow = 0;
  for (const fn of input.fns) {
    const steps = [...(input.visits.get(fn.id) ?? [])];
    const flow = steps.length > 0;
    if (!flow) offFlow++;
    if (!flow && input.folded) continue;
    const mode = flow && input.currentCluster !== null && fn.cluster === input.currentCluster ? "slot"
      : !flow && input.compact && input.currentCluster !== null ? "stub" : "named";
    const [h, pitch] = flow ? ROW.flow : ROW[mode === "stub" ? "stub" : "named"];
    const short = fn.name.length > NAME_CAP ? fn.name.slice(0, NAME_CAP - 1) + "…" : fn.name;
    const current = fn.id === input.currentKey;
    // one pill for each step, 6 px apart
    const pill = flow && !current ? steps.reduce((sum, i) => sum + 6 + Math.max(22, 12 + mono(String(i).length, FS_CODE)), 0) : 0;
    const w = mode === "slot" ? Math.round(26 + mono(Math.min(fn.name.length, 40), FS_CODE))
      : mode === "stub" ? Math.max(22, Math.round((20 + mono(Math.min(fn.name.length, 40), FS_CODE)) * 0.55))
        : Math.round(16 + (flow ? 12 : 0) + mono(short.length, FS_CODE));
    const row: FileRow = { key: fn.id, name: fn.name, short, steps, cluster: fn.cluster, mode, flow, current, y, h, w, min: Math.min(w, flow ? MIN_FLOW_NODE : 22), right: 26 + w + pill };
    rows.push(row);
    right = Math.max(right, row.right);
    min = Math.max(min, 26 + row.min + pill);
    height = y + h;
    y += pitch;
  }
  return { rows, total: input.fns.length, offFlow, height, right, min };
}

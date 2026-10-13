/* File box rows. This module owns the rows of the current file's box for one step: mode, place, and width.
   Inputs: the file's functions in source order, the flow visits, and the step. It never reads the DOM. */
import type { DeckFunction } from "../data/models.ts";
import { mono } from "./util.ts";

/** Row height and pitch for each kind of row. */
const ROW = { flow: [20, 26], named: [16, 22], stub: [3, 7] } as const;
/** Longest shown name of a row outside the flow, in characters. */
const NAME_CAP = 30;
/** The first row sits below the file node. */
const FILE_TOP = 28;

export interface FileRow {
  key: string; name: string; short: string; steps: number[]; cluster: string | null;
  /** slot: a visited member of the current cluster. named: a flow row of another cluster, or any row when compact is off. stub: off the flow while compact is on. */
  mode: "slot" | "named" | "stub";
  flow: boolean; current: boolean; y: number; h: number; w: number;
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

export interface FileBox { rows: FileRow[]; total: number; offFlow: number; height: number; right: number }

/** Lays out the rows of one file for one step. */
export function fileBox(input: FileBoxInput): FileBox {
  const rows: FileRow[] = [];
  let y = FILE_TOP, height = 20, right = 0, offFlow = 0;
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
    const pill = flow && !current ? 6 + Math.max(22, 12 + mono(steps.join(" ").length, 10.5)) : 0;
    const w = mode === "slot" ? Math.round(26 + mono(Math.min(fn.name.length, 40), 11))
      : mode === "stub" ? Math.max(22, Math.round((20 + mono(Math.min(fn.name.length, 40), 10.5)) * 0.55))
        : Math.round(16 + (flow ? 12 : 0) + mono(short.length, 10.5));
    const row: FileRow = { key: fn.id, name: fn.name, short, steps, cluster: fn.cluster, mode, flow, current, y, h, w, right: 26 + w + pill };
    rows.push(row);
    right = Math.max(right, row.right);
    height = y + h;
    y += pitch;
  }
  return { rows, total: input.fns.length, offFlow, height, right };
}

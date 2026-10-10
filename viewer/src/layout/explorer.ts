/* Explorer rows. This module owns the rows of the code explorer: directories and files in tree order,
   their tree guides, and the open state rules. It never reads the DOM.
   # Sections
   rows, guides, open state */
import type { DeckTree } from "../data/models.ts";
import { plural } from "./util.ts";

// ============ rows ============

/** One explorer row. A directory path ends with "/". `files` holds every file at or under the row. */
export interface ExplorerRow {
  kind: "dir" | "file"; path: string; name: string; depth: number;
  parent: ExplorerRow | null; last: boolean; files: string[];
}

/** Builds the rows in tree order. The tree already lists files before subdirectories, each in name order. */
export function buildExplorer(tree: DeckTree): ExplorerRow[] {
  const rows: ExplorerRow[] = [];
  const walk = (node: DeckTree, path: string, parent: ExplorerRow | null, depth: number): string[] => {
    const keys = Object.keys(node), files: string[] = [];
    keys.forEach((key, i) => {
      const kind = key.endsWith("/") ? "dir" : "file";
      const row: ExplorerRow = { kind, path: path + key, name: key, depth, parent, last: i === keys.length - 1, files: [] };
      rows.push(row);
      row.files = kind === "dir" ? walk(node[key] ?? {}, row.path, row, depth + 1) : [row.path];
      files.push(...row.files);
    });
    return files;
  };
  walk(tree, "", null, 0);
  return rows;
}

// ============ guides ============

export type GuideKind = "pipe" | "gap" | "tee" | "end";

/** One cell per ancestor below the root (pipe or gap), then a tee or a rounded end for the row itself. */
export function guideKinds(row: ExplorerRow): GuideKind[] {
  if (!row.parent) return [];
  const kinds: GuideKind[] = [];
  for (let a = row.parent; a.parent; a = a.parent) kinds.unshift(a.last ? "gap" : "pipe");
  kinds.push(row.last ? "end" : "tee");
  return kinds;
}

// ============ open state ============

/** Opens every directory above a row. Returns true when the open state changes. */
export function openTo(open: Set<string>, row: ExplorerRow): boolean {
  let changed = false;
  for (let a = row.parent; a; a = a.parent) if (!open.has(a.path)) { open.add(a.path); changed = true; }
  return changed;
}

/** True when every directory above the row is open. */
export function isShown(row: ExplorerRow, open: ReadonlySet<string>): boolean {
  for (let a = row.parent; a; a = a.parent) if (!open.has(a.path)) return false;
  return true;
}

/** Returns the row, or its nearest shown directory when a closed directory holds it. */
export function anchorRow(row: ExplorerRow, open: ReadonlySet<string>): ExplorerRow {
  let r: ExplorerRow = row;
  while (r.parent && !isShown(r, open)) r = r.parent;
  return r;
}

/** Returns the note of a closed directory ("+3 files"), or "" for an open one or a file. */
export const dirNote = (row: ExplorerRow, open: ReadonlySet<string>) =>
  row.kind === "dir" && !open.has(row.path) ? `+${plural(row.files.length, "file")}` : "";

// Tests for the file box rows: modes, widths, compact, and folds.

import assert from "node:assert/strict";
import test from "node:test";

import type { DeckFunction } from "../src/data/models.ts";
import { fileBox, type FileBoxInput } from "../src/layout/file-box.ts";

const fn = (name: string, cluster: string | null): DeckFunction => ({ id: `src/a.rs::callable:${name}`, name, file: "src/a.rs", cluster });
const LONG = "a_function_name_that_runs_well_past_thirty_chars";
const fns = [fn("a", "c1"), fn("b", "c2"), fn("c", null), fn(LONG, null)];
const input = (over: Partial<FileBoxInput> = {}): FileBoxInput => ({
  fns,
  visits: new Map([[fns[0].id, [3]], [fns[1].id, [5, 9]]]),
  currentCluster: "c1", currentKey: fns[0].id, compact: true, folded: false, ...over,
});

test("current cluster rows are slots, other flow rows are named, and off-flow rows are stubs", () => {
  const box = fileBox(input());
  assert.deepEqual(box.rows.map(r => r.mode), ["slot", "named", "stub", "stub"]);
  assert.deepEqual(box.rows.map(r => r.y), [28, 54, 80, 87]);
  assert.equal(box.height, 90);
  assert.equal(box.total, 4);
  assert.equal(box.offFlow, 2);
});

test("the current slot has no step pill, and other flow rows make room for theirs", () => {
  const [a, b] = fileBox(input()).rows;
  assert.equal(a.current, true);
  assert.equal(a.right, 26 + a.w);
  assert.ok(b.right > 26 + b.w);
});

test("compact off names every row, cutting long names to 30 characters", () => {
  const rows = fileBox(input({ compact: false })).rows;
  assert.deepEqual(rows.map(r => r.mode), ["slot", "named", "named", "named"]);
  assert.equal(rows[3].short.length, 30);
  assert.ok(rows[3].short.endsWith("…"));
});

test("a fold hides the off-flow rows and counts them", () => {
  const box = fileBox(input({ folded: true }));
  assert.deepEqual(box.rows.map(r => r.name), ["a", "b"]);
  assert.equal(box.offFlow, 2);
});

test("at rest every row is named and nothing is a slot", () => {
  const box = fileBox(input({ currentCluster: null, currentKey: null, compact: false }));
  assert.deepEqual(box.rows.map(r => r.mode), ["named", "named", "named", "named"]);
});

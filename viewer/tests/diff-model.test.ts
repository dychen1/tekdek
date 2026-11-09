import assert from "node:assert/strict";
import { test } from "node:test";

import type { DiffArtifact, DiffRow, FlowArtifact } from "../src/data/models.ts";
import { validRow } from "../src/data/load.ts";
import { indentOf } from "../src/layout/util.ts";
import { buildDiffModel, movedText, numberLines, slotsOf } from "../src/diff/model.ts";

/** A small frozen flow: each step is `[id, cluster key]`, and each cluster label is its key in capitals. */
function flow(steps: [string, string][]): FlowArtifact {
  const keys = [...new Set(steps.map(([, cluster]) => cluster))];
  return {
    kind: "tekdek.flow", schemaVersion: 4, flowId: "demo", generation: { ordinal: 1 },
    facts: { binding: { graphDigest: "sha256:0" }, nodes: [] },
    intent: {
      clusters: keys.map(key => ({ key, label: key.toUpperCase(), phase: "transform", phaseNote: null, input: [], output: [], rules: ["A test rule."] })),
      steps: steps.map(([id, clusterKey]) => ({ id, nodeKey: `src/a.rs::callable:${id}`, clusterKey, title: `Step ${id}`, ledger: { takes: null, requires: null, does: ["Work."], onFail: null, returns: null, writes: null } })),
    },
    assignment: { members: [] },
    view: { clusters: keys.map(clusterKey => ({ clusterKey, color: "cyan" })), steps: [] },
  };
}

const row = (kind: DiffRow["kind"], beforeStep: string | null, afterStep: string | null, added = 0, removed = 0, path = "src/a.rs"): DiffRow => ({
  kind, beforeStep, afterStep, added, removed, lines: [],
  baseSpan: beforeStep ? { path: kind === "moved" ? "src/old.rs" : path, start: 1, end: 9 } : null,
  headSpan: afterStep ? { path, start: 1, end: 9 } : null,
});

/** Before: route b1 b2, write b3 b4, route b5. After: route a1 a2, open a3, write a4, route a5 a6. */
const fixture: DiffArtifact = {
  kind: "tekdek.diff", schemaVersion: 1, name: "demo",
  base: { commit: "aaa", flow: flow([["b1", "route"], ["b2", "route"], ["b3", "write"], ["b4", "write"], ["b5", "route"]]) },
  head: { commit: "bbb", flow: flow([["a1", "route"], ["a2", "route"], ["a3", "open"], ["a4", "write"], ["a5", "route"], ["a6", "route"]]) },
  pairs: [{ before: "b1", after: "a1" }, { before: "b2", after: "a2" }, { before: "b3", after: "a4" }, { before: "b5", after: "a5" }],
  notes: [{ side: "after", clusterKey: "write", text: "writes in one transaction" }],
  rows: [
    row("unchanged", "b1", "a1"), row("changed", "b2", "a2", 4, 2), row("new", null, "a3", 10), row("moved", "b3", "a4", 1, 1),
    row("unchanged", "b5", "a5"), row("changed", null, "a6", 4, 2), row("removed", "b4", null, 0, 7, "src/b.rs"),
  ],
};

test("the totals count After kinds and removed Before steps", () => {
  assert.deepEqual(buildDiffModel(fixture).totals, { new: 1, removed: 1, changed: 2, moved: 1 });
});

test("runs with the same label share a row, and a repeated After label continues", () => {
  const model = buildDiffModel(fixture);
  assert.deepEqual(model.rows.map(r => [r.before?.label ?? null, r.after?.label ?? null, r.continues]), [
    ["ROUTE", "ROUTE", false], [null, "OPEN", false], ["WRITE", "WRITE", false], ["ROUTE", "ROUTE", true],
  ]);
});

test("a row counts its changed steps and its lines, and shows nothing without a change", () => {
  const [route, open, write, again] = buildDiffModel(fixture).rows;
  assert.deepEqual([route!.counts, route!.added, route!.removed], [{ new: 0, removed: 0, changed: 1, moved: 0 }, 4, 2]);
  assert.deepEqual([open!.counts.new, open!.added], [1, 10]);
  assert.deepEqual([write!.counts, write!.added, write!.removed], [{ new: 0, removed: 1, changed: 0, moved: 1 }, 1, 8]);
  // a6 shows the same function span as a2 does in another row, so it counts in its own row
  assert.equal(again!.changed, true);
  assert.equal(write!.notes.after, "writes in one transaction");
  assert.equal(route!.notes.after, null);
});

test("a step that shows the same function twice in one row counts its lines once", () => {
  const twice: DiffArtifact = { ...fixture, rows: fixture.rows.map(r => (r.afterStep === "a1" ? row("changed", "b1", "a1", 4, 2) : r)) };
  const route = buildDiffModel(twice).rows[0]!;
  assert.deepEqual([route.counts.changed, route.added, route.removed], [2, 4, 2]);
});

test("a row opens the deck at its first changed After step", () => {
  const model = buildDiffModel(fixture);
  assert.deepEqual(model.rows.map(r => r.entry), [2, 3, 4, 6]);
  assert.equal(model.rowOfAfter(4), 2);
});

test("an open row matches Before and After steps by pair, with an empty slot for a removed step", () => {
  const model = buildDiffModel(fixture);
  const slots = slotsOf(model, model.rows[2]!).map(s => [s.before?.id ?? null, s.after?.id ?? null]);
  assert.deepEqual(slots, [["b3", "a4"], ["b4", null]]);
  assert.equal(model.before[2]!.kind, "moved");
  assert.equal(movedText(model.after[3]!.row), "moved from src/old.rs to src/a.rs");
});

test("diff lines count from the spans: context moves both, removed the old, added the new", () => {
  const lines = numberLines({
    ...row("changed", "b2", "a2"), baseSpan: { path: "a", start: 10, end: 13 }, headSpan: { path: "a", start: 20, end: 23 },
    lines: [{ op: "context", text: "" }, { op: "removed", text: "" }, { op: "removed", text: "" }, { op: "added", text: "" }, { op: "context", text: "" }],
  });
  assert.deepEqual(lines.map(l => l.no), [20, 11, 12, 21, 22]);
});

test("a diff row with a kind, count, or line op outside the schema is rejected", () => {
  const row: DiffRow = {
    kind: "changed", beforeStep: "a", afterStep: "a", baseSpan: { path: "a.rs", start: 1, end: 2 }, headSpan: null,
    added: 1, removed: 0, lines: [{ op: "added", text: "<b>x</b>" }],
  };
  assert.equal(validRow(row), true);
  assert.equal(validRow({ ...row, kind: 'x" onmouseover="alert(1)' as DiffRow["kind"] }), false);
  assert.equal(validRow({ ...row, added: "<img>" as unknown as number }), false);
  assert.equal(validRow({ ...row, baseSpan: { path: "a.rs", start: "<i>" as unknown as number, end: 2 } }), false);
  assert.equal(validRow({ ...row, lines: [{ op: "<s>" as "added", text: "" }] }), false);
});

test("the indent of a code line counts spaces and tab stops of 8", () => {
  assert.equal(indentOf("    let a = 1;"), 4);
  assert.equal(indentOf("\tx"), 8);
  assert.equal(indentOf("  \tx"), 8);
  assert.equal(indentOf("x"), 0);
});

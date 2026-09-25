// Tests for the tree layout and the flow plan: rest state, the fixed flow layout, open state, and the deck rectangle.

import assert from "node:assert/strict";
import test from "node:test";

import { deckFlow, deckGraph, withMembership } from "../src/data/adapt.ts";
import { buildPlan, dockOrder, effectiveCluster, nearestStep, rangesOf } from "../src/layout/flow-plan.ts";
import type { FlowPlan } from "../src/layout/flow-plan.ts";
import { buildRows, closeFlowPath, deckColumn, deckRect, layoutRows, NAME_CAP, openFlowPath, openPath, restOpenState, toggleOpen } from "../src/layout/tree.ts";
import type { Row } from "../src/layout/tree.ts";

import { artifact, graphDocument, must, smallGraph } from "./fixtures.ts";

function setup() {
  const base = deckGraph(graphDocument(), "tekdek");
  const flow = deckFlow(artifact, base, artifact.facts.binding.graphDigest);
  const graph = withMembership(base, flow);
  const { rows, rowById } = buildRows(graph);
  const plan = buildPlan(flow, graph.calls, rowById);
  const clusters = new Map(flow.clusters.map(cluster => [cluster.id, cluster]));
  return { graph, flow, rows, rowById, plan, clusters };
}

/** The flow layout at flow start, with the path to the flow rows open. */
function flowLayout({ rows, plan }: { rows: Row[]; plan: FlowPlan }, layoutCluster: string | null = null) {
  const open = restOpenState(rows);
  openFlowPath(open, plan);
  layoutRows(rows, { flow: true, layoutCluster, open, plan });
  return open;
}

test("at rest only a lone root directory is open, so every file and deeper directory is closed", () => {
  const { rows, plan } = setup();
  const open = restOpenState(rows);
  assert.deepEqual([...open.keys()], ["dir:src/"]);
  layoutRows(rows, { flow: false, layoutCluster: null, open, plan });
  assert.ok(rows.filter(row => row.kind === "fn").every(row => row.mode === "hidden"), "closed files hide their functions");
  assert.ok(rows.filter(row => row.kind === "file" && row.dirs.length === 1).every(row => row.mode === "full"));
});

test("a codebase with several top-level entries starts with every directory closed", () => {
  const { rows } = buildRows(deckGraph(smallGraph(["main.py", "src/a.py", "tests/t.py"]), "multi"));
  assert.equal(restOpenState(rows).size, 0);
});

test("no row moves between flow steps, whichever cluster is lifted", () => {
  const { rows, plan, flow, clusters } = setup();
  const open = restOpenState(rows);
  openFlowPath(open, plan);
  const baseline = new Map<string, number>();
  for (const step of flow.steps) {
    const cluster = effectiveCluster(flow, clusters, step.index);
    layoutRows(rows, { flow: true, layoutCluster: cluster, open, plan });
    for (const row of rows) {
      if (!baseline.has(row.id)) baseline.set(row.id, row.y);
      else assert.equal(row.y, baseline.get(row.id), `${row.id} moved at step ${step.index}`);
    }
    for (const id of must(clusters.get(must(cluster, "cluster")), "cluster").members) assert.equal(must(rows.find(row => row.id === id), id).mode, "ghost");
  }
});

test("every function a flow needs keeps a named row, and call neighbors keep plain names", () => {
  const context = setup();
  flowLayout(context);
  for (const id of context.plan.flowFns) assert.equal(must(context.rows.find(row => row.id === id), id).mode, "named", id);
  for (const id of context.plan.flowNear) assert.equal(must(context.rows.find(row => row.id === id), id).mode, "named", id);
});

test("flow rows stay shown in a file that the user closes during the flow", () => {
  const { rows, rowById, plan } = setup();
  const open = restOpenState(rows);
  openFlowPath(open, plan);
  const main = must(rowById.get("src/main.rs::callable:main"), "main row");
  toggleOpen(open, must(main.parent, "main file"));
  layoutRows(rows, { flow: true, layoutCluster: null, open, plan });
  assert.equal(main.mode, "named");
});

test("clear closes what the flow opened and keeps what the user opened", () => {
  const { rows, rowById, plan } = setup();
  const open = restOpenState(rows);
  const userFile = must(rowById.get("file:src/extra.rs"), "extra.rs row");
  openPath(open, userFile);
  openFlowPath(open, plan);
  assert.ok([...open.values()].includes("flow"));
  closeFlowPath(open);
  assert.ok(![...open.values()].includes("flow"));
  assert.equal(open.get(userFile.id), "user");
});

test("step ranges, stack order, and the nearest visit follow the frozen runs", () => {
  const { plan, clusters } = setup();
  const command = "src/cli/flow.rs::callable:import";
  assert.equal(rangesOf(plan, "route-command"), "1–6, 21");
  assert.equal(dockOrder(plan, must(clusters.get("route-command")), 2)[0], "src/main.rs::callable:main");
  assert.equal(nearestStep(plan, command, 6), 5, "the visit in the current pass wins");
  assert.equal(nearestStep(plan, command, 20), 21);
});

test("the deck spans its member slots below a title band", () => {
  const context = setup();
  flowLayout(context, "check-output");
  const members = must(context.clusters.get("check-output")).members.map(id => must(context.rowById.get(id), id));
  const rect = deckRect(members, deckColumn(context.rows));
  assert.ok(rect.y < Math.min(...members.map(row => row.y)));
  assert.ok(rect.y + rect.h > Math.max(...members.map(row => row.y)));
});

test("the deck column sits right of the shown rows only", () => {
  const context = setup();
  flowLayout(context);
  const column = deckColumn(context.rows);
  const shown = context.rows.filter(row => row.mode !== "hidden" && row.kind === "fn");
  assert.ok(shown.every(row => row.x + Math.max(row.gw, row.cw) + 48 <= column));
});

test("a long name outside every flow cluster is capped", () => {
  const { rows } = buildRows(deckGraph(smallGraph(["src/a.rs"], [["src/a.rs", "x".repeat(90)]]), "long"));
  assert.equal(must(rows.find(row => row.kind === "fn")).w, Math.round(20 + NAME_CAP * 0.6 * 10.5));
});

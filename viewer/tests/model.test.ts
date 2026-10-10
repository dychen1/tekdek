// Tests for the deck model: membership, derived maps, the rest state, and the file that the file box shows.

import assert from "node:assert/strict";
import test from "node:test";

import { deckFlow, deckGraph } from "../src/data/adapt.ts";
import type { DeckFlow } from "../src/data/models.ts";
import { isShown } from "../src/layout/explorer.ts";
import { MIN_DECK_W } from "../src/layout/deck-geometry.ts";
import { buildModel, initialState, shownFile, withMembership } from "../src/layout/model.ts";

import { artifact, DIGEST, graphDocument, must } from "./fixtures.ts";

const graph = deckGraph(graphDocument(), "demo");
const flow = deckFlow(artifact, graph, DIGEST);
const model = buildModel(graph, flow, new Map());

test("membership marks each function with the cluster that holds it", () => {
  const graph = deckGraph(graphDocument(), "tekdek");
  const marked = withMembership(graph, deckFlow(artifact, graph, DIGEST));
  assert.equal(marked.functions.filter(fn => fn.cluster).length, 21);
  assert.equal(must(marked.functions.find(fn => fn.id === "src/main.rs::callable:main")).cluster, "route-command");
  assert.equal(graph.functions.filter(fn => fn.cluster).length, 0, "the source graph stays unmarked");
});

test("members follow their first visit and the deck is as wide as the widest cluster needs", () => {
  for (const cluster of model.clusters) {
    const keys = must(model.members.get(cluster.id), cluster.id);
    const firsts = keys.map(key => model.plan.visits.get(key)?.[0] ?? Number.MAX_SAFE_INTEGER);
    assert.deepEqual(firsts, [...firsts].sort((a, b) => a - b));
  }
  assert.ok(model.deckSize.width >= MIN_DECK_W);
  assert.ok(model.deckSize.maxHeight > 0);
  assert.ok(model.fileRight > 0);
});

test("only flow files carry clusters, and the rest state opens the path to every flow file", () => {
  for (const file of model.clustersByFile.keys()) assert.ok(model.plan.flowFiles.has(file), file);
  const state = initialState(model);
  assert.equal(state.cur, 0);
  assert.equal(state.compact, true);
  assert.equal(state.zoom, 1);
  assert.equal(state.browseFile, null);
  for (const file of model.plan.flowFiles.keys()) assert.equal(isShown(must(model.rowByPath.get(file), file), state.open), true, file);
});

test("a codebase without a flow builds an empty plan", () => {
  const bare = buildModel(graph, null, new Map());
  assert.equal(bare.N, 0);
  assert.equal(bare.plan.flowFiles.size, 0);
  assert.equal(bare.fileRight, 0);
  assert.equal(bare.stepCluster(1), null);
  assert.equal(shownFile(bare, initialState(bare)), null);
});

test("the file box shows nothing at rest, the step file in a flow, and the browsed file over both", () => {
  const state = initialState(model);
  assert.equal(shownFile(model, state), null);
  state.cur = 1;
  const step = must(model.steps[0], "step 1");
  const shown = must(shownFile(model, state), "step file");
  assert.equal(shown.file, must(model.FN.get(step.node), step.node).file);
  assert.equal(shown.key, step.node);
  assert.equal(shown.cluster, model.stepCluster(1));
  state.browseFile = "src/elsewhere.rs";
  assert.deepEqual(shownFile(model, state), { file: "src/elsewhere.rs", cluster: null, key: null });
  state.cur = 0;
  assert.deepEqual(shownFile(model, state), { file: "src/elsewhere.rs", cluster: null, key: null });
});

test("a step whose function the graph lost shows no file", () => {
  const lost: DeckFlow = { ...flow, steps: flow.steps.map((step, i) => (i === 0 ? { ...step, missing: true } : step)) };
  const broken = buildModel(graph, lost, new Map());
  const state = initialState(broken);
  state.cur = 1;
  assert.equal(shownFile(broken, state), null);
  assert.equal(broken.stepAt(1)?.missing, true);
  assert.equal(broken.fnAt(1), null);
});

test("the step and its function at an index, and neither at rest or past the end", () => {
  const step = must(model.stepAt(1), "step 1");
  assert.equal(step, model.steps[0]);
  assert.equal(must(model.fnAt(1), "step 1 function").id, step.node);
  for (const index of [0, model.N + 1]) {
    assert.equal(model.stepAt(index), null);
    assert.equal(model.fnAt(index), null);
  }
});

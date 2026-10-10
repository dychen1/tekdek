// Tests for the flow plan: visits, runs, flow files, member order, and file stepping.

import assert from "node:assert/strict";
import test from "node:test";

import { deckFlow, deckGraph } from "../src/data/adapt.ts";
import { buildPlan, memberOrder, nextStepOfFile, rangesOf } from "../src/layout/plan.ts";

import { artifact, DIGEST, graphDocument, must } from "./fixtures.ts";

const graph = deckGraph(graphDocument(), "demo");
const flow = deckFlow(artifact, graph, DIGEST);
const fileOf = (key: string) => graph.functions.find(fn => fn.id === key)?.file;
const plan = buildPlan(flow, fileOf);

test("the plan lists the steps of each flow file in order", () => {
  for (const [file, steps] of plan.flowFiles) {
    assert.deepEqual(steps, [...steps].sort((a, b) => a - b));
    for (const i of steps) assert.equal(fileOf(must(flow.steps[i - 1]).node), file);
  }
});

test("members follow their first visit, and a file click goes to its next step", () => {
  for (const cluster of flow.clusters) {
    const order = memberOrder(plan, cluster);
    const firsts = order.map(id => must(plan.visits.get(id), id)[0]);
    assert.deepEqual(firsts, [...firsts].sort((a, b) => a - b));
    assert.ok(rangesOf(plan, cluster.id).length > 0);
  }
  const [file, steps] = must([...plan.flowFiles][0]);
  assert.equal(nextStepOfFile(plan, file, 0), steps[0]);
  assert.equal(nextStepOfFile(plan, file, steps[steps.length - 1]), steps[0]);
  assert.equal(nextStepOfFile(plan, "src/none.rs", 0), null);
});

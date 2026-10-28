// Tests for the flow plan: visits, runs, flow files, member order, and file stepping.

import assert from "node:assert/strict";
import test from "node:test";

import { deckFlow, deckGraph } from "../src/data/adapt.ts";
import { buildPlan, memberOrder, nextRunStart, nextStepOfFile, prevRunStart, rangesOf } from "../src/layout/plan.ts";

import { artifact, DIGEST, graphDocument, must } from "./fixtures.ts";

const graph = deckGraph(graphDocument());
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
  const [file, s] = must([...plan.flowFiles].find(([, steps]) => steps.length >= 3), "file with three steps");
  assert.equal(nextStepOfFile(plan, file, 0), s[0]);
  assert.equal(nextStepOfFile(plan, file, s[0]), s[1]);
  assert.equal(nextStepOfFile(plan, file, s[1]), s[2]);
  assert.equal(nextStepOfFile(plan, file, s[s.length - 1]), s[0]);
  const gap = s.findIndex((step, k) => k + 1 < s.length && s[k + 1] > step + 1);
  if (gap >= 0) assert.equal(nextStepOfFile(plan, file, s[gap] + 1), s[gap + 1]);
  assert.equal(nextStepOfFile(plan, "src/none.rs", 0), null);
});

test("h and l go to the first step of the previous and the next section", () => {
  const starts = plan.runs.map(run => run.start);
  assert.ok(starts.length >= 2, "a flow with two sections");
  assert.equal(nextRunStart(plan, 0), starts[0]);
  assert.equal(prevRunStart(plan, 0), null);
  for (const [i, run] of plan.runs.entries()) {
    for (const step of [run.start, run.end]) {
      assert.equal(nextRunStart(plan, step), starts[i + 1] ?? null);
      assert.equal(prevRunStart(plan, step), i ? starts[i - 1] : null);
    }
  }
});

// Tests for the data layer: the graph digest check, load failures, the graph adapter, and the frozen flow adapter.

import assert from "node:assert/strict";
import test from "node:test";

import { deckFlow, deckGraph, displayName, fileTree, pickFlow, frozenFlows } from "../src/data/adapt.ts";
import { digestOf, loadFlow, loadIndex, loadSource, LoadError, verifyGraph } from "../src/data/load.ts";

import { artifact, DIGEST, graphDocument, must } from "./fixtures.ts";

const bytesOf = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).buffer;

test("the graph check accepts bytes whose digest matches the index", async () => {
  const bytes = bytesOf(graphDocument());
  const document = await verifyGraph(bytes, { file: "graph.json", digest: await digestOf(bytes) });
  assert.equal(document.schemaVersion, 1);
});

test("the graph check rejects changed bytes and unsupported documents with their cause", async () => {
  const bytes = bytesOf(graphDocument());
  await assert.rejects(verifyGraph(bytes, { file: "graph.json", digest: "sha256:other" }),
    (error) => error instanceof LoadError && error.reason === "digest");
  const wrong = bytesOf({ schemaVersion: 2, graph: { nodes: [], edges: [] } });
  await assert.rejects(verifyGraph(wrong, { file: "graph.json", digest: await digestOf(wrong) }),
    (error) => error instanceof LoadError && error.reason === "graph");
});

test("a load whose answer is not JSON fails with the cause of its file", async (t) => {
  t.mock.method(globalThis, "fetch", async () => new Response("{", { status: 200 }));
  await assert.rejects(loadFlow({ name: "a", file: "a/generations/1-000000000000.json" }),
    (error) => error instanceof LoadError && error.reason === "flow");
  await assert.rejects(loadIndex(), (error) => error instanceof LoadError && error.reason === "fetch");
  await assert.rejects(loadSource("demo.json", "a"), (error) => error instanceof LoadError && error.reason === "fetch");
});

test("a load that the server refuses or never answers is a fetch failure", async (t) => {
  const answer = t.mock.method(globalThis, "fetch", async () => new Response("{}", { status: 500 }));
  await assert.rejects(loadFlow({ name: "a", file: "a.json" }), (error) => error instanceof LoadError && error.reason === "fetch");
  await assert.rejects(loadSource("demo.json", "a"), (error) => error instanceof LoadError && error.reason === "fetch");
  answer.mock.mockImplementation(async () => { throw new TypeError("network down"); });
  await assert.rejects(loadIndex(), (error) => error instanceof LoadError && error.reason === "fetch");
});

test("method names keep their type and nested functions keep their parent", () => {
  assert.equal(displayName("src/a.rs::type:Store::callable:open", "open"), "Store::open");
  assert.equal(displayName("src/a.py::callable:outer::callable:inner", "inner"), "outer::inner");
  assert.equal(displayName("src/main.rs::callable:main", "main"), "main");
});

test("the file tree lists files before subdirectories, each in name order", () => {
  const tree = fileTree(["src/z.rs", "src/a/b.rs", "src/b.rs", "README.py"]);
  assert.deepEqual(Object.keys(tree), ["README.py", "src/"]);
  assert.deepEqual(Object.keys(must(tree["src/"])), ["b.rs", "z.rs", "a/"]);
});

test("the deck graph keeps distinct calls between two callables and drops self calls", () => {
  const graph = deckGraph(graphDocument(), "tekdek");
  const keys = graph.calls.map(call => `${call.source}>${call.target}`);
  assert.equal(new Set(keys).size, keys.length);
  assert.ok(graph.calls.every(call => call.source !== call.target));
  assert.ok(graph.files.includes("src/extra.rs"));
  assert.equal(must(graph.functions.find(fn => fn.id.endsWith("callable:open"))).name, "Store::open");
});

test("a frozen flow gives its clusters in intent order with frozen colors and assigned members", () => {
  const graph = deckGraph(graphDocument(), "tekdek");
  const flow = deckFlow(artifact, graph, DIGEST);
  assert.equal(flow.name, "flow-generation");
  assert.equal(flow.steps.length, 25);
  assert.deepEqual(flow.clusters.map(cluster => [cluster.id, cluster.color, cluster.members.length]),
    [["route-command", "cyan", 6], ["check-output", "red", 4], ["derive-layers", "yellow", 6], ["write-generation", "magenta", 2], ["refresh-catalog", "lime", 3]]);
  assert.equal(flow.stale, false);
  assert.ok(flow.steps.every(step => !step.missing));
});

test("a step whose function left the graph is marked missing and leaves the members", () => {
  const gone = must(artifact.intent.steps[0], "first step").nodeKey;
  const graph = deckGraph(graphDocument({ drop: [gone] }), "tekdek");
  const flow = deckFlow(artifact, graph, "sha256:other");
  assert.equal(must(flow.steps[0], "first step").missing, true);
  assert.ok(!flow.clusters.some(cluster => cluster.members.includes(gone)));
  assert.equal(flow.stale, true, "another graph digest makes the flow stale");
});

test("a file that is not a frozen schema 3 flow is rejected", () => {
  const graph = deckGraph(graphDocument(), "tekdek");
  assert.throws(() => deckFlow({ ...artifact, schemaVersion: 2 }, graph, DIGEST), /schema 3/);
});

test("only frozen schema 3 flows show, and the picker opens the requested flow, else the first one", () => {
  const flows = frozenFlows([{ name: "legacy", file: "legacy.json" }, { name: "a", file: "a/generations/1-000000000000.json", schemaVersion: 3 },
    { name: "b", file: "b/generations/1-000000000000.json", schemaVersion: 3 }]);
  assert.deepEqual(flows.map(flow => flow.name), ["a", "b"]);
  assert.equal(must(pickFlow(flows, "b")).name, "b");
  assert.equal(must(pickFlow(flows, "missing")).name, "a");
  assert.equal(pickFlow([], null), null);
  assert.deepEqual(frozenFlows(undefined), []);
});

test("the flow adapter carries the transition, the fact text, and the generation of each step", () => {
  const graph = deckGraph(graphDocument(), "demo");
  const flow = deckFlow(artifact, graph, DIGEST);
  assert.equal(flow.generation, artifact.generation.ordinal);
  assert.equal(must(flow.steps[0]).transition, "flow_start");
  const views = new Map(artifact.view.steps.map(view => [view.stepId, view.transition]));
  for (const step of flow.steps) assert.equal(step.transition, views.get(step.id) ?? null);
  const fact = must(artifact.facts.nodes.find(node => node.key === must(flow.steps[1]).node));
  assert.equal(must(flow.steps[1]).signature, fact.signature ?? null);
  assert.equal("behavior" in flow, false);
});

// Shared fixtures for the viewer tests: the demo flow artifact and a graph that holds its facts.

import { readFileSync } from "node:fs";

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const current = read("../../demo/flows/flow-generation/current.json");
export const artifact = read(`../../demo/flows/flow-generation/generations/${current.ordinal}-${current.generationDigest.slice(7, 19)}.json`);
export const DIGEST = artifact.facts.binding.graphDigest;

/** A graph document that holds the fact nodes of the fixture artifact, their modules, and their edges. */
export function graphDocument({ drop = [] } = {}) {
  const nodes = [], ids = new Map();
  const add = (node) => { const id = nodes.length; nodes.push({ id, declarations: [{ start: id * 10, end: id * 10 + 5 }], ...node }); ids.set(node.key, id); };
  for (const path of [...new Set(artifact.facts.nodes.map(node => node.path))]) add({ key: path, name: path, path, kind: "module" });
  for (const node of artifact.facts.nodes) if (!drop.includes(node.key)) add({ key: node.key, name: node.name, path: node.path, kind: node.kind });
  add({ key: "src/extra.rs::type:Store::callable:open", name: "open", path: "src/extra.rs", kind: "callable" });
  const edges = artifact.facts.edges.filter(edge => ids.has(edge.source) && ids.has(edge.target))
    .flatMap(edge => [edge, edge].map(e => ({ source: ids.get(e.source), target: ids.get(e.target), kind: e.kind, weight: 1 })));
  edges.push({ source: ids.get(artifact.facts.nodes[0].key), target: ids.get(artifact.facts.nodes[0].key), kind: "calls", weight: 1 });
  return { schemaVersion: 1, graph: { nodes, edges } };
}

/** A graph document with the given callables. Each callable key is `<path>::callable:<name>`. */
export function smallGraph(paths, callables = []) {
  const nodes = paths.map((path, id) => ({ id, key: path, name: path, path, kind: "module" }));
  for (const [path, name] of callables) nodes.push({ id: nodes.length, key: `${path}::callable:${name}`, name, path, kind: "callable" });
  return { schemaVersion: 1, graph: { nodes, edges: [] } };
}

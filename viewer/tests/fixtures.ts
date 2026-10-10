// Shared fixtures for the viewer tests: the demo flow artifact and a graph that holds its facts.

import { readFileSync } from "node:fs";

import type { FlowArtifact, GraphDocument, GraphEdge, GraphNode } from "../src/data/models.ts";

/** A fact node and a fact edge of the demo artifact, which the fixture graphs copy. */
interface FactNode { key: string; name: string; path: string; kind: string; signature?: string | null; documentation?: string | null }
interface FactEdge { source: string; target: string; kind: string }

/** The demo artifact, with the fact fields that the fixtures read. */
type DemoArtifact = Omit<FlowArtifact, "facts"> & { facts: { binding: { graphDigest: string }; nodes: FactNode[]; edges: FactEdge[] } };

const read = <T>(path: string): T => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8")) as T;
const current = read<{ ordinal: number; generationDigest: string }>("../../demo/flows/flow-generation/current.json");
export const artifact = read<DemoArtifact>(`../../demo/flows/flow-generation/generations/${current.ordinal}-${current.generationDigest.slice(7, 19)}.json`);
export const DIGEST = artifact.facts.binding.graphDigest;

/** Returns the value, or fails the test when the value is missing. */
export function must<T>(value: T | null | undefined, what = "value"): T {
  if (value === null || value === undefined) throw new Error(`missing ${what}`);
  return value;
}

/** A graph document that holds the fact nodes of the fixture artifact, their modules, and their edges. */
export function graphDocument({ drop = [] }: { drop?: string[] } = {}): GraphDocument {
  const nodes: GraphNode[] = [], ids = new Map<string, number>();
  const add = (node: Omit<GraphNode, "id">) => {
    const id = nodes.length;
    nodes.push({ id, declarations: [{ start: id * 10, end: id * 10 + 5 }], ...node });
    ids.set(node.key, id);
  };
  for (const path of new Set(artifact.facts.nodes.map(node => node.path))) add({ key: path, name: path, path, kind: "module" });
  for (const node of artifact.facts.nodes) if (!drop.includes(node.key)) add({ key: node.key, name: node.name, path: node.path, kind: node.kind });
  add({ key: "src/extra.rs::type:Store::callable:open", name: "open", path: "src/extra.rs", kind: "callable" });
  const id = (key: string) => must(ids.get(key), key);
  const edges: GraphEdge[] = artifact.facts.edges.filter(edge => ids.has(edge.source) && ids.has(edge.target))
    .flatMap(edge => [edge, edge].map(e => ({ source: id(e.source), target: id(e.target), kind: e.kind })));
  const first = must(artifact.facts.nodes[0], "first fact node").key;
  edges.push({ source: id(first), target: id(first), kind: "calls" });
  return { schemaVersion: 1, graph: { nodes, edges } };
}

/** A graph document with the given callables. Each callable key is `<path>::callable:<name>`. */
export function smallGraph(paths: string[], callables: [path: string, name: string][] = []): GraphDocument {
  const nodes: GraphNode[] = paths.map((path, id) => ({ id, key: path, name: path, path, kind: "module" }));
  for (const [path, name] of callables) nodes.push({ id: nodes.length, key: `${path}::callable:${name}`, name, path, kind: "callable" });
  return { schemaVersion: 1, graph: { nodes, edges: [] } };
}

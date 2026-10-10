/* Deck adapters. This module turns a verified graph document and a frozen flow artifact into deck data.
   Inputs: a schema 1 graph document and a schema 3 flow artifact. Outputs: DeckGraph and DeckFlow.
   The adapters read data only; they never fetch.
   # Sections
   graph, flow, shared rules */
import type { ClusterColor, DeckCall, DeckCluster, DeckFlow, DeckFunction, DeckGraph, DeckStep, DeckTree, FlowArtifact, FlowEntry, GraphDocument } from "./models.ts";

// ============ graph ============

/** Returns the display name of a callable: the key segments after the path, without kind prefixes. */
export function displayName(key: string, fallback: string): string {
  const parts = key.split("::").slice(1).map(part => part.slice(part.indexOf(":") + 1));
  return parts.length ? parts.join("::") : fallback;
}

/** Builds the deck graph from a verified graph document.
 *  Files come from module nodes and callable paths. Functions keep source order in each file.
 *  Calls are the distinct `calls` edges between two different callables. */
export function deckGraph(document: GraphDocument, repo: string): DeckGraph {
  const nodes = document.graph.nodes;
  const byId = new Map(nodes.map(node => [node.id, node]));
  const files = new Set<string>();
  for (const node of nodes) if (node.kind === "module" || node.kind === "callable") files.add(node.path);
  const start = (node: typeof nodes[number]) => node.declarations?.[0]?.start ?? Number.MAX_SAFE_INTEGER;
  const callables = nodes.filter(node => node.kind === "callable")
    .sort((a, b) => a.path.localeCompare(b.path) || start(a) - start(b) || a.key.localeCompare(b.key));
  const functions: DeckFunction[] = callables.map(node => ({ id: node.key, name: displayName(node.key, node.name), file: node.path, cluster: null }));
  const seen = new Set<string>();
  const calls: DeckCall[] = [];
  for (const edge of document.graph.edges) {
    if (edge.kind !== "calls") continue;
    const source = byId.get(edge.source), target = byId.get(edge.target);
    if (!source || !target || source.kind !== "callable" || target.kind !== "callable" || source.key === target.key) continue;
    const id = `${source.key}\n${target.key}`;
    if (seen.has(id)) continue;
    seen.add(id);
    calls.push({ source: source.key, target: target.key });
  }
  const sorted = [...files].sort();
  return { repo, files: sorted, tree: fileTree(sorted), functions, calls };
}

/** Builds the directory tree. In each directory, files come first, then subdirectories, each in name order. */
export function fileTree(paths: readonly string[]): DeckTree {
  interface Node { dirs: Map<string, Node>; files: string[] }
  const root: Node = { dirs: new Map(), files: [] };
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (const dir of parts.slice(0, -1)) {
      if (!node.dirs.has(dir)) node.dirs.set(dir, { dirs: new Map(), files: [] });
      node = node.dirs.get(dir)!;
    }
    node.files.push(parts[parts.length - 1]);
  }
  const emit = (node: Node): DeckTree => {
    const out: DeckTree = {};
    for (const file of [...node.files].sort()) out[file] = null;
    for (const name of [...node.dirs.keys()].sort()) out[name + "/"] = emit(node.dirs.get(name)!);
    return out;
  };
  return emit(root);
}

// ============ flow ============

const COLORS = new Set<ClusterColor>(["cyan", "red", "yellow", "magenta", "lime", "orange", "blue", "violet"]);

/** Adapts a frozen schema 3 flow artifact.
 *  Clusters keep intent order. Members come from assigned member decisions that the graph contains.
 *  A step with no cluster label joins no deck; release 1 artifacts always carry a label. */
export function deckFlow(artifact: FlowArtifact, graph: DeckGraph, graphDigest: string, entry: FlowEntry | null = null): DeckFlow {
  if (artifact?.kind !== "tekdek.flow" || artifact.schemaVersion !== 3 || !Array.isArray(artifact.intent?.steps)) {
    throw new Error("The flow file is not a frozen schema 3 flow artifact.");
  }
  const present = new Set(graph.functions.map(fn => fn.id));
  const colors = new Map(artifact.view.clusters.map(view => [view.clusterKey, view.color]));
  const clusters: DeckCluster[] = artifact.intent.clusters.map(cluster => {
    const color = colors.get(cluster.key) as ClusterColor | undefined;
    return {
      id: cluster.key, label: cluster.label, description: cluster.description,
      color: color && COLORS.has(color) ? color : "accent",
      members: artifact.assignment.members
        .filter(member => member.outcome === "assigned" && member.clusterKey === cluster.key && present.has(member.nodeKey))
        .map(member => member.nodeKey),
    };
  });
  const transitions = new Map((artifact.view.steps ?? []).map(view => [view.stepId, view.transition]));
  const facts = new Map((artifact.facts.nodes ?? []).map(node => [node.key, node]));
  const steps: DeckStep[] = artifact.intent.steps.map((step, i) => ({
    index: i + 1, id: step.id, title: step.title, node: step.nodeKey, cluster: step.clusterKey ?? "",
    annotation: step.annotation, missing: !present.has(step.nodeKey),
    transition: transitions.get(step.id) ?? null,
    signature: facts.get(step.nodeKey)?.signature ?? null,
    documentation: facts.get(step.nodeKey)?.documentation ?? null,
  }));
  return {
    name: artifact.flowId, generation: artifact.generation?.ordinal ?? 1,
    stale: Boolean(entry?.stale) || artifact.facts.binding.graphDigest !== graphDigest,
    steps, clusters,
  };
}

// ============ shared rules ============

/** Returns a copy of the graph where each function carries the flow cluster that holds it. */
export function withMembership(graph: DeckGraph, flow: DeckFlow | null): DeckGraph {
  const clusterOf = new Map<string, string>();
  for (const cluster of flow?.clusters ?? []) for (const id of cluster.members) if (!clusterOf.has(id)) clusterOf.set(id, cluster.id);
  return { ...graph, functions: graph.functions.map(fn => ({ ...fn, cluster: clusterOf.get(fn.id) ?? null })) };
}

/** Returns the flows that the viewer shows: the frozen schema 3 entries of a codebase, in index order. */
export const frozenFlows = (flows: readonly FlowEntry[] | undefined) => (flows ?? []).filter(flow => flow.schemaVersion === 3);

/** Picks the flow to open: the requested name, else the first flow. */
export function pickFlow(flows: readonly FlowEntry[], requested: string | null): FlowEntry | null {
  return flows.find(flow => flow.name === requested) ?? flows[0] ?? null;
}

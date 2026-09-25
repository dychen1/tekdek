/* Artifact loading. This module owns every fetch of the viewer: the artifact index, one graph file with its
   SHA-256 digest check, and one frozen flow generation. Each failure is a LoadError with its cause.
   # Sections
   errors, fetch, index, graph, flow */
import type { ArtifactEntry, ArtifactIndex, FlowArtifact, FlowEntry, GraphDocument } from "./models.ts";

// ============ errors ============

/** The cause of a load failure. Each cause has one recovery step in the page. */
export type LoadReason = "fetch" | "digest" | "graph" | "flow" | "empty";

/** A load failure with its cause. */
export class LoadError extends Error {
  constructor(readonly reason: LoadReason, message: string) { super(message); }
}

// ============ fetch ============

const artifactUrl = (path: string) => "/artifacts/" + path.split("/").map(encodeURIComponent).join("/");

async function fetchBytes(path: string): Promise<Response> {
  let response: Response;
  try { response = await fetch(artifactUrl(path), { cache: "no-store" }); }
  catch { throw new LoadError("fetch", `The viewer server did not answer the request for ${path}.`); }
  if (!response.ok) throw new LoadError("fetch", `Could not load ${path} (HTTP ${response.status}).`);
  return response;
}

// ============ index ============

/** Loads the artifact index. An index with no codebase is an `empty` failure. */
export async function loadIndex(): Promise<ArtifactIndex> {
  const index = await (await fetchBytes("index.json")).json() as ArtifactIndex;
  if (!index?.artifacts?.length) throw new LoadError("empty", "No codebase is indexed yet.");
  return index;
}

// ============ graph ============

/** Returns the `sha256:<hex>` digest of the bytes. */
export async function digestOf(bytes: ArrayBuffer): Promise<string> {
  const hash = await crypto.subtle.digest("SHA-256", bytes);
  return `sha256:${Array.from(new Uint8Array(hash), byte => byte.toString(16).padStart(2, "0")).join("")}`;
}

/** Checks graph bytes against the index digest before any graph fact is used, then parses them.
 *  A digest mismatch is a `digest` failure; a document that is not a schema 1 graph is a `graph` failure. */
export async function verifyGraph(bytes: ArrayBuffer, entry: Pick<ArtifactEntry, "file" | "digest">): Promise<GraphDocument> {
  if (await digestOf(bytes) !== entry.digest) throw new LoadError("digest", `The graph file ${entry.file} changed after the artifact index was written.`);
  let graph: GraphDocument;
  try { graph = JSON.parse(new TextDecoder().decode(bytes)); }
  catch { throw new LoadError("graph", `The graph file ${entry.file} is not valid JSON.`); }
  if (graph?.schemaVersion !== 1 || !Array.isArray(graph.graph?.nodes) || !Array.isArray(graph.graph?.edges)) {
    throw new LoadError("graph", `The graph file ${entry.file} has an unsupported schema.`);
  }
  return graph;
}

/** Loads and verifies the graph file of one codebase. */
export async function loadGraph(entry: ArtifactEntry): Promise<GraphDocument> {
  return verifyGraph(await (await fetchBytes(entry.file)).arrayBuffer(), entry);
}

// ============ flow ============

/** Loads the frozen generation that an index entry points to. The adapter checks its shape. */
export async function loadFlow(entry: FlowEntry): Promise<FlowArtifact> {
  return await (await fetchBytes(`flows/${entry.file}`)).json() as FlowArtifact;
}

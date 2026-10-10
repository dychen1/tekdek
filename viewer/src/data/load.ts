/* Artifact loading. This module owns every fetch of the viewer: the artifact index, one graph file with its
   SHA-256 digest check, one frozen flow generation, and the function source of one flow. Each failure is a
   LoadError with its cause.
   # Sections
   errors, fetch, index, graph, flow, source */
import type { ArtifactEntry, ArtifactIndex, FlowArtifact, FlowEntry, GraphDocument, SourceDocument, SourceView } from "./models.ts";

// ============ errors ============

/** The cause of a load failure. Each cause has one recovery step in the page. */
export type LoadReason = "fetch" | "digest" | "graph" | "flow" | "empty";

/** A load failure with its cause. */
export class LoadError extends Error {
  constructor(readonly reason: LoadReason, message: string) { super(message); }
}

// ============ fetch ============

const artifactUrl = (path: string) => "/artifacts/" + path.split("/").map(encodeURIComponent).join("/");

/** Fetches one URL. No answer or an HTTP error is a `fetch` failure. */
async function fetchOk(url: string, what: string): Promise<Response> {
  let response: Response;
  try { response = await fetch(url, { cache: "no-store" }); }
  catch { throw new LoadError("fetch", `The viewer server did not answer the request for ${what}.`); }
  if (!response.ok) throw new LoadError("fetch", `Could not load ${what} (HTTP ${response.status}).`);
  return response;
}

/** Fetches and parses one JSON answer. An answer that is not JSON is a failure with the caller's reason. */
async function fetchJson<T>(url: string, what: string, reason: LoadReason): Promise<T> {
  const response = await fetchOk(url, what);
  try { return await response.json() as T; }
  catch { throw new LoadError(reason, `The answer for ${what} is not valid JSON.`); }
}

// ============ index ============

/** Loads the artifact index. An index with no codebase is an `empty` failure. */
export async function loadIndex(): Promise<ArtifactIndex> {
  const index = await fetchJson<ArtifactIndex>(artifactUrl("index.json"), "index.json", "fetch");
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
  return verifyGraph(await (await fetchOk(artifactUrl(entry.file), entry.file)).arrayBuffer(), entry);
}

// ============ flow ============

/** Loads the frozen generation that an index entry points to. The adapter checks its shape. */
export async function loadFlow(entry: FlowEntry): Promise<FlowArtifact> {
  const path = `flows/${entry.file}`;
  return fetchJson<FlowArtifact>(artifactUrl(path), path, "flow");
}

// ============ source ============

/** Loads the source of every step function of one flow. The server cuts it from the project files. */
export async function loadSource(artifact: string, flow: string): Promise<Map<string, SourceView>> {
  const url = `/source?artifact=${encodeURIComponent(artifact)}&flow=${encodeURIComponent(flow)}`;
  const document = await fetchJson<SourceDocument>(url, `the source of ${flow}`, "fetch");
  return new Map((document.functions ?? []).map(view => [view.key, view]));
}

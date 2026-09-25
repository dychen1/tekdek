/* Viewer data model. This module owns the wire types that the viewer reads (the artifact index, the graph
   document, the frozen flow artifact) and the deck types that the adapters produce.
   # Sections
   artifact index, graph document, flow artifact, deck data */

// ============ artifact index ============

/** One flow in the artifact index. `file` points to a frozen schema 3 generation under `flows/`. */
export interface FlowEntry { name: string; file: string; behavior?: string; stepCount?: number; clusterCount?: number; schemaVersion?: number; stale?: boolean }

/** One indexed codebase. */
export interface ArtifactEntry { label?: string; file: string; digest: string; flows?: FlowEntry[] }

/** The artifact index that the viewer server returns. */
export interface ArtifactIndex { defaultArtifact?: string; artifacts: ArtifactEntry[] }

// ============ graph document ============

/** The fields of one graph node that the viewer reads. */
export interface GraphNode { id: number; key: string; name: string; path: string; kind: string; declarations?: { start: number; end: number }[] }

/** The fields of one graph edge that the viewer reads. */
export interface GraphEdge { source: number; target: number; kind: string }

/** A schema 1 graph document, after its digest check. */
export interface GraphDocument { schemaVersion: number; graph: { nodes: GraphNode[]; edges: GraphEdge[] } }

// ============ flow artifact ============

/** The fields of a frozen schema 3 flow artifact that the viewer reads. */
export interface FlowArtifact {
  kind: string; schemaVersion: number; flowId: string;
  facts: { binding: { graphDigest: string } };
  intent: {
    behavior: string;
    clusters: { key: string; label: string; description: string }[];
    steps: { id: string; nodeKey: string; clusterKey: string | null; title: string; annotation: string }[];
  };
  assignment: { members: { outcome: string; nodeKey: string; clusterKey?: string }[] };
  view: { clusters: { clusterKey: string; color: string }[] };
}

// ============ deck data ============

/** Palette slots of `flow-bright/1`, plus the accent for a cluster with an unknown color. */
export type ClusterColor = "cyan" | "red" | "yellow" | "magenta" | "lime" | "orange" | "blue" | "violet" | "accent";

/** One callable from the indexed graph. `cluster` is the flow cluster that holds it, or null. */
export interface DeckFunction { id: string; name: string; file: string; cluster: string | null }

/** One static call relationship in the source, caller first. It is not a runtime count. */
export interface DeckCall { source: string; target: string }

/** Directory tree: a key that ends with "/" is a directory; any other key is a file. */
export interface DeckTree { [name: string]: DeckTree | null }

/** Source facts that the deck shows, independent of any flow. */
export interface DeckGraph { repo: string; files: string[]; tree: DeckTree; functions: DeckFunction[]; calls: DeckCall[] }

/** One flow cluster. `members` holds only functions that the bound graph contains. */
export interface DeckCluster { id: string; label: string; description: string; color: ClusterColor; members: string[] }

/** One flow step. `missing` is true when the bound graph no longer has the step function. */
export interface DeckStep { index: number; id: string; title: string; node: string; cluster: string; annotation: string; missing: boolean }

/** One playable flow. `stale` is true when the flow was frozen against another graph digest. */
export interface DeckFlow { name: string; behavior: string; stale: boolean; steps: DeckStep[]; clusters: DeckCluster[] }

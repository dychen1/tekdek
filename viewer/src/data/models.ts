/* Viewer data model. This module owns the wire types that the viewer reads (the artifact index, the graph
   document, the frozen flow artifact) and the deck types that the adapters produce.
   # Sections
   artifact index, graph document, flow artifact, diff artifact, deck data, function source */

// ============ artifact index ============

/** One flow in the artifact index. `file` points to a frozen schema 4 generation under `flows/`. */
export interface FlowEntry { name: string; file: string; stepCount?: number; clusterCount?: number; schemaVersion?: number; stale?: boolean }

/** One indexed codebase. `digest` names the graph document that the server sends, with its cached semantic edges;
 *  `graphDigest` names the graph file that flows bind. An older server sends no `graphDigest`: the two are the same. */
export interface ArtifactEntry { label?: string; file: string; digest: string; graphDigest?: string; flows?: FlowEntry[] }

/** One frozen PR diff in the artifact index. `file` is relative to the store, for example `diffs/<name>.json`. */
export interface DiffEntry { name: string; file: string; baseCommit: string; headCommit: string; behavior: string }

/** The artifact index that the viewer server returns. */
export interface ArtifactIndex { defaultArtifact?: string; artifacts: ArtifactEntry[]; diffs?: DiffEntry[] }

// ============ graph document ============

/** The fields of one graph node that the viewer reads. */
export interface GraphNode { id: number; key: string; name: string; path: string; kind: string; declarations?: { start: number; end: number }[] }

/** The fields of one graph edge that the viewer reads. */
export interface GraphEdge { source: number; target: number; kind: string }

/** A schema 1 graph document, after its digest check. */
export interface GraphDocument { schemaVersion: number; graph: { nodes: GraphNode[]; edges: GraphEdge[] } }

// ============ flow artifact ============

/** The static call relation from the previous step's function. It is not a runtime observation. */
export type Transition = "flow_start" | "same_function" | "calls" | "returns_to_caller" | "sibling_call"
  | "calls_indirectly" | "returns_to_ancestor" | "no_static_edge";

/** The step text: one row for each used slot, in display order. A null slot is not used. Items hold marks. */
export interface StepLedger {
  takes: string[] | null; requires: string[] | null; does: string[] | null;
  onFail: string[] | null; returns: string[] | null; writes: string[] | null;
}

/** The fields of a frozen schema 4 flow artifact that the viewer reads. */
export interface FlowArtifact {
  kind: string; schemaVersion: number; flowId: string;
  generation: { ordinal: number };
  facts: { binding: { graphDigest: string }; nodes: { key: string; signature?: string | null; documentation?: string | null }[] };
  intent: {
    clusters: { key: string; label: string; phase: string; phaseNote: string | null; input: string[]; output: string[]; rules: string[] }[];
    steps: { id: string; nodeKey: string; clusterKey: string | null; title: string; ledger: StepLedger }[];
  };
  assignment: { members: { outcome: string; nodeKey: string; clusterKey?: string }[] };
  view: { clusters: { clusterKey: string; color: string }[]; steps: { stepId: string; transition: Transition }[] };
}

// ============ diff artifact ============

/** The change of one step. `moved` means that the function is in another file. A cluster change is never a move. */
export type ChangeKind = "new" | "removed" | "changed" | "moved" | "unchanged";

/** Lines `start` to `end` of one file, counted from 1, both included. */
export interface LineSpan { path: string; start: number; end: number }

/** One line of a unified diff. */
export interface DiffLine { op: "context" | "added" | "removed"; text: string }

/** The derived facts of one After step, of one pair, or of one removed Before step. */
export interface DiffRow {
  kind: ChangeKind; beforeStep: string | null; afterStep: string | null;
  baseSpan: LineSpan | null; headSpan: LineSpan | null; added: number; removed: number; lines: DiffLine[];
}

/** The note of one cluster on one side: what its code does differently. */
export interface DiffNote { side: "before" | "after"; clusterKey: string; text: string }

/** A frozen schema 1 PR diff. `rows` holds the After steps in flow order, then the removed Before steps. */
export interface DiffArtifact {
  kind: string; schemaVersion: number; name: string;
  base: { commit: string; flow: FlowArtifact }; head: { commit: string; flow: FlowArtifact };
  pairs: { before: string; after: string }[]; notes: DiffNote[]; rows: DiffRow[];
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
export interface DeckGraph { tree: DeckTree; functions: DeckFunction[]; calls: DeckCall[] }

/** One flow cluster. `members` holds only functions that the bound graph contains. */
/** One cluster. `phase` is the readable stage name. `input` and `output` name data models; empty means none. */
export interface DeckCluster {
  id: string; label: string; phase: string; input: string[]; output: string[]; rules: string[]; color: ClusterColor; members: string[];
}

/** One flow step. `missing` is true when the bound graph no longer has the step function. */
export interface DeckStep {
  index: number; id: string; title: string; node: string; cluster: string; ledger: StepLedger; missing: boolean;
  transition: Transition | null; signature: string | null; documentation: string | null;
}

/** One playable flow. `stale` is true when the flow was frozen against another graph digest. */
export interface DeckFlow { name: string; generation: number; stale: boolean; steps: DeckStep[]; clusters: DeckCluster[] }

// ============ function source ============

/** The source of one step function, as the `/source` route answers it. Line numbers are one-based. */
export interface SourceView {
  key: string; status: "ok" | "missing" | "changed" | "unreadable" | "no_range";
  path?: string; language?: string; reason?: string;
  firstLine?: number; declarationLine?: number; docLines?: [number, number] | null; signatureLines?: [number, number];
  lines?: string[]; more?: number;
}

/** The `/source` answer for one flow. */
export interface SourceDocument { artifact: string; flow: string; functions: SourceView[] }

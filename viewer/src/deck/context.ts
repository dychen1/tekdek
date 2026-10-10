/* Deck context. This module owns what the deck modules share for one codebase and one flow:
   the DOM helper, the part interfaces, and the context. The model and the state come from `layout/model.ts`.
   `deck.ts` builds one context; every other deck module reads and writes through it.
   # Sections
   DOM helpers, parts, context */
import type { DeckFlow, DeckGraph, SourceView } from "../data/models.ts";
import type { DeckModel, DeckState } from "../layout/model.ts";

// ============ DOM helpers ============

export const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector(selector) as T;

// ============ parts ============

/** Created by `explorer.ts`. */
export interface Explorer {
  /** Marks the current file, the current cluster's files, and the open state in the rows. */
  sync(): void;
  /** Scrolls a long tree so the current file's row sits inside the canvas frame's height. */
  reveal(): void;
  /** The row element that the guide starts from, or null when no file shows. */
  anchorEl(): HTMLElement | null;
  destroy(): void;
}

/** Created by `file-box.ts`. */
export interface FileBoxView {
  render(): void;
  /** The height of the shown box, from its top edge to the bottom of its last row. */
  height(): number;
  nodeEl(): HTMLElement | null;
  /** The current function's row node, or null when no row is current. */
  slotEl(): HTMLElement | null;
  /** The largest right edge of the rows on screen. */
  rowsRight(): number;
  destroy(): void;
}

/** Created by `cluster-deck.ts`. */
export interface ClusterDeck {
  /** Replaces the deck with the deck of a cluster, already raised, or removes it for null. */
  show(cluster: string | null): void;
  /** Marks the current box and shows the file path on the boxes of other files. */
  mark(): void;
  /** Puts the deck at its screen spot while the canvas scales by k around the anchor. */
  pin(spot: { x: number; y: number }, anchor: { x: number; y: number }, k: number): void;
  /** The lid of the current step's box, or null when the deck has none. */
  currentLid(): HTMLElement | null;
  /** The right edge of the deck's title band, or 0 without a deck. */
  deckRight(): number;
  destroy(): void;
}

/** Created by `links.ts`. */
export interface Links { draw(): void; destroy(): void }

/** Declared here and created by `code.ts`. */
export interface CodeColumn { render(prev: number): void; destroy(): void }
/** Declared here and created by `bar.ts`. */
export interface Bar { render(clusterChanged: boolean): void; destroy(): void }
/** Declared here and created by `tools.ts`. */
export interface Tools {
  sync(): void; layoutTop(): void; setFull(on: boolean): void; toggleCompact(): void; setZoom(k: number): void;
  keyOpen(): boolean; setKey(open: boolean, returnFocus?: boolean): void; destroy(): void;
}
/** Declared here and created by `input.ts`. */
export interface Input { destroy(): void }

/** The deck modules. `deck.ts` fills this object after it creates every module. */
export interface DeckParts {
  explorer: Explorer; fileBox: FileBoxView; clusterDeck: ClusterDeck; links: Links;
  code: CodeColumn; bar: Bar; tools: Tools; input: Input;
}

// ============ context ============

/** Options that the page gives a deck. */
export interface DeckOptions {
  graph: DeckGraph;
  flow: DeckFlow | null;
  /** The source of each step function, by function key. Empty when the flow has none. */
  sources: ReadonlyMap<string, SourceView>;
  /** Number of flows in the codebase. The flow badge is a picker when it is more than one. */
  flowCount: number;
  onFlowPick?: (anchor: HTMLElement) => void;
  onStep?: (step: number) => void;
}

/** The shared context of one deck. */
export interface DeckContext {
  options: DeckOptions;
  model: DeckModel;
  state: DeckState;
  parts: DeckParts;
  /** Aborts every listener of the deck on teardown. */
  signal: AbortSignal;
  /** True when the viewer prefers reduced motion. */
  reducedMotion: boolean;
  /** Goes to a step. */
  go(step: number): void;
  /** Places the file box and the deck, then draws the connectors. */
  place(): void;
  /** Draws the connectors on every frame for a while (layout settles, fonts load), then once at rest. */
  kick(ms: number): void;
}

/* Deck instance. This module creates the deck for one codebase and one flow, wires its modules, and owns the step
   controller and the placement of the file box and the pinned deck. The page shell (`index.html`) holds the static
   elements; an instance fills them, and `destroy` empties them.
   # Sections
   instance, stepping, placement, boot and teardown */
import { deckSpot, fileBoxCenter, FILE_X, fileRoom, zoomFit } from "../layout/deck-geometry.ts";
import { openTo } from "../layout/explorer.ts";
import { buildModel, initialState, shownZoom } from "../layout/model.ts";
import { clamp, f1 } from "../layout/util.ts";
import { createBar } from "./bar.ts";
import { createClusterDeck } from "./cluster-deck.ts";
import { createCode } from "./code.ts";
import { createExplorer } from "./explorer.ts";
import { createFileBox } from "./file-box.ts";
import { createInput } from "./input.ts";
import { createLinks } from "./links.ts";
import { $, pageOrigin, type DeckContext, type DeckOptions, type DeckParts } from "./context.ts";
import { createTools } from "./tools.ts";


export interface DeckInstance {
  go(step: number): void;
  destroy(): void;
  /** State for browser checks. */
  inspect: Record<string, unknown>;
}

/** Returns the screen rectangle of an element, or null. */
function rectOf(el: Element | null) {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { x: r.x, y: r.y, width: r.width, height: r.height };
}

/** Creates the deck for one codebase and one flow in the page shell. */
export function createDeck(options: DeckOptions): DeckInstance {

  // ============ instance ============

  const abort = new AbortController();
  const model = buildModel(options.graph, options.flow, options.sources);
  const state = initialState(model);
  const ctx: DeckContext = {
    model, state, parts: {} as DeckParts, signal: abort.signal, go, place, placeSoon,
  };
  let alive = true;
  /** The cluster whose deck shows, or null. */
  let deckCluster: string | null = null;
  // The page shell holds these elements. `place` reads them, so they come before the parts.
  const cv = $("#cv"), fileblk = $("#fileblk"), frameEl = $("#frame"), card = $("#card");
  let placeRaf = 0;
  // The boot class keeps the first frames still: no fade, no transition. It also covers a URL step.
  document.body.classList.add("booting");
  // Every module reads the others through `ctx.parts` at call time, so the creation order is free.
  // A part may call `ctx.place` only after all parts exist.
  const parts = ctx.parts;
  parts.explorer = createExplorer(ctx);
  parts.fileBox = createFileBox(ctx);
  parts.clusterDeck = createClusterDeck(ctx);
  parts.links = createLinks(ctx);
  parts.code = createCode(ctx);
  parts.bar = createBar(ctx);
  parts.tools = createTools(ctx);
  createInput(ctx);
  const { explorer, fileBox, clusterDeck, links, code, bar, tools } = parts;
  // Every element that goes to a step carries `data-step`: in the file box, the deck, the code column, and the step strip.
  document.addEventListener("click", e => {
    const t = (e.target as Element).closest<HTMLElement>("[data-step]");
    if (t) go(+t.dataset.step!);
  }, { signal: abort.signal });
  addEventListener("resize", placeSoon, { signal: abort.signal });

  // ============ stepping ============
  // Nothing on the canvas moves between steps: each step sets the file box, the deck, and the connectors at their
  // final place in one frame. A cluster change replaces the deck. Only the code column and the bar text fade in.

  /** Goes to a step. A step clears a browsed file, also when the step does not change. A clear keeps its step for forward. */
  function go(n: number) {
    n = clamp(Math.round(n), 0, model.N);
    if (!Number.isFinite(n)) return;
    const browsed = state.browseFile !== null;
    state.browseFile = null;
    if (n === state.cur) { if (browsed) render(n); return; }
    const prev = state.cur;
    if (!n) state.resume = prev;
    state.cur = n;
    // full screen shows code, and the rest state has none
    if (!n && state.full) tools.setFull(false);
    render(prev);
  }

  /** Shows the current step in every part. `prev` is the step before it. */
  function render(prev: number) {
    const n = state.cur, step = model.stepAt(n);
    const cluster = model.stepCluster(n);
    const clusterChanged = prev !== n && (!n || !prev || cluster !== model.stepCluster(prev));
    document.documentElement.style.setProperty("--cc", cluster ? model.CL.get(cluster)!.cv : "var(--accent)");
    document.body.classList.toggle("flow", !!step);
    options.onStep?.(n);
    const fn = model.fnAt(n);
    const row = fn ? model.rowByPath.get(fn.file) : undefined;
    if (row) openTo(state.open, row);
    explorer.sync();
    // the canvas frame has no size in full screen, so the tree keeps its scroll there
    if (!state.full) explorer.reveal();
    fileBox.render();
    if (deckCluster !== cluster) { clusterDeck.show(cluster); deckCluster = cluster; }
    clusterDeck.mark();
    code.render(prev);
    place();
    bar.render(clusterChanged);
    tools.sync();
  }

  // ============ placement ============
  // The file box stays level with its explorer row, so the guide runs straight. Only a box too tall for the frame at
  // that height moves up. The deck is pinned: one spot for every step. Zoom scales the canvas around the file node.
  // The deck keeps its right edge and top edge at every zoom. Zoom stops where the deck and the file box no longer fit
  // the frame side by side.

  /** Places the file box and the deck, then draws the connectors. A browsed file shows without a deck.
   *  Returns the start zoom of the frame that it measured. */
  function place(): number {
    if (state.browseFile && deckCluster !== null) { clusterDeck.show(null); deckCluster = null; }
    // page coordinates: the canvas layer sits at the page origin, which moves when the page scrolls
    const o = pageOrigin(), r = frameEl.getBoundingClientRect();
    const frame = { left: r.left - o.x, top: r.top - o.y, width: r.width, height: r.height };
    const codeLeft = card.getBoundingClientRect().left - o.x;
    // full screen hides the frame, and a fit of no frame is the floor: the last fit stays
    const fit = state.full ? null : zoomFit(frame, codeLeft, model.deckSize, model.fileRight, model.fileMin);
    if (fit) state.zoomMax = fit.max;
    const k = shownZoom(state), x = frame.left + FILE_X;
    const spot = deckSpot(frame, codeLeft, model.deckSize.width, model.deckSize.maxHeight, k);
    // with a deck up, a row that would reach it ends in an ellipsis, so the tether never crosses a row
    if (deckCluster !== null) fileblk.style.setProperty("--fb-room", `${f1(fileRoom(frame.left, spot.x, k))}px`);
    else fileblk.style.removeProperty("--fb-room");
    let y = frame.top + 18;
    if (fileBox.nodeEl()) {
      const rr = explorer.anchorEl()?.getBoundingClientRect();
      y = fileBoxCenter(rr ? rr.top - o.y + rr.height / 2 : y, frame, fileBox.height(), k);
      fileblk.style.transform = `translate(${f1(x)}px,${f1(y - 10)}px)`;
    }
    state.anchor = { x, y };
    cv.style.transformOrigin = `${f1(x)}px ${f1(y)}px`;
    cv.style.transform = `scale(${k})`;
    cv.style.setProperty("--k", String(k));
    clusterDeck.pin(spot, state.anchor, k);
    links.draw();
    return fit?.start ?? k;
  }

  function placeSoon() {
    if (placeRaf) return;
    // a smaller window can lower the zoom, so the tools show the new level
    placeRaf = requestAnimationFrame(() => { placeRaf = 0; tools.layoutTop(); place(); tools.sync(); });
  }

  // ============ boot and teardown ============
  // The first place() measures the frame. When the widest flow file box and the deck do not fit at 100%, the canvas
  // starts at the largest zoom that fits, but not under the floor.

  tools.layoutTop();
  explorer.sync();
  fileBox.render();
  state.zoom = place();
  render(0);
  requestAnimationFrame(() => requestAnimationFrame(() => {
    if (!alive) return;
    document.body.classList.remove("booting");
    links.draw();
  }));
  // Loaded fonts change line heights. The code column renders first: its step text moves the code header that the arrow joins.
  document.fonts?.ready.then(() => {
    if (!alive) return;
    tools.layoutTop();
    code.render(state.cur);
    place();
  });

  function destroy() {
    alive = false;
    abort.abort();
    if (placeRaf) cancelAnimationFrame(placeRaf);
    placeRaf = 0;
    for (const part of [tools, bar, code, links, clusterDeck, fileBox, explorer]) part.destroy();
    document.body.classList.remove("flow", "full", "booting");
    document.documentElement.style.removeProperty("--cc");
  }

  /** The `d` attribute of a connector path, or null when the connector is hidden. */
  const pathOf = (group: string, path: string) => ($(group).classList.contains("away") ? null : $(path).getAttribute("d"));

  return {
    go, destroy,
    inspect: {
      go,
      get cur() { return state.cur; },
      /** The zoom that the map shows. */
      get zoom() { return shownZoom(state); },
      get full() { return state.full; },
      get compact() { return state.compact; },
      /** The screen box of the deck camera (its pinned spot), or null without a deck. */
      deckRect: () => rectOf(cv.querySelector(".deckcam")),
      fileNodeRect: () => rectOf(fileBox.nodeEl()),
      paths: () => ({
        guide: pathOf("#guideG", "#guidePath"),
        tether: pathOf("#tetherG", "#tetherPath"),
        arrow: pathOf("#arrowG", "#arrPath"),
      }),
    },
  };
}

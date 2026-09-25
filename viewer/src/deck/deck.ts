/* Deck instance. This module creates the deck for one codebase and one flow, wires its modules, and owns the
   step controller and the frame loop. The page shell (`index.html`) holds the static elements; an instance fills
   them and `destroy` empties them.
   # Sections
   instance, step controller, frame loop, boot and teardown */
import { clamp } from "../layout/camera.ts";
import { buildModel, initialState, KEY_ANN, readTokens, store, type DeckContext, type DeckOptions, type DeckParts } from "./context.ts";
import { createConnectors } from "./connectors.ts";
import { createControls } from "./controls.ts";
import { createInput } from "./input.ts";
import { createPanels } from "./panels.ts";
import { createSidebar } from "./sidebar.ts";
import { createViewport } from "./viewport.ts";
import { createWorld } from "./world.ts";

export type { DeckOptions } from "./context.ts";

export interface DeckInstance {
  go(step: number): void;
  destroy(): void;
  /** State for browser checks. */
  inspect: Record<string, unknown>;
}

/** Creates the deck for one codebase and one flow in the page shell. */
export function createDeck(options: DeckOptions): DeckInstance {

  // ============ instance ============

  const abort = new AbortController();
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const model = buildModel(options.graph, options.flow);
  const state = initialState(model, store.get(KEY_ANN) === "1");
  let timers: number[] = [];
  let alive = true;
  const ctx: DeckContext = {
    options, model, state, tokens: readTokens(), parts: {} as DeckParts,
    signal: abort.signal, reducedMotion,
    D: ms => (reducedMotion ? 0 : ms),
    later(ms, fn) { timers.push(window.setTimeout(fn, ms)); },
    clearTimers() { timers.forEach(clearTimeout); timers = []; },
    go,
  };
  // Every module reads the others through `ctx.parts` at call time, so the creation order is free.
  const parts: DeckParts = ctx.parts;
  parts.viewport = createViewport(ctx);
  parts.world = createWorld(ctx);
  parts.connectors = createConnectors(ctx);
  parts.sidebar = createSidebar(ctx);
  parts.panels = createPanels(ctx);
  parts.controls = createControls(ctx);
  parts.input = createInput(ctx);
  const { viewport, world, connectors, sidebar, panels, controls } = parts;

  // ============ step controller ============
  // Within the lifted cluster only the focus moves. A cluster change sinks the old deck first, then lifts the new one.
  // The tree does not move between clusters: only the old slots become named rows and the new rows become slots.

  function go(n: number) {
    n = clamp(n, 0, model.N);
    if (n === state.cur) return;
    state.cur = n;
    options.onStep?.(n);
    const target = n ? model.stepCluster(n) : null;
    controls.update(); sidebar.update();
    panels.refreshHover();
    if (target && target === state.layoutCluster && target === state.liftedCluster) {
      world.applyFocus(); viewport.focus(); connectors.render(); connectors.show(true);
      panels.updateDock();
      return;
    }
    if (n && !target && state.flowOn && !state.layoutCluster && !state.liftedCluster) { world.applyFocus(); viewport.row(); return; }
    if (!n && !state.flowOn && !state.layoutCluster && !state.liftedCluster) return;
    clusterTransition(target, n > 0);
  }

  function clusterTransition(target: string | null, wantFlow: boolean) {
    ctx.clearTimers();
    panels.dropBoxHover();
    const hadLift = !!state.liftedCluster && state.liftedCluster !== target;
    if (hadLift) world.drop(state.liftedCluster!);
    connectors.show(false); connectors.setArrow(false);
    const t1 = hadLift ? ctx.D(380) : 0;
    ctx.later(t1, () => {
      // The flow layout starts and ends here, once per flow.
      world.setFlowLayout(wantFlow);
      state.layoutCluster = target;
      document.body.classList.toggle("lifted", !!target);
      world.relayout(); panels.renderCallout(); panels.renderDock();
      if (target) viewport.cluster();
      else { world.applyFocus(); if (wantFlow) viewport.row(); else viewport.rest(true); }
    });
    if (!target) return;
    const t2 = t1 + ctx.D(hadLift ? 300 : 380);
    ctx.later(t2, () => { if (state.liftedCluster !== target) world.lift(target); world.applyFocus(); });
    ctx.later(t2 + ctx.D(760), () => { connectors.render(); connectors.show(true); connectors.setArrow(true); });
  }

  // ============ frame loop ============

  function tick() {
    if (!alive) return;
    requestAnimationFrame(tick);
    panels.placeHover();
    connectors.draw();
  }
  requestAnimationFrame(tick);

  // ============ boot and teardown ============

  controls.syncAnnotation();
  viewport.measure();
  world.relayout();
  viewport.rest(false);
  panels.renderCallout(); panels.renderDock(); controls.update(); sidebar.update();
  const worldEl = document.getElementById("world")!;
  requestAnimationFrame(() => requestAnimationFrame(() => { worldEl.classList.remove("boot"); document.body.classList.remove("booting"); }));

  function destroy() {
    alive = false;
    abort.abort();
    ctx.clearTimers();
    for (const part of [controls, panels, sidebar, connectors, world]) part.destroy();
    document.body.classList.remove("lifted", "flow");
    document.body.style.removeProperty("--cc");
  }

  return {
    go, destroy,
    inspect: {
      go, get cur() { return state.cur; }, get cam() { return { ...state.cam }; }, get view() { return { ...state.view }; },
      slab: (cid: string) => { const rect = world.slabRect(cid); return rect && { ...rect }; },
      row: (id: string) => { const r = model.rowById.get(id); return r && { x: r.x, y: r.y, mode: r.mode }; },
      dir: (id: string) => { const r = model.rowById.get(id); return r && { by: state.open.get(id) ?? null, y: r.y, mode: r.mode }; },
      frame: () => ({ w: state.treeW, h: state.treeH }),
      rows: () => model.rows.map(r => ({ id: r.id, y: r.y, h: r.h, mode: r.mode, dot: r.kind === "fn" && !!r.fn!.cluster })),
      clusterOf: (index: number) => model.stepCluster(index),
      clusters: () => model.clusters.map(c => ({ id: c.id, members: c.members.length })),
    },
  };
}

/* Deck viewport. This module owns the camera element: it measures the map area, applies the camera,
   and places the camera for rest, fit, cluster, focus, and row views. The math lives in `layout/camera.ts`. */
import { cameraTransform, clamp, clusterZoom, emptyCam, fitCam, focusCam, viewOf, zoomAt } from "../layout/camera.ts";
import { DECK_IN, GH, PAD } from "../layout/tree.ts";
import { $, type DeckContext } from "./context.ts";

/** Camera moves: `false` is instant, `true` is the camera move, and `"layout"` follows a row move,
 *  so a row that shifts holds its place on screen. */
export type CameraAnimation = boolean | "layout";

export interface Viewport {
  /** Measures the map area between the sidebar, the right column, the tools, and the controls. */
  measure(): void;
  apply(anim: CameraAnimation): void;
  fit(anim: boolean): void;
  /** Flat and readable, with the top of the staircase in view. */
  rest(anim: boolean): void;
  /** Pitches the camera and zooms to the lifted cluster, then focuses the current step. */
  cluster(): void;
  /** Looks between the focus slot and its box. A step without a box looks at the deck. */
  focus(): void;
  /** A flow step with no deck: a flat pan to its row. */
  row(): void;
  zoomBy(factor: number, sx: number, sy: number): void;
}

export function createViewport(ctx: DeckContext): Viewport {
  const { model, state, tokens } = ctx;
  const camera = $("#camera"), world = $("#world"), controlsEl = $("#controls"), zoomLbl = $("#zoomLbl");
  const geometry = () => ({ deckX: state.deckX, deckIn: DECK_IN, deckZ: tokens.deckZ, focusDz: tokens.focusDz, boxH: tokens.boxH, gh: GH });

  function measure() {
    const cs = getComputedStyle(document.documentElement);
    const sb = parseFloat(cs.getPropertyValue("--sb")) || 262, rc = parseFloat(cs.getPropertyValue("--rc")) || 320;
    state.view = viewOf(innerWidth, innerHeight, sb, rc, controlsEl.getBoundingClientRect().top || innerHeight - 150);
  }
  function apply(anim: CameraAnimation) {
    camera.style.transition = !anim || ctx.reducedMotion ? "none" : anim === "layout" ? "transform var(--dur-layout) var(--ease-lift)" : "";
    camera.style.transform = cameraTransform(state.cam, state.view);
    world.classList.toggle("far", state.cam.k < 0.5);
    world.style.setProperty("--ls", clamp(0.8 / state.cam.k, 1, 4.2).toFixed(3));
    zoomLbl.textContent = Math.round(state.cam.k * 100) + "%";
    ctx.parts.sidebar.updateViewportBar();
  }
  function fit(anim: boolean) {
    state.cam = fitCam(state.cam, state.view, state.treeW, state.treeH, !!state.liftedCluster, tokens.deckZ);
    apply(anim);
  }
  function rest(anim: boolean) { state.cam = emptyCam(state.view, state.treeW); apply(anim); }
  function cluster() {
    const deck = ctx.parts.world.slabRect(state.layoutCluster!);
    state.cam = { ...state.cam, tilt: tokens.tilt, k: clusterZoom(state.view, deck, tokens.tilt, tokens.deckZ, PAD) };
    focus();
  }
  function focus() {
    const step = model.steps[state.cur - 1];
    if (!step || !state.layoutCluster) return;
    const deck = ctx.parts.world.slabRect(state.layoutCluster);
    const row = step.missing ? null : model.rowById.get(step.node);
    // A step without a box looks at the deck, from its first member.
    const anchor = row && row.fn!.cluster ? row : model.rowById.get(model.CL.get(state.layoutCluster)!.members[0])!;
    state.cam = focusCam(state.cam, state.view, { x: anchor.x, y: anchor.y, gw: anchor.gw, lw: anchor.lw }, deck, geometry());
    apply(true);
  }
  function row() {
    const step = model.steps[state.cur - 1];
    const target = step && !step.missing ? model.rowById.get(step.node) : null;
    state.cam = { ...state.cam, tilt: 0 };
    if (target) state.cam = { ...state.cam, Fx: target.x + 160, Fy: target.y + 40 };
    apply(true);
  }
  function zoomBy(factor: number, sx: number, sy: number) { state.cam = zoomAt(state.cam, state.view, factor, sx, sy); apply(false); }

  return { measure, apply, fit, rest, cluster, focus, row, zoomBy };
}

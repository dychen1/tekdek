/* Deck input. This module owns the pointer, wheel, click, and keyboard input of the map, and the window resize.
   # Sections
   pointer, clicks, keyboard, resize */
import { toggleOpen } from "../layout/tree.ts";
import { nearestStep } from "../layout/flow-plan.ts";
import { RAD } from "../layout/camera.ts";
import { icon } from "./panels.ts";
import { $, type DeckContext } from "./context.ts";

export interface Input {
  /** True while the pointer drags the map. Box details stay closed during a drag. */
  dragging(): boolean;
}

export function createInput(ctx: DeckContext): Input {
  const { model, state, signal } = ctx;
  const stage = $("#stage");

  // ============ pointer ============

  let drag: { x: number; y: number; id: number; moved: boolean } | null = null, suppressClick = false;
  stage.addEventListener("pointerdown", e => {
    if (e.button !== 0) return;
    drag = { x: e.clientX, y: e.clientY, id: e.pointerId, moved: false };
  }, { signal });
  stage.addEventListener("pointermove", e => {
    if (!drag || e.pointerId !== drag.id) return;
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
    if (!drag.moved && Math.hypot(dx, dy) < 4) return;
    if (!drag.moved) {
      drag.moved = true; stage.classList.add("dragging");
      try { stage.setPointerCapture(e.pointerId); } catch { /* synthetic pointer: capture is optional */ }
    }
    drag.x = e.clientX; drag.y = e.clientY;
    const { cam } = state;
    state.cam = { ...cam, Fx: cam.Fx - dx / cam.k, Fy: cam.Fy - dy / (cam.k * Math.cos(cam.tilt * RAD)) };
    ctx.parts.viewport.apply(false);
  }, { signal });
  const endDrag = () => {
    if (!drag) return;
    if (drag.moved) { suppressClick = true; setTimeout(() => { suppressClick = false; }, 0); }
    stage.classList.remove("dragging");
    drag = null;
  };
  stage.addEventListener("pointerup", endDrag, { signal });
  stage.addEventListener("pointercancel", endDrag, { signal });
  stage.addEventListener("wheel", e => {
    e.preventDefault();
    ctx.parts.viewport.zoomBy(Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0016)), e.clientX, e.clientY);
  }, { passive: false, signal });

  // ============ clicks ============
  // Fold a directory or a file (the row keeps its place), close or open a box (it keeps its place),
  // or go to the nearest visit of a function.

  stage.addEventListener("click", e => {
    if (suppressClick) return;
    const target = e.target as Element;
    const rowEl = target.closest<HTMLElement>(".row");
    if (!rowEl) return;
    const r = model.rowById.get(rowEl.dataset.id!);
    if (!r) return;
    if (target.closest(".fold")) { toggleOpen(state.open, r); ctx.parts.world.refreshTree(r); return; }
    if (target.closest(".cfold")) {
      const closed = r.el.classList.toggle("closed");
      r.cfoldEl!.innerHTML = icon(closed ? "plus" : "minus");
      r.cfoldEl!.setAttribute("aria-label", (closed ? "Open box " : "Close box ") + r.name);
      r.cfoldEl!.setAttribute("aria-expanded", String(!closed));
      return;
    }
    if (r.kind === "fn" && (target.closest(".tmain") || target.closest(".node"))) {
      const n = nearestStep(model.plan, r.id, state.cur);
      if (n) ctx.go(n);
    }
  }, { signal });

  // ============ keyboard ============

  addEventListener("keydown", e => {
    if (e.altKey || e.metaKey || e.ctrlKey) return;
    const { controls } = ctx.parts;
    if (e.key === "ArrowRight") { e.preventDefault(); ctx.go(state.cur + 1); }
    else if (e.key === "ArrowLeft") { e.preventDefault(); ctx.go(state.cur - 1); }
    else if (e.key === "Escape") {
      // Close the key first; clear the flow last.
      if (controls.keyOpen()) controls.setKey(false, true);
      else ctx.go(0);
    }
    else if (e.key === "?") controls.setKey(!controls.keyOpen());
    else if (e.key === "f" || e.key === "F") ctx.parts.viewport.fit(true);
  }, { signal });

  // ============ resize ============

  addEventListener("resize", () => {
    const { viewport } = ctx.parts;
    viewport.measure();
    if (state.layoutCluster) viewport.cluster(); else if (!state.flowOn) viewport.rest(false); else viewport.apply(false);
  }, { signal });

  return { dragging: () => !!drag && drag.moved };
}

/* Deck camera. This module owns the parallel projection with a pitch, and the camera placements.
   A point (x, y, z) lands at (cx + k(x - Fx), cy + k((y - Fy)cos t - z sin t)). Verticals stay vertical.
   The module is pure math: it reads a view and returns camera values.
   # Sections
   model, transform, placements */

// ============ model ============

export const RAD = Math.PI / 180;
export const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));
export const f1 = (v: number) => v.toFixed(1);

/** Zoom `k`, pitch `tilt` in degrees, and the world point `Fx, Fy` at the view center. */
export interface Cam { k: number; tilt: number; Fx: number; Fy: number }

/** The map area between the sidebar, the right column, the top tools, and the controls. */
export interface View { W: number; H: number; left: number; right: number; top: number; bottom: number; cx: number; cy: number }

export interface Rect { x: number; y: number; w: number; h: number }

/** A tall deck does not zoom below this floor; the camera follows the focus instead. */
export const READ_K = 0.75;

/** Returns the map area from the window size, the column widths, and the top of the controls. */
export function viewOf(W: number, H: number, sb: number, rc: number, controlsTop: number): View {
  const left = sb, right = W - rc - 24, top = 48, bottom = controlsTop - 10;
  return { W, H, left, right, top, bottom, cx: left + (right - left) * 0.48, cy: top + (bottom - top) * 0.5 };
}

// ============ transform ============

/** Returns the CSS transform of the camera element. */
export function cameraTransform(cam: Cam, view: View): string {
  return `translate3d(${f1(view.cx)}px,${f1(view.cy)}px,0) rotateX(${cam.tilt}deg) ` +
    `scale3d(${cam.k},${cam.k},${cam.k}) translate3d(${f1(-cam.Fx)}px,${f1(-cam.Fy)}px,0)`;
}

/** Projects a world point to the screen. */
export function project(cam: Cam, view: View, x: number, y: number, z = 0): [number, number] {
  const t = cam.tilt * RAD;
  return [view.cx + cam.k * (x - cam.Fx), view.cy + cam.k * ((y - cam.Fy) * Math.cos(t) - z * Math.sin(t))];
}

// ============ placements ============

/** Fits the whole tree, and the raised deck when one is up. */
export function fitCam(cam: Cam, view: View, treeW: number, treeH: number, lifted: boolean, deckZ: number): Cam {
  const t = cam.tilt * RAD;
  const mw = view.right - view.left - 60, mh = view.bottom - view.top - 60;
  const ph = treeH * Math.cos(t) + (lifted ? (deckZ + 60) * Math.sin(t) : 0);
  return { ...cam, k: clamp(Math.min(mw / treeW, mh / ph), 0.14, 1.3), Fx: treeW / 2, Fy: treeH / 2 - (lifted ? (deckZ / 2) * Math.tan(t) : 0) };
}

/** Flat and readable, with the top of the staircase in view. */
export function emptyCam(view: View, treeW: number): Cam {
  const k = clamp((view.right - view.left - 120) / treeW, 0.6, 1.0);
  return { tilt: 0, k, Fx: treeW / 2, Fy: (view.cy - view.top - 36) / k };
}

/** Projected height of a deck and its slots per unit of zoom: the raised top edge to the lowest slot. */
export const deckSpan = (deck: Rect, t: number, deckZ: number) => (deck.h - 26) * Math.cos(t) + deckZ * Math.sin(t);

/** Returns the zoom for a cluster: the deck fits the width down to the zoom floor. */
export function clusterZoom(view: View, deck: Rect, tilt: number, deckZ: number, pad: number): number {
  const k1 = (view.bottom - view.top - 24) / deckSpan(deck, tilt * RAD, deckZ);
  const k2 = (view.right - view.left - 30) / ((deck.x + deck.w - pad + 30) * 1.12);
  return clamp(Math.min(Math.max(k1, READ_K), k2), 0.56, 1.0);
}

/** The camera looks between the focus slot and its box; the box sits a little above center.
 *  When the whole deck fits, the pan keeps its raised top edge and its lowest slot in view.
 *  The pan never puts the deck's right edge under the right column. */
export function focusCam(cam: Cam, view: View, slot: { x: number; y: number; gw: number; lw: number }, deck: Rect,
  geo: { deckX: number; deckIn: number; deckZ: number; focusDz: number; boxH: number; gh: number }): Cam {
  const z = geo.deckZ + 1 + geo.focusDz + geo.boxH;
  const boxX = geo.deckX + geo.deckIn + slot.lw / 2;
  // The deck's right edge stays at least 12 px left of the map area's right edge.
  const Fx = Math.max(0.5 * (slot.x + slot.gw / 2) + 0.5 * boxX, deck.x + deck.w - (view.right - 12 - view.cx) / cam.k);
  const t = cam.tilt * RAD, c = Math.cos(t), sn = Math.sin(t), k = cam.k;
  const want = slot.y + geo.gh / 2 - 0.55 * z * Math.tan(t);
  const fyMax = deck.y - ((view.top + 8 - view.cy) / k + geo.deckZ * sn) / c;
  const fyMin = deck.y + deck.h - 26 - ((view.bottom - 8 - view.cy) / k) / c;
  return { ...cam, Fx, Fy: fyMin <= fyMax ? clamp(want, fyMin, fyMax) : want };
}

/** Zooms by a factor around a screen point, which keeps its place. */
export function zoomAt(cam: Cam, view: View, factor: number, sx: number, sy: number): Cam {
  const kc0 = cam.k * Math.cos(cam.tilt * RAD);
  const px = cam.Fx + (sx - view.cx) / cam.k, py = cam.Fy + (sy - view.cy) / kc0;
  const k = clamp(cam.k * factor, 0.14, 2.4);
  const kc1 = k * Math.cos(cam.tilt * RAD);
  return { ...cam, k, Fx: px - (sx - view.cx) / k, Fy: py - (sy - view.cy) / kc1 };
}

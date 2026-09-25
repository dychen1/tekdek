// Tests for the camera math: the parallel projection, the zoom, and the camera placements.

import assert from "node:assert/strict";
import test from "node:test";

import { clusterZoom, emptyCam, focusCam, project, READ_K, viewOf, zoomAt } from "../src/layout/camera.ts";

const geometry = (deckX: number) => ({ deckX, deckIn: 26, deckZ: 170, focusDz: 30, boxH: 28, gh: 32 });

test("the projection keeps verticals vertical at any pitch and height", () => {
  const view = viewOf(1440, 900, 262, 320, 760);
  const cam = { k: 0.8, tilt: 15, Fx: 300, Fy: 400 };
  assert.equal(project(cam, view, 120, 10)[0], project(cam, view, 120, 900, 170)[0]);
});

test("a zoom keeps the world point under the pointer in place", () => {
  const view = viewOf(1440, 900, 262, 320, 760);
  const cam = { k: 0.8, tilt: 15, Fx: 300, Fy: 400 };
  const zoomed = zoomAt(cam, view, 1.5, 700, 300);
  const x = cam.Fx + (700 - view.cx) / cam.k;
  assert.ok(Math.abs(project(cam, view, x, 0)[0] - project(zoomed, view, x, 0)[0]) < 1e-6);
});

test("the rest camera is flat, and a tall deck does not zoom below the reading floor", () => {
  const view = viewOf(1440, 900, 262, 320, 760);
  assert.equal(emptyCam(view, 900).tilt, 0);
  assert.ok(clusterZoom(view, { x: 400, y: 0, w: 400, h: 5000 }, 15, 170, 30) >= Math.min(READ_K, 1));
});

test("the focus camera looks between the slot and its box", () => {
  const view = viewOf(1440, 900, 262, 320, 760);
  const focused = focusCam({ k: 0.8, tilt: 15, Fx: 300, Fy: 400 }, view, { x: 100, y: 500, gw: 80, lw: 120 },
    { x: 400, y: 460, w: 380, h: 200 }, geometry(400));
  assert.ok(focused.Fx > 100 && focused.Fx < 400 + 26 + 120);
});

test("the focus camera keeps the deck's right edge left of the map area", () => {
  const view = viewOf(1024, 768, 228, 282, 630);
  const deck = { x: 600, y: 400, w: 400, h: 300 };
  const cam = focusCam({ k: 0.56, tilt: 15, Fx: 0, Fy: 0 }, view, { x: 80, y: 500, gw: 120, lw: 200 }, deck, geometry(600));
  assert.ok(project(cam, view, deck.x + deck.w, deck.y)[0] <= view.right - 12 + 1e-6);
});

// Tests for the connector paths: the guide, the tether, and the arrow.

import assert from "node:assert/strict";
import test from "node:test";

import { arrowPath, guidePath, ortho, tetherPath } from "../src/layout/paths.ts";

test("the guide runs straight to a level file node and takes one elbow otherwise", () => {
  assert.equal(guidePath(200, 300, { left: 330, top: 290, height: 20 }, 1), "M200.0,300.0 H330.0");
  const bent = guidePath(200, 300, { left: 330, top: 190, height: 20 }, 1);
  assert.ok(bent.startsWith("M200.0,300.0") && bent.includes("Q") && bent.endsWith("L330.0,200.0"));
});

test("an orthogonal path rounds each corner with at most 4 px", () => {
  assert.equal(ortho([[0, 0], [10, 0], [10, 10]]), "M0.0,0.0 L6.0,0.0 Q10.0,0.0 10.0,4.0 L10.0,10.0");
});

test("the tether leaves the slot level past the widest row, then curves into the lid", () => {
  const d = tetherPath({ x: 400, y: 300 }, 450, { x: 620, y: 240 });
  assert.ok(d.startsWith("M400.0,300.0 L460.0,300.0 C"));
  assert.ok(d.endsWith("620.0,240.0"));
});

test("the arrow leaves the lid past the deck edge, curves into the code header, and hides when there is no room", () => {
  const arrow = arrowPath({ x: 800, y: 240 }, 900, { x: 1010, y: 280 });
  assert.equal(arrow.ok, true);
  assert.ok(arrow.d.startsWith("M800.0,240.0 L912.0,240.0 C"));
  assert.equal(arrow.head, "M1010.0,280.0 L1001.0,275.5 L1001.0,284.5 z");
  assert.equal(arrowPath({ x: 1000, y: 240 }, 1005, { x: 1010, y: 280 }).ok, false);
});

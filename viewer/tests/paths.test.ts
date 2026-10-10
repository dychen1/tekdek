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

test("the guide elbow at 60% zoom places its gutter 8.4 px left of the box", () => {
  assert.equal(
    guidePath(200, 300, { left: 330, top: 190, height: 20 }, 0.6),
    "M200.0,300.0 L317.6,300.0 Q321.6,300.0 321.6,296.0 L321.6,204.0 Q321.6,200.0 325.6,200.0 L330.0,200.0",
  );
});

test("a tether whose lid is close to the rows bends at the lid margin", () => {
  assert.equal(tetherPath({ x: 400, y: 300 }, 450, { x: 480, y: 240 }), "M400.0,300.0 L440.0,300.0 C462.0,300.0 458.0,240.0 480.0,240.0");
});

test("a tether whose lid is left of the slot curves back from the slot", () => {
  assert.equal(tetherPath({ x: 400, y: 300 }, 450, { x: 350, y: 240 }), "M400.0,300.0 L400.0,300.0 C372.5,300.0 377.5,240.0 350.0,240.0");
});

test("an arrow whose header is close to the deck turns 60 px before the header", () => {
  const arrow = arrowPath({ x: 800, y: 240 }, 900, { x: 950, y: 280 });
  assert.equal(arrow.d, "M800.0,240.0 L890.0,240.0 C923.0,240.0 917.0,280.0 950.0,280.0");
  assert.equal(arrow.head, "M950.0,280.0 L941.0,275.5 L941.0,284.5 z");
  assert.equal(arrow.ok, true);
});

test("an arrow whose header is near the slot starts its curve at the slot", () => {
  const arrow = arrowPath({ x: 800, y: 240 }, 900, { x: 830, y: 280 });
  assert.equal(arrow.d, "M800.0,240.0 L800.0,240.0 C816.5,240.0 813.5,280.0 830.0,280.0");
  assert.equal(arrow.ok, true);
});

test("an arrow needs more than 24 px of room to show", () => {
  assert.equal(arrowPath({ x: 800, y: 240 }, 900, { x: 824, y: 280 }).ok, false);
  assert.equal(arrowPath({ x: 800, y: 240 }, 900, { x: 825, y: 280 }).ok, true);
});

test("a guide row below the frame puts its gutter 8 px left of the frame", () => {
  const frame = { left: 244, top: 52, bottom: 630 };
  assert.equal(
    guidePath(200, 700, { left: 330, top: 290, height: 20 }, 1, frame),
    "M200.0,700.0 L232.0,700.0 Q236.0,700.0 236.0,696.0 L236.0,304.0 Q236.0,300.0 240.0,300.0 L330.0,300.0",
  );
});

test("a guide row above the frame puts its gutter 8 px left of the frame", () => {
  const frame = { left: 244, top: 52, bottom: 630 };
  assert.equal(
    guidePath(200, 30, { left: 330, top: 290, height: 20 }, 1, frame),
    "M200.0,30.0 L232.0,30.0 Q236.0,30.0 236.0,34.0 L236.0,296.0 Q236.0,300.0 240.0,300.0 L330.0,300.0",
  );
});

test("a guide row outside the frame keeps its gutter left of the frame at 60% zoom", () => {
  const frame = { left: 244, top: 52, bottom: 630 };
  assert.equal(
    guidePath(135, 721, { left: 274, top: 505, height: 12 }, 0.6, frame),
    "M135.0,721.0 L232.0,721.0 Q236.0,721.0 236.0,717.0 L236.0,515.0 Q236.0,511.0 240.0,511.0 L274.0,511.0",
  );
});

test("a guide row inside the frame range keeps the gutter at the file box", () => {
  const frame = { left: 244, top: 52, bottom: 630 };
  const node = { left: 330, top: 290, height: 20 };
  const inside = "M200.0,500.0 L312.0,500.0 Q316.0,500.0 316.0,496.0 L316.0,304.0 Q316.0,300.0 320.0,300.0 L330.0,300.0";
  assert.equal(guidePath(200, 500, node, 1, frame), inside);
  assert.equal(guidePath(200, 500, node, 1), inside);
  assert.equal(guidePath(200, 630, node, 1, frame), guidePath(200, 630, node, 1));
  assert.equal(guidePath(200, 52, node, 1, frame), guidePath(200, 52, node, 1));
});

test("a guide to a level file node stays straight whatever the frame", () => {
  assert.equal(guidePath(200, 300, { left: 330, top: 290, height: 20 }, 1, { left: 244, top: 52, bottom: 630 }), "M200.0,300.0 H330.0");
});

test("a guide row that ends right of the frame gutter never turns back", () => {
  const d = guidePath(240, 700, { left: 330, top: 290, height: 20 }, 1, { left: 244, top: 52, bottom: 630 });
  assert.ok(d.startsWith("M240.0,700.0 L242.0,700.0 Q244.0,700.0 244.0,698.0"));
});

// Tests for the deck geometry: box widths, the deck size, the pinned spot, file box placement, and the starting zoom.

import assert from "node:assert/strict";
import test from "node:test";

import { boxWidth, DECK_Z, deckHeight, deckSize, deckSpot, fileBoxCenter, home, MIN_DECK_W, SIN, startZoom } from "../src/layout/deck-geometry.ts";

const close = (actual: number, expected: number) => assert.ok(Math.abs(actual - expected) < 0.01, `${actual} != ${expected}`);
const frame = { left: 290, top: 52, width: 700, height: 738 };

test("a box fits its name and step numbers, and the file path when it lives in another file", () => {
  assert.equal(boxWidth("freeze", "src/flow/import.rs", [18], false), 89);
  assert.equal(boxWidth("freeze", "src/flow/import.rs", [18], true), 161);
});

test("the deck takes the widest cluster's width and the tallest cluster's height, once", () => {
  const small = [{ name: "a", file: "a.rs", steps: [1] }];
  assert.deepEqual(deckSize([small]), { width: MIN_DECK_W, maxHeight: deckHeight(1) });
  const wide = [{ name: "a_very_long_function_name_for_a_box", file: "src/some/deep/folder/file.rs", steps: [1, 2, 3] }];
  const size = deckSize([small, wide, [...small, ...small, ...small]]);
  assert.ok(size.width > MIN_DECK_W);
  assert.equal(size.maxHeight, deckHeight(3));
  assert.deepEqual(home(2), [26, 56 + 80]);
});

test("the deck pins 120 px below the frame top and 90 px before the code column, and moves up on a short frame", () => {
  const spot = deckSpot(frame, 1008, 300, deckHeight(7));
  close(spot.x, 1008 - 90 - 12 - 300);
  close(spot.y, 52 + 120 + DECK_Z * SIN);
  const short = deckSpot({ ...frame, height: 300 }, 1008, 300, deckHeight(7));
  close(short.y, 52 + 4 + DECK_Z * SIN);
});

test("the file box stays level with its row unless it would leave the frame", () => {
  assert.equal(fileBoxCenter(300, frame, 200, 1), 300);
  assert.equal(fileBoxCenter(10, frame, 200, 1), frame.top + 18);
  assert.equal(fileBoxCenter(760, frame, 400, 1), frame.top + frame.height - 8 - 400 + 10);
});

test("a narrow canvas starts at the largest zoom that fits, down to 60%", () => {
  assert.equal(startZoom(290, 1000, 300), 1);
  assert.equal(startZoom(290, 606, 400), 0.6);
  assert.equal(startZoom(290, 650, 400), 0.7);
  assert.equal(startZoom(290, 300, 400), 0.6);
});

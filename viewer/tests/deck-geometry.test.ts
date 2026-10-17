// Tests for the deck geometry: box widths, the deck size, the pinned spot, file box placement, and the zoom range.

import assert from "node:assert/strict";
import test from "node:test";

import { boxWidth, COS, DECK_Z, deckBelow, deckHeight, deckShift, deckSize, deckSpot, deckTransform, fileBoxCenter, fileRoom, home, MIN_DECK_W, SHADOW, SIN, zoomFit, ZOOM_FLOOR } from "../src/layout/deck-geometry.ts";

const close = (actual: number, expected: number) => assert.ok(Math.abs(actual - expected) < 0.01, `${actual} != ${expected}`);
const frame = { left: 290, top: 52, width: 700, height: 738 };

test("a box fits its name and step numbers, and the file name of its path when it lives in another file", () => {
  assert.equal(boxWidth("freeze", "src/flow/import.rs", [18], false), 90);
  // the lid cuts the path from its start: the box makes room for "…/import.rs"
  assert.equal(boxWidth("freeze", "src/flow/import.rs", [18], true), 121);
  assert.equal(boxWidth("freeze", "src/flow/import.rs", [18], true), boxWidth("freeze", "…/import.rs", [18], true));
  assert.equal(boxWidth("x", "import.rs", [18], true), boxWidth("x", "…/import.rs", [18], true) - 13);
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
  const spot = deckSpot(frame, 1008, 300, deckHeight(7), 1);
  close(spot.x, 1008 - 90 - 12 - 300);
  close(spot.y, 52 + 120 + DECK_Z * SIN);
  const short = deckSpot({ ...frame, height: 300 }, 1008, 300, deckHeight(7), 1);
  close(short.y, 52 + 4 + DECK_Z * SIN);
});

test("a zoomed deck keeps its right edge and its top edge on screen", () => {
  const spot = deckSpot(frame, 1008, 300, deckHeight(7), 0.6);
  close(spot.x + 300 * 0.6, 1008 - 90 - 12);
  close(spot.y - DECK_Z * SIN * 0.6, 52 + 120);
});

test("the deck pins to the free room when the frame is between the floor and the top offset", () => {
  const mid = { ...frame, height: 450 };
  const need = deckHeight(7) * COS + DECK_Z * SIN + SHADOW + 8;
  assert.ok(mid.height - need > 4 && mid.height - need < 120, `room ${mid.height - need} must fall between 4 and 120`);
  const spot = deckSpot(mid, 1008, 300, deckHeight(7), 1);
  close(spot.y, mid.top + (mid.height - need) + DECK_Z * SIN);
});

test("the deck transform doubles the offset from the anchor at half zoom", () => {
  assert.equal(deckTransform({ x: 600.3, y: 200 }, { x: 300, y: 100 }, 0.5), "translate(900.6px,300.0px) rotateX(15deg)");
});

test("the file box stays level with its row unless it would leave the frame", () => {
  assert.equal(fileBoxCenter(300, frame, 200, 1), 300);
  assert.equal(fileBoxCenter(10, frame, 200, 1), frame.top + 18);
  assert.equal(fileBoxCenter(760, frame, 400, 1), frame.top + frame.height - 8 - 400 + 10);
  // at any zoom the moved box ends 8 px above the frame's bottom edge
  const y = fileBoxCenter(760, frame, 400, 0.75);
  close(y + (400 - 10) * 0.75, frame.top + frame.height - 8);
});

test("the zoom starts at the largest tenth that fits, from the floor to 100%", () => {
  const deck = { width: 300, maxHeight: deckHeight(7) };
  assert.equal(zoomFit(frame, 1008, deck, 100, 100).start, 1);
  assert.equal(zoomFit(frame, 1008, deck, 300, 200).start, 0.9);
  assert.equal(zoomFit(frame, 1008, deck, 500, 200).start, ZOOM_FLOOR);
});

test("at the largest zoom the widest file box ends 40 px before the deck, and the deck fits the frame", () => {
  const deck = { width: 300, maxHeight: deckHeight(7) };
  for (const fileRight of [100, 300]) {
    const { max } = zoomFit(frame, 1008, deck, fileRight, fileRight);
    const spot = deckSpot(frame, 1008, deck.width, deck.maxHeight, max);
    close(fileRoom(frame.left, spot.x, max), fileRight);
    assert.ok(spot.y - DECK_Z * SIN * max >= frame.top + 4);
  }
  // under the floor the zoom stops at the floor, and the narrowest rows still fit the room that is left
  const narrow = zoomFit(frame, 1008, deck, 500, 200);
  assert.equal(narrow.max, ZOOM_FLOOR);
  const room = fileRoom(frame.left, deckSpot(frame, 1008, deck.width, deck.maxHeight, ZOOM_FLOOR).x, ZOOM_FLOOR);
  assert.ok(room < 500 && room >= 200, `room ${room}`);
});

test("the floor gives way where even the narrowest rows would reach the deck", () => {
  const deck = { width: 300, maxHeight: deckHeight(7) };
  const { start, max } = zoomFit(frame, 1008, deck, 900, 500);
  close(max, 546 / 800);
  assert.equal(start, max);
  close(fileRoom(frame.left, deckSpot(frame, 1008, deck.width, deck.maxHeight, max).x, max), 500);
});

test("a short frame stops the zoom where the deck still fits its height", () => {
  const short = { ...frame, height: 300 };
  const deck = { width: 300, maxHeight: deckHeight(7) };
  const { start, max } = zoomFit(short, 1008, deck, 100, 100);
  close(max, (300 - 12) / (deckHeight(7) * COS + DECK_Z * SIN + SHADOW));
  assert.equal(start, max);
});

test("past the width fit the deck moves right, so the widest file box still ends 40 px before it", () => {
  const deck = { width: 300, maxHeight: deckHeight(7) };
  const { wide } = zoomFit(frame, 1008, deck, 300, 300);
  assert.equal(deckShift(frame, 1008, wide, wide * 0.9), 0);
  for (const k of [wide * 1.2, 1.5]) {
    const spot = deckSpot(frame, 1008 + deckShift(frame, 1008, wide, k), deck.width, deck.maxHeight, k);
    close(fileRoom(frame.left, spot.x, k), 300);
  }
});

test("the deck reaches below the frame only where its height no longer fits", () => {
  const depth = deckHeight(7) * COS + DECK_Z * SIN + SHADOW;
  assert.equal(deckBelow(depth + 8, deckHeight(7), 1), 0);
  close(deckBelow(300, deckHeight(7), 1.5), depth * 1.5 + 8 - 300);
  const spot = deckSpot({ ...frame, height: 300 }, 1008, 300, deckHeight(7), 1.5);
  close(spot.y - DECK_Z * SIN * 1.5 + depth * 1.5 - (frame.top + 300 - 4), deckBelow(300, deckHeight(7), 1.5));
});

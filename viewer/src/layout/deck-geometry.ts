/* Deck geometry. This module owns the sizes and screen places of the file box and the pinned cluster deck.
   It is pure math: it reads rectangles and returns numbers or a CSS transform.
   # Sections
   constants, boxes and deck, placement */
import { clamp, f1, mono } from "./util.ts";

// ============ constants ============

const TILT = 15;
export const COS = Math.cos(TILT * Math.PI / 180), SIN = Math.sin(TILT * Math.PI / 180);
/** The deck's height above the base plane. The deck camera passes it to CSS as `--deck-z`. */
export const DECK_Z = 170;
const DECK_IN = 26, BAND = 56, PITCH = 40;
export const SHADOW = 12;
/** The pinned deck: its top edge sits DECK_TOP below the frame top, and its right edge leaves ARROW_GAP for the arrow's curve. */
const DECK_TOP = 120, ARROW_GAP = 90;
export const MIN_DECK_W = 272;
/** The file box's left edge, from the frame's left edge. */
export const FILE_X = 30;

// ============ boxes and deck ============

/** The width of a lifted box: its name (or its file path when it lives in another file) and its step numbers. */
export const boxWidth = (name: string, file: string, steps: readonly number[], away: boolean) =>
  Math.round(17 + Math.max(mono(name.length, 11.5), away ? mono(file.length, 10.5) : 0) + 10 + mono(steps.join(" ").length, 10.5) + 8);

export const deckHeight = (members: number) => BAND + members * PITCH + 4;

/** One width and the tallest height over every cluster, so the pinned deck never changes width and always fits. */
export function deckSize(clusters: readonly (readonly { name: string; file: string; steps: readonly number[] }[])[]): { width: number; maxHeight: number } {
  let width = MIN_DECK_W, maxHeight = 0;
  for (const members of clusters) {
    if (!members.length) continue;
    width = Math.max(width, DECK_IN + Math.max(...members.map(m => boxWidth(m.name, m.file, m.steps, true))) + 44);
    maxHeight = Math.max(maxHeight, deckHeight(members.length));
  }
  return { width, maxHeight };
}

/** The deck-plane place of the i-th box. */
export const home = (i: number): [number, number] => [DECK_IN, BAND + i * PITCH];

// ============ placement ============

export interface Frame { left: number; top: number; width: number; height: number }

/** The pinned deck spot on screen: one top edge and one left edge for every step and cluster. */
export function deckSpot(frame: Frame, codeLeft: number, deckWidth: number, maxHeight: number): { x: number; y: number } {
  const need = maxHeight * COS + DECK_Z * SIN + SHADOW + 8;
  const top = frame.top + clamp(DECK_TOP, 4, Math.max(4, frame.height - need));
  return { x: codeLeft - ARROW_GAP - 12 - deckWidth, y: top + DECK_Z * SIN };
}

/** The deck's CSS transform inside the canvas, which scales by k around the anchor, so the deck stays at its spot. */
export const deckTransform = (spot: { x: number; y: number }, anchor: { x: number; y: number }, k: number) =>
  `translate(${f1(anchor.x + (spot.x - anchor.x) / k)}px,${f1(anchor.y + (spot.y - anchor.y) / k)}px) rotateX(${TILT}deg)`;

/** The screen y of the file node's center: level with its explorer row, unless the box would leave the frame. */
export function fileBoxCenter(rowY: number, frame: Frame, fileHeight: number, k: number): number {
  const lo = frame.top + 18, hi = Math.max(lo, frame.top + frame.height - 8 - fileHeight * k + 10);
  return clamp(rowY, lo, hi);
}

/** The largest zoom, in 10% steps from 60% to 100%, at which the widest flow file box ends 40 px before the deck. */
export function startZoom(frameLeft: number, deckLeft: number, fileRight: number): number {
  const k = (deckLeft - frameLeft - FILE_X - 40) / Math.max(1, fileRight);
  return clamp(Math.floor(k * 10 + 1e-9) / 10, 0.6, 1);
}

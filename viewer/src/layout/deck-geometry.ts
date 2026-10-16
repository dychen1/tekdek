/* Deck geometry. This module owns the sizes and screen places of the file box and the pinned cluster deck.
   It is pure math: it reads rectangles and returns numbers or a CSS transform.
   # Sections
   constants, boxes and deck, placement */
import { clamp, f1, FS_CODE, FS_CODE_L, mono } from "./util.ts";

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
/** The file box rows end CLEAR px before the deck, so the tether runs level out of the rows before it curves.
 *  `paths.ts` hides a tether whose curve gets less room. */
export const CLEAR = 40;
/** The zoom range. The floor keeps 11 px map text at 8 px or more on screen. */
export const ZOOM_MIN = 0.5, ZOOM_MAX = 1.5, ZOOM_FLOOR = 0.75;

// ============ boxes and deck ============

/** The characters of a file path that a lid makes room for: "…/" and the file name, or a shorter whole path.
 *  The lid cuts a longer path from its start, so the deck stays narrow and the file name shows. */
const lidPathLength = (file: string) => Math.min(file.length, file.length - file.lastIndexOf("/") + 1);

/** The width of a lifted box: its name (or the end of its file path when it lives in another file) and its step numbers. */
export const boxWidth = (name: string, file: string, steps: readonly number[], away: boolean) =>
  Math.round(17 + Math.max(mono(name.length, FS_CODE_L), away ? mono(lidPathLength(file), FS_CODE) : 0) + 10 + mono(steps.join(" ").length, FS_CODE) + 8);

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

/** The deck's screen height at 100%: from its raised top edge down to its cast shadow. */
const deckDepth = (maxHeight: number) => maxHeight * COS + DECK_Z * SIN + SHADOW;

/** The pinned deck spot on screen at zoom k. The right edge and the top edge stay put; the deck grows left and down. */
export function deckSpot(frame: Frame, codeLeft: number, deckWidth: number, maxHeight: number, k: number): { x: number; y: number } {
  const need = deckDepth(maxHeight) * k + 8;
  const top = frame.top + clamp(DECK_TOP, 4, Math.max(4, frame.height - need));
  return { x: codeLeft - ARROW_GAP - 12 - deckWidth * k, y: top + DECK_Z * SIN * k };
}

/** The room for the file box rows left of the deck, in box pixels from the box's left edge. */
export const fileRoom = (frameLeft: number, deckLeft: number, k: number) => Math.max(0, (deckLeft - CLEAR - frameLeft - FILE_X) / k);

/** The deck's CSS transform inside the canvas, which scales by k around the anchor, so the deck stays at its spot. */
export const deckTransform = (spot: { x: number; y: number }, anchor: { x: number; y: number }, k: number) =>
  `translate(${f1(anchor.x + (spot.x - anchor.x) / k)}px,${f1(anchor.y + (spot.y - anchor.y) / k)}px) rotateX(${TILT}deg)`;

/** The screen y of the file node's center: level with its explorer row, unless the box would leave the frame. */
export function fileBoxCenter(rowY: number, frame: Frame, fileHeight: number, k: number): number {
  const lo = frame.top + 18, hi = Math.max(lo, frame.top + frame.height - 8 - fileHeight * k + 10);
  return clamp(rowY, lo, hi);
}

/** The zoom range of a frame. `max` is the largest zoom at which the deck fits the frame height and the widest flow
 *  file box (`fileRight`) ends CLEAR px before the deck, but not under the floor: there the names end in an ellipsis.
 *  The floor gives way where even the narrowest rows (`fileMin`) would reach the deck.
 *  `start` is that fit in 10% steps, from the floor to 100%. */
export function zoomFit(frame: Frame, codeLeft: number, deck: { width: number; maxHeight: number }, fileRight: number, fileMin: number): { start: number; max: number } {
  const room = codeLeft - ARROW_GAP - 12 - CLEAR - frame.left - FILE_X;
  const side = room / Math.max(1, fileRight + deck.width);
  const floor = Math.min(ZOOM_FLOOR, room / Math.max(1, fileMin + deck.width));
  const tall = (frame.height - 12) / deckDepth(deck.maxHeight);
  const max = clamp(Math.min(tall, Math.max(floor, side)), ZOOM_MIN, ZOOM_MAX);
  return { start: Math.min(max, 1, Math.max(floor, Math.floor(side * 10 + 1e-9) / 10)), max };
}

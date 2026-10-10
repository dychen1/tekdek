/* Connector paths. This module owns the SVG path data of the guide, the tether, and the arrow.
   Inputs are screen points from the rectangles of the two ends; it never reads the DOM. */
import { f1 } from "./util.ts";

export type Pt = [number, number];
const P2 = (x: number, y: number) => `${f1(x)},${f1(y)}`;

/** An orthogonal path with 4 px elbow corners. */
export function ortho(pts: Pt[]): string {
  let d = `M${P2(...pts[0])}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const [ax, ay] = pts[i - 1], [bx, by] = pts[i], [cx, cy] = pts[i + 1];
    const r = Math.min(4, Math.hypot(bx - ax, by - ay) / 2, Math.hypot(cx - bx, cy - by) / 2);
    const ux = Math.sign(bx - ax), uy = Math.sign(by - ay), vx = Math.sign(cx - bx), vy = Math.sign(cy - by);
    d += ` L${P2(bx - ux * r, by - uy * r)} Q${P2(bx, by)} ${P2(bx + vx * r, by + vy * r)}`;
  }
  return d + ` L${P2(...pts[pts.length - 1])}`;
}

/** The guide: straight across to a level file node, else one tree elbow in the gutter left of the box. */
export function guidePath(sx: number, sy: number, node: { left: number; top: number; height: number }, k: number): string {
  const ny = node.top + node.height / 2;
  if (Math.abs(sy - ny) <= node.height / 2 - 1) return `M${P2(sx, sy)} H${f1(node.left)}`;
  const gx = node.left - 14 * k;
  return ortho([[sx, sy], [gx, sy], [gx, ny], [node.left, ny]]);
}

/** One S curve with level ends from a level lead-out point to the end. */
const sCurve = (a: { x: number; y: number }, tx: number, b: { x: number; y: number }) => {
  const dx = b.x - tx;
  return `M${P2(a.x, a.y)} L${P2(tx, a.y)} C${P2(tx + dx * 0.55, a.y)} ${P2(b.x - dx * 0.55, b.y)} ${P2(b.x, b.y)}`;
};

/** The tether: level out of the slot past the widest row, then one S into the lid. */
export function tetherPath(a: { x: number; y: number }, rowsRight: number, b: { x: number; y: number }): string {
  const mx = Math.min(b.x - 40, Math.max(a.x, rowsRight) + 10);
  return sCurve(a, Math.max(a.x, mx), b);
}

/** The arrow: out of the lid past the deck's right edge, then one S into the code header. Hidden under 24 px of room. */
export function arrowPath(a: { x: number; y: number }, deckRight: number, b: { x: number; y: number }): { d: string; head: string; ok: boolean } {
  const mx = Math.max(a.x, Math.min(deckRight + 12, b.x - 60));
  return {
    d: sCurve(a, mx, b),
    head: `M${P2(b.x, b.y)} L${P2(b.x - 9, b.y - 4.5)} L${P2(b.x - 9, b.y + 4.5)} z`,
    ok: b.x - a.x > 24,
  };
}

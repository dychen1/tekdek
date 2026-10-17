/* Deck links. This module owns the three connectors, drawn in screen space from the on-screen rectangles
   of the two elements that each one joins: the guide (explorer row to file node), the tether (current slot to
   current lid), and the arrow (current lid to code header). Each end touches its element. A connector whose map end
   lies outside the canvas frame hides: the frame clips the map past its fit. */
import { arrowPath, guidePath, tetherPath } from "../layout/paths.ts";
import { f1 } from "../layout/util.ts";
import { $, type DeckContext, type Links } from "./context.ts";

export function createLinks(ctx: DeckContext): Links {
  const { state } = ctx;
  const guideG = $("#guideG"), guideEl = $("#guidePath");
  const tetherG = $("#tetherG"), tetherEl = $("#tetherPath"), tetherDot = $("#tetherDot");
  const arrowG = $("#arrowG"), arrEl = $("#arrPath"), arrHead = $("#arrHead"), arrDot = $("#arrDot");
  const sbTree = $("#sbTree"), codeHead = $("#codeHead"), frameEl = $("#frame"), svg = $("#links");
  /** Hides every connector. */
  const hideAll = () => { for (const g of [guideG, tetherG, arrowG]) g.classList.add("away"); };
  /** True when a screen point lies inside the frame's rectangle. */
  const inFrame = (p: { x: number; y: number }, fr: DOMRect) => p.x >= fr.left && p.x <= fr.right && p.y >= fr.top && p.y <= fr.bottom;

  /** Draws the guide from the end of the explorer row straight across to the file node. */
  function drawGuide(fr: DOMRect): boolean {
    const row = ctx.parts.explorer.anchorEl(), node = ctx.parts.fileBox.nodeEl();
    if (!row || !node) return false;
    const n = node.getBoundingClientRect();
    if (!inFrame({ x: n.left, y: n.top + n.height / 2 }, fr)) return false;
    const tree = sbTree.getBoundingClientRect(), rr = row.getBoundingClientRect(), y = rr.top + rr.height / 2;
    if (y < tree.top || y > tree.bottom) return false;
    const ends = [".snm", ".dots", ".dnote"].map(q => row.querySelector(q)).filter((el): el is Element => !!el && el.getBoundingClientRect().width > 0);
    if (!ends.length) return false;
    const sx = Math.max(...ends.map(el => el.getBoundingClientRect().right)) + 6;
    guideEl.setAttribute("d", guidePath(sx, y, n, state.zoom, fr));
    return true;
  }

  /** Draws the tether from the current slot to the current lid, level out past the widest row, then one S curve. */
  function drawTether(lid: HTMLElement | null, fr: DOMRect): boolean {
    const slot = ctx.parts.fileBox.slotEl();
    if (!lid || !slot) return false;
    const s = slot.getBoundingClientRect(), l = lid.getBoundingClientRect();
    const a = { x: s.right, y: s.top + s.height / 2 }, b = { x: l.left, y: l.top + l.height / 2 };
    if (!inFrame(a, fr) || !inFrame(b, fr)) return false;
    const tether = tetherPath(a, ctx.parts.fileBox.rowsRight(), b);
    tetherEl.setAttribute("d", tether.d);
    tetherDot.setAttribute("cx", f1(a.x)); tetherDot.setAttribute("cy", f1(a.y));
    return tether.ok;
  }

  /** Draws the arrow from the current lid out past the deck's edge, then one S curve into the code header. */
  function drawArrow(lid: HTMLElement | null, fr: DOMRect): boolean {
    if (!lid) return false;
    const l = lid.getBoundingClientRect(), h = codeHead.getBoundingClientRect();
    const a = { x: l.right, y: l.top + l.height / 2 }, b = { x: h.left, y: h.top + h.height / 2 };
    if (!inFrame(a, fr)) return false;
    const arrow = arrowPath(a, ctx.parts.clusterDeck.deckRight(), b);
    arrEl.setAttribute("d", arrow.d);
    arrHead.setAttribute("d", arrow.head);
    arrDot.setAttribute("cx", f1(a.x)); arrDot.setAttribute("cy", f1(a.y));
    return arrow.ok;
  }

  function draw() {
    if (state.full) { hideAll(); return; }
    // the layer scrolls with the page: its view box starts at its own screen position, so screen points draw in place
    const o = svg.getBoundingClientRect();
    svg.setAttribute("viewBox", `${f1(o.left)} ${f1(o.top)} ${f1(o.width)} ${f1(o.height)}`);
    const lid = ctx.parts.clusterDeck.currentLid(), fr = frameEl.getBoundingClientRect();
    guideG.classList.toggle("away", !drawGuide(fr));
    tetherG.classList.toggle("away", !drawTether(lid, fr));
    arrowG.classList.toggle("away", !drawArrow(lid, fr));
  }

  return { draw, destroy: hideAll };
}

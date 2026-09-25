/* Deck connectors. This module owns every line on the map: the tethers from slots to boxes, the static call
   arcs of the active cluster, and the arrow from the focus box to the extracted stack.
   Every connector is drawn in screen space, once per frame, from the on-screen rects of the nodes it joins.
   A rect already holds the camera pitch, zoom and pan, the lift and its animation, the box fold, and the stack scroll.
   Rule: a tether runs from the slot's right edge to the box lid's left edge; a call arc runs from right edge to right edge;
   the stack arrow runs from the focus lid's right edge to the focus stack box's left edge. Arrowhead tips sit on the end point.
   # Sections
   edges, drawing, stack arrow */
import { clamp, f1 } from "../layout/camera.ts";
import { $, svgEl, type DeckContext } from "./context.ts";

export interface Connectors {
  /** Builds the edges of the current step: calls between members join box edges on the deck,
   *  and calls from the focus to outside rows join its slot to named rows on the tree. */
  render(): void;
  show(on: boolean): void;
  setArrow(on: boolean): void;
  /** Draws every connector from the current on-screen geometry. The frame loop calls it. */
  draw(): void;
  destroy(): void;
}

interface Edge { el: SVGPathElement; a: Element; b: Element; dot?: SVGCircleElement }
type Point = [number, number];

const rectOf = (el: Element) => el.getBoundingClientRect();
const leftMid = (b: DOMRect): Point => [b.left, b.top + b.height / 2];
const rightMid = (b: DOMRect): Point => [b.right, b.top + b.height / 2];

/** A call arc: out to the right of both ends and of xr (the widest box between them), then back in to the target edge. */
function arcD([x1, y1]: Point, [x2, y2]: Point, xr: number, s: number) {
  const xm = Math.max(x1, x2, xr) + s * 24 + Math.abs(y2 - y1) * 0.2;
  return `M${f1(x1)},${f1(y1)} C${f1(xm)},${f1(y1)} ${f1(xm)},${f1(y2)} ${f1(x2)},${f1(y2)}`;
}

export function createConnectors(ctx: DeckContext): Connectors {
  const { model, state } = ctx;
  const { rows, rowById, memberRows, CL } = model;
  const links = $("#links"), baseG = $<SVGGElement>("#baseG"), intG = $<SVGGElement>("#intG");
  const dock = $("#dock"), rcolEl = $(".rcol");
  const arrowLayer = $("#arrowLayer"), arrPath = $("#arrPath"), arrHead = $("#arrHead"), arrDot = $("#arrDot");

  // ============ edges ============

  const edges: { cid: string | null; int: Edge[]; base: Edge[] } = { cid: null, int: [], base: [] };
  function edgePath(g: SVGGElement, cls: string, marker: string, a: Element, b: Element, from: string, to: string): Edge {
    const el = svgEl("path", cls) as SVGPathElement;
    el.setAttribute("marker-end", `url(#${marker})`);
    el.dataset.from = from; el.dataset.to = to;
    g.appendChild(el);
    return { el, a, b };
  }
  function render() {
    intG.replaceChildren(); baseG.replaceChildren();
    edges.cid = null; edges.int = []; edges.base = [];
    rows.forEach(r => r.el.classList.remove("nbfocus"));
    const s = model.steps[state.cur - 1];
    if (!s || !state.layoutCluster) return;
    const C = CL.get(state.layoutCluster)!, focus = s.missing ? null : s.node;
    edges.cid = C.id;
    for (const e of model.internalOf.get(C.id)!) {
      const a = rowById.get(e.source)!, b = rowById.get(e.target)!;
      const hot = e.source === focus || e.target === focus;
      edges.int.push(edgePath(intG, "e int" + (hot ? " hot" : ""), "ah-air", a.lidEl!, b.lidEl!, "lid:" + a.id, "lid:" + b.id));
    }
    if (!focus || !C.members.includes(focus)) return;
    const fr = rowById.get(focus)!;
    for (const e of model.callsOf.get(focus) ?? []) {
      const other = e.source === focus ? e.target : e.source;
      if (C.members.includes(other)) continue;
      const o = rowById.get(other);
      if (!o || o.mode === "hidden") continue;
      o.el.classList.add("nbfocus");
      const out = e.source === focus, a = out ? fr : o, b = out ? o : fr;
      const E = edgePath(baseG, "e ext", "ah-edge", a.nodeEl, b.nodeEl, (a === fr ? "slot:" : "node:") + a.id, (b === fr ? "slot:" : "node:") + b.id);
      E.dot = svgEl("circle", "enode") as SVGCircleElement;
      baseG.appendChild(E.dot);
      edges.base.push(E);
    }
  }
  function show(on: boolean) {
    baseG.classList.toggle("show", on);
    intG.classList.toggle("show", on && !!state.liftedCluster && state.liftedCluster === edges.cid);
  }

  // ============ drawing ============

  let linkScale = 0, linksLive = false;
  function drawLinks() {
    const now = performance.now();
    const live = !!state.liftedCluster || memberRows.some(r => r.sinkUntil > now);
    if (!live) {
      if (linksLive) { linksLive = false; memberRows.forEach(r => { r.tetherEl!.style.display = ""; }); }
      return;
    }
    linksLive = true;
    // the camera scale on screen now, mid-transition included; stroke widths follow it within bounds
    const s = clamp(rectOf(ctx.parts.world.frame).width / state.treeW, 0.55, 1.25);
    if (Math.abs(s - linkScale) > 0.01) { linkScale = s; links.style.setProperty("--lk", s.toFixed(3)); }
    const lid = new Map<string, DOMRect>();
    for (const r of memberRows) {
      const op = r.el.classList.contains("up") || r.sinkUntil > now ? +getComputedStyle(r.lidEl!).opacity : 0;
      if (op < 0.01) { r.tetherEl!.style.display = ""; continue; }
      const rect = rectOf(r.lidEl!);
      lid.set(r.id, rect);
      const a = rightMid(rectOf(r.nodeEl)), b = leftMid(rect);
      r.tetherEl!.style.display = "inline";
      r.tetherEl!.style.opacity = op.toFixed(3);
      r.tetherLine!.setAttribute("d", `M${f1(a[0])},${f1(a[1])} L${f1(b[0])},${f1(b[1])}`);
      r.tetherDot!.setAttribute("cx", f1(a[0])); r.tetherDot!.setAttribute("cy", f1(a[1]));
    }
    if (edges.int.length) {
      const lids = CL.get(edges.cid!)!.members.map(id => lid.get(id) ?? rectOf(rowById.get(id)!.lidEl!));
      for (const E of edges.int) {
        const p = rightMid(rectOf(E.a)), q = rightMid(rectOf(E.b));
        const lo = Math.min(p[1], q[1]), hi = Math.max(p[1], q[1]);
        const xr = Math.max(-Infinity, ...lids.filter(l => l.top + l.height / 2 >= lo && l.top + l.height / 2 <= hi).map(l => l.right));
        E.el.setAttribute("d", arcD(p, q, xr, s));
      }
    }
    for (const E of edges.base) {
      const p = rightMid(rectOf(E.a)), q = rightMid(rectOf(E.b));
      E.el.setAttribute("d", arcD(p, q, -Infinity, s));
      E.dot!.setAttribute("cx", f1(p[0])); E.dot!.setAttribute("cy", f1(p[1]));
    }
  }

  // ============ stack arrow ============
  // With the focus box out of view, the arrow starts on the deck's right edge at the box height.
  // With either end out of view (the map, or the stack list scroll), it fades out.

  let arrowOn = false;
  function setArrow(on: boolean) { arrowOn = on; arrowLayer.classList.toggle("on", on); }
  function drawArrow() {
    const s = model.steps[state.cur - 1];
    if (!arrowOn || !state.liftedCluster || !s) return;
    const item = dock.querySelector(".ditem.focus .dbox"), list = dock.querySelector(".dk-list");
    const fr = s.missing ? null : rowById.get(s.node);
    let ok = !!item && !!list && !!fr && !!fr.lidEl;
    if (ok) {
      const { view } = state;
      const ir = rectOf(item!), lr = rectOf(list!), deck = rectOf(ctx.parts.world.slabTop(state.liftedCluster));
      const xMax = rectOf(rcolEl).left - 8;
      const seen = (x: number, y: number) => x >= view.left && x <= xMax && y >= 0 && y <= innerHeight;
      const [ex, ey] = leftMid(ir);
      let [sx, sy] = rightMid(rectOf(fr!.lidEl!)), from = "lid:" + fr!.id;
      if (!fr!.el.classList.contains("up") || !seen(sx, sy)) {
        sx = deck.right; sy = clamp(sy, deck.top + 12, deck.bottom - 12); from = "dtop:" + state.liftedCluster;
      }
      ok = seen(sx, sy) && ey >= lr.top && ey <= lr.bottom && ex - sx > 32;
      if (ok) {
        // straight out along the box row to the deck edge, then one curve into the stack box
        const mx = Math.max(sx, Math.min(deck.right + 10, ex - 48)), dx = ex - mx;
        arrPath.setAttribute("d", `M${f1(sx)},${f1(sy)} L${f1(mx)},${f1(sy)} C${f1(mx + dx * 0.5)},${f1(sy)} ${f1(ex - dx * 0.45)},${f1(ey)} ${f1(ex)},${f1(ey)}`);
        arrHead.setAttribute("d", `M${f1(ex)},${f1(ey)} L${f1(ex - 9)},${f1(ey - 4.5)} L${f1(ex - 9)},${f1(ey + 4.5)} z`);
        arrDot.setAttribute("cx", f1(sx)); arrDot.setAttribute("cy", f1(sy));
        arrPath.dataset.from = from; arrPath.dataset.to = "dbox:" + s.node;
      }
    }
    arrowLayer.classList.toggle("away", !ok);
  }

  return {
    render, show, setArrow,
    draw() { drawLinks(); drawArrow(); },
    destroy() {
      baseG.replaceChildren(); intG.replaceChildren();
      baseG.classList.remove("show"); intG.classList.remove("show");
      arrowLayer.classList.remove("on", "away");
    },
  };
}

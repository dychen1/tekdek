/* Deck world. This module owns the map plane: the tree rows, the lifted boxes, the decks, and the frame.
   It applies the layout from `layout/tree.ts` to the DOM, keeps the flow layout fixed between steps,
   and lifts, drops, and focuses boxes.
   # Sections
   build, layout, fold stops, decks, lift and drop */
import { closeFlowPath, deckColumn, deckRect, DECK_IN, elbow, fileNote, layoutRows, MODES, openFlowPath, type LayoutInput } from "../layout/tree.ts";
import { dockOrder } from "../layout/flow-plan.ts";
import { clamp, type Rect } from "../layout/camera.ts";
import { esc, icon, PTR } from "./panels.ts";
import { $, mk, svgEl, type ClusterView, type DeckContext, type RowView } from "./context.ts";

export interface World {
  /** Lays out every row, the frame, and the active deck from the current state. */
  relayout(): void;
  /** Lays out the tree again after a user fold. The row in `keep` holds its place on screen. */
  refreshTree(keep?: RowView): void;
  /** Opens the flow path once at flow start, or closes what the flow opened at clear. */
  setFlowLayout(on: boolean): void;
  lift(cluster: string): void;
  drop(cluster: string): void;
  /** Marks the focus box, its tether, and the current file for the current step. */
  applyFocus(): void;
  /** The deck rectangle of a cluster; undefined when the cluster has no member in the graph. */
  slabRect(cluster: string): Rect;
  /** The top face of a cluster deck, for the stack arrow. */
  slabTop(cluster: string): HTMLElement;
  /** The dashed frame around the tree. Its on-screen width gives the camera scale. */
  frame: HTMLElement;
  destroy(): void;
}

export function createWorld(ctx: DeckContext): World {
  const { model, state, tokens, signal } = ctx;
  const { rows, rowById, memberRows, sideRows, CL, clusters } = model;
  const world = $("#world"), tetherG = $<SVGGElement>("#tetherG");

  // ============ build ============

  world.replaceChildren();
  world.classList.add("boot");
  const frame = mk("div", "frame", `<div class="frame-title">${esc(model.graph.repo)}<span>${model.graph.files.length} files · ${model.graph.functions.length} functions</span></div>`);
  world.appendChild(frame);
  for (const r of rows) world.appendChild(buildRow(r, CL.get(r.fn?.cluster ?? "") ?? null));

  function buildRow(r: RowView, cluster: ClusterView | null): HTMLDivElement {
    const row = mk("div", "row " + r.kind) as HTMLDivElement;
    r.el = row; row.dataset.id = r.id;
    if (r.parent) {
      r.elbowEl = mk("div", "elbow") as HTMLDivElement;
      r.elbowEl.style.left = (9 - 26) + "px";
      r.elbowEl.style.width = (26 - 12) + "px";
      row.appendChild(r.elbowEl);
    }
    let inner: string;
    if (r.kind === "dir") {
      inner = `<button class="fold" type="button" aria-label="Collapse ${esc(r.id.slice(4))}" aria-expanded="true">${icon("chev")}</button>` +
        `${icon("folder")}<span class="lbl">${esc(r.name)}<span class="dnote"></span></span>`;
    } else if (r.kind === "file") {
      inner = `<button class="fold" type="button" aria-label="Collapse ${esc(r.file)}" aria-expanded="true">${icon("chev")}</button><span class="lbl">${esc(r.name)}<span class="fnote"></span></span>`;
      if (model.clustersByFile.has(r.file!)) row.classList.add("hasm");
    } else inner = `<span class="nm">${esc(r.name)}</span>`;
    r.nodeEl = mk("div", "node", inner) as HTMLDivElement;
    row.appendChild(r.nodeEl);
    if (r.kind !== "fn") { r.noteEl = r.nodeEl.querySelector<HTMLElement>(".fnote, .dnote"); r.foldEl = r.nodeEl.querySelector<HTMLButtonElement>(".fold"); }
    if (r.kind === "fn" && model.plan.visits.has(r.id)) row.classList.add("visited");
    if (r.kind === "fn" && cluster) {
      row.classList.add("m"); row.style.setProperty("--c", cluster.cv);
      row.style.setProperty("--lw", r.lw + "px");
      r.liftEl = mk("div", "lift",
        `<div class="face fshadow"></div><div class="riser"><div class="face front"></div>` +
        `<div class="face lid">${PTR}<button class="tmain" type="button" tabindex="-1" aria-label="${esc(r.name)}"><span class="ln">${esc(r.name)}</span></button>` +
        `<button class="cfold" type="button" tabindex="-1" aria-label="Close box ${esc(r.name)}" aria-expanded="true">${icon("minus")}</button><span class="stepno"></span></div></div>`) as HTMLDivElement;
      r.stepnoEl = r.liftEl.querySelector<HTMLElement>(".stepno")!;
      r.cfoldEl = r.liftEl.querySelector<HTMLButtonElement>(".cfold")!;
      r.tmainEl = r.liftEl.querySelector<HTMLButtonElement>(".tmain")!;
      r.lidEl = r.liftEl.querySelector<HTMLDivElement>(".lid")!;
      row.appendChild(r.liftEl);
      // the tether: a screen-space line from the slot socket to the box
      r.tetherEl = svgEl("g", "tether") as SVGGElement;
      r.tetherEl.style.setProperty("--c", cluster.cv);
      r.tetherLine = svgEl("path") as SVGPathElement; r.tetherLine.dataset.from = "slot:" + r.id; r.tetherLine.dataset.to = "lid:" + r.id;
      r.tetherDot = svgEl("circle") as SVGCircleElement;
      r.tetherEl.append(r.tetherLine, r.tetherDot);
      tetherG.appendChild(r.tetherEl);
    }
    return row;
  }

  // decks: one per cluster that has a member in the graph
  const slabs = new Map<string, { el: HTMLDivElement; top: HTMLElement; rect: Rect }>();
  for (const c of clusters) {
    if (!c.members.length) continue;
    const el = mk("div", "slab") as HTMLDivElement;
    el.dataset.c = c.id;
    el.style.setProperty("--c", c.cv);
    el.innerHTML = `<div class="shadow"></div><div class="deck"><div class="dfront"></div>` +
      `<div class="dtop"><div class="dlabel"><span class="dname">${esc(c.label)}</span><span class="dcount">${c.members.length} ${c.members.length === 1 ? "function" : "functions"}</span></div></div></div>`;
    world.appendChild(el);
    slabs.set(c.id, { el, top: el.querySelector<HTMLElement>(".dtop")!, rect: { x: 0, y: 0, w: 10, h: 10 } });
  }

  // ============ layout ============

  const layoutInput = (): LayoutInput => ({ flow: state.flowOn, layoutCluster: state.layoutCluster, open: state.open, plan: model.plan });

  // Closed directories never hide an active slot or the current step's function.
  function assertSlotsShown(C: ClusterView | null) {
    if (!C) return;
    for (const id of C.members) {
      const r = rowById.get(id)!;
      console.assert(r.mode === "ghost" && r.parent!.mode !== "hidden", `A closed directory hides the slot ${id}.`);
    }
    const s = model.steps[state.cur - 1];
    if (s && !s.missing && s.cluster === C.id) console.assert(rowById.get(s.node)!.mode !== "hidden", `A closed directory hides step ${state.cur}.`);
  }
  // The flow layout is fixed: no row moves between steps. The first flow layout, or a user fold, sets the baseline.
  function assertFlowLayout() {
    if (!state.flowOn) { state.flowYs = null; return; }
    if (!state.flowYs) { state.flowYs = new Map(rows.map(r => [r.id, r.y])); return; }
    for (const r of rows) console.assert(state.flowYs.get(r.id) === r.y, `The row ${r.id} moved between flow steps.`);
  }
  // Deck column: right of every shown row in files that hold cluster members. Rest point = beside the slot.
  function setDeckColumn() {
    state.deckX = deckColumn(rows);
    for (const r of memberRows) {
      r.restX = r.gw + 6;
      r.upX = state.deckX + DECK_IN - r.x;
      if (r.el.classList.contains("up")) setLiftZ(r);
    }
  }

  function relayout() {
    const C = state.layoutCluster ? CL.get(state.layoutCluster)! : null;
    const input = layoutInput();
    state.treeH = layoutRows(rows, input);
    let maxR = 0;
    for (const r of rows) {
      if (r.mode !== "hidden") maxR = Math.max(maxR, r.x + r.cw);
      r.el.style.transform = `translate3d(${r.x}px,${r.y}px,0)`;
      for (const md of MODES) r.el.classList.toggle(md, r.mode === md);
      // A file box grows with its label, which scales up when the camera zooms out.
      r.nodeEl.style.width = r.kind === "file" ? `calc(${r.cw}px * var(--ls, 1))` : r.cw + "px";
      r.nodeEl.style.height = r.h + "px";
      r.nodeEl.style.top = r.top + "px";
      const e = elbow(r);
      if (e && r.elbowEl) { r.elbowEl.style.top = e.top + "px"; r.elbowEl.style.height = e.height + "px"; }
      if (r.kind === "file") {
        const isOpen = state.open.has(r.id);
        r.el.classList.toggle("folded", !isOpen);
        r.foldEl!.setAttribute("aria-expanded", String(isOpen));
        r.foldEl!.setAttribute("aria-label", (isOpen ? "Collapse " : "Expand ") + r.file);
        r.noteEl!.textContent = fileNote(r, input);
      } else if (r.kind === "dir") {
        const isOpen = state.open.has(r.id);
        r.el.classList.toggle("folded", !isOpen);
        r.el.classList.toggle("flowopen", state.open.get(r.id) === "flow");
        r.foldEl!.setAttribute("aria-expanded", String(isOpen));
        r.foldEl!.setAttribute("aria-label", (isOpen ? "Collapse " : "Expand ") + r.id.slice(4));
        r.noteEl!.textContent = isOpen ? "" : r.note!;
      }
      r.el.classList.toggle("buried", r.buried);
      r.el.inert = r.mode === "hidden";
    }
    syncFoldStops();
    assertSlotsShown(C);
    if (state.flowOn && !state.flowYs) setDeckColumn();
    assertFlowLayout();
    // The frame fits the shown rows. During a flow it only grows, so it never jitters between steps.
    const width = Math.max(maxR + 90, state.flowOn && memberRows.length ? state.deckX + DECK_IN + model.maxLw + 110 : 0, 360);
    state.treeW = state.flowOn ? Math.max(state.treeW, width) : width;
    frame.style.width = state.treeW + "px";
    frame.style.height = state.treeH + "px";
    if (C) placeSlab(C.id, !slabs.get(C.id)!.el.classList.contains("up"));
  }

  function refreshTree(keep?: RowView) {
    const y0 = keep ? keep.y : 0;
    state.flowYs = null;
    relayout();
    if (state.liftedCluster) applyFocus();
    if (state.cur) { ctx.parts.connectors.render(); ctx.parts.connectors.show(!!state.liftedCluster); }
    if (keep && keep.y !== y0) { state.cam = { ...state.cam, Fy: state.cam.Fy + keep.y - y0 }; ctx.parts.viewport.apply("layout"); }
  }

  function setFlowLayout(on: boolean) {
    if (on && !state.flowOn) { openFlowPath(state.open, model.plan); state.flowOn = true; }
    if (!on && state.flowOn) { closeFlowPath(state.open); state.flowOn = false; }
  }

  // ============ fold stops ============
  // Map chevrons share one roving tab stop: the last focused chevron, else the first shown one.
  // Up and Down move between shown chevrons; Home and End go to the first and the last.

  let foldStop: RowView | null = null;
  const shownFolds = () => sideRows.filter(r => r.mode !== "hidden");
  function syncFoldStops() {
    if (!foldStop || foldStop.mode === "hidden") foldStop = shownFolds()[0] ?? null;
    for (const r of sideRows) r.foldEl!.tabIndex = r === foldStop ? 0 : -1;
  }
  world.addEventListener("focusin", e => {
    const fold = (e.target as Element).closest(".row .fold");
    if (!fold) return;
    const r = rowById.get((fold.closest(".row") as HTMLElement).dataset.id!)!;
    if (r !== foldStop) { foldStop = r; syncFoldStops(); }
  }, { signal });
  world.addEventListener("keydown", e => {
    const fold = (e.target as Element).closest(".row .fold");
    if (!fold || !["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) return;
    e.preventDefault(); e.stopPropagation();
    const list = shownFolds(), i = list.indexOf(foldStop!);
    const j = e.key === "ArrowDown" ? i + 1 : e.key === "ArrowUp" ? i - 1 : e.key === "Home" ? 0 : list.length - 1;
    const next = list[clamp(j, 0, list.length - 1)];
    if (next) { foldStop = next; syncFoldStops(); next.foldEl!.focus({ preventScroll: true }); }
  }, { signal });

  // ============ decks ============

  function placeSlab(cid: string, instant: boolean) {
    const s = slabs.get(cid)!, R = deckRect(CL.get(cid)!.members.map(id => rowById.get(id)!), state.deckX);
    s.rect = R;
    if (instant) s.el.classList.add("noanim");
    s.el.style.transform = `translate3d(${R.x}px,${R.y}px,0)`;
    s.el.style.width = R.w + "px";
    s.el.style.height = R.h + "px";
    if (instant) { void s.el.offsetWidth; s.el.classList.remove("noanim"); }
  }

  // ============ lift and drop ============

  function setLiftZ(r: RowView) {
    const up = r.el.classList.contains("up");
    r.liftEl!.style.transform = up ? `translate3d(${r.upX}px,0,${tokens.deckZ + 1}px)` : `translate3d(${r.restX}px,0,0)`;
    const tab = up ? 0 : -1;
    r.tmainEl!.tabIndex = tab; r.cfoldEl!.tabIndex = tab;
  }
  function lift(cid: string) {
    const order = dockOrder(model.plan, CL.get(cid)!, state.cur);
    order.forEach((id, i) => {
      const r = rowById.get(id)!;
      r.el.style.setProperty("--dly", ctx.D(i * 65) + "ms");
      r.el.classList.add("up");
      setLiftZ(r);
    });
    slabs.get(cid)!.el.classList.add("up");
    state.liftedCluster = cid;
    ctx.later(ctx.D(1100), () => order.forEach(id => rowById.get(id)!.el.style.setProperty("--dly", "0ms")));
  }
  // The tethers of sinking boxes stay drawn until the boxes land and fade.
  function drop(cid: string) {
    const until = performance.now() + ctx.D(1100);
    CL.get(cid)!.members.forEach((id, i) => {
      const r = rowById.get(id)!;
      r.el.style.setProperty("--dly", ctx.D(i * 25) + "ms");
      r.el.classList.remove("up", "focus", "peek");
      r.tetherEl!.classList.remove("focus");
      r.sinkUntil = until;
      setLiftZ(r);
    });
    slabs.get(cid)!.el.classList.remove("up");
    state.liftedCluster = null;
  }
  function applyFocus() {
    const s = model.steps[state.cur - 1], fid = s && !s.missing ? s.node : null;
    for (const r of memberRows) {
      const on = !!fid && r.id === fid && r.el.classList.contains("up");
      r.el.classList.toggle("focus", on);
      r.tetherEl!.classList.toggle("focus", on);
      if (on) r.stepnoEl!.textContent = String(state.cur);
      r.tmainEl!.setAttribute("aria-label", on ? `${r.name}, step ${state.cur}` : r.name);
      setLiftZ(r);
    }
    const ff = fid ? model.FN.get(fid)!.file : null;
    for (const r of rows) if (r.kind === "file") r.el.classList.toggle("curfile", !!ff && r.file === ff);
  }

  return {
    relayout, refreshTree, setFlowLayout, lift, drop, applyFocus, frame,
    slabRect: cid => slabs.get(cid)?.rect as Rect,
    slabTop: cid => slabs.get(cid)!.top,
    destroy() { world.replaceChildren(); tetherG.replaceChildren(); },
  };
}

/* Deck panels. This module owns the right column (the callout and the extracted stack), the box details
   popover, and the text and markup of every panel, key row, tip, and picker. The markup builders return
   HTML strings with every data field escaped; `createPanels` owns the elements and their events.
   # Sections
   helpers, callout markup, stack markup, key markup, box details markup, picker markup,
   right column, box details */
import type { ArtifactEntry, ClusterColor, DeckCluster, DeckFlow, DeckFunction, FlowEntry } from "../data/models.ts";
import { dockOrder, nearestStep, rangesOf, runAt, type FlowPlan, type Run } from "../layout/flow-plan.ts";
import { clamp, f1 } from "../layout/camera.ts";
import type { DeckContext } from "./context.ts";

// ============ helpers ============

export const esc = (s: unknown) => String(s).replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
export const icon = (name: string) => `<svg class="ic" aria-hidden="true"><use href="#i-${name}"/></svg>`;
export const PTR = '<svg class="ic ptr" aria-hidden="true"><use href="#i-ptr"/></svg>';
/** Returns the CSS color of a cluster. */
export const colorVar = (color: ClusterColor) => `var(--cl-${color})`;
const plural = (n: number, one: string, many = one + "s") => `${n} ${n === 1 ? one : many}`;

// ============ callout markup ============

/** The rest callout: the flow badge (a picker when the codebase has more than one flow), the counts, and notes. */
export function restCallout(flow: DeckFlow | null, flowCount: number, repo: string): string {
  if (!flow) {
    return `<span class="pbadge"><span>no flow</span></span>` +
      `<p class="co-desc">${esc(repo)} has no saved flows yet.</p>` +
      `<p class="co-meta">Freeze one with <code>flow import</code>, then reload. The tree stays browsable.</p>`;
  }
  const badge = flowCount > 1
    ? `<button class="pbadge" type="button" id="flowPick" aria-haspopup="dialog" aria-expanded="false" aria-label="Flow ${esc(flow.name)}. Choose another flow"><span>${esc(flow.name)}</span>${icon("chev")}</button>`
    : `<span class="pbadge"><span>${esc(flow.name)}</span></span>`;
  const count = `${plural(flow.steps.length, "step")} in ${plural(flow.clusters.length, "cluster")}. Each cluster lifts its functions out of the tree onto a colored deck.`;
  const missing = flow.steps.filter(step => step.missing).length;
  const warn = [
    flow.stale ? "This flow was frozen against an older graph. Some steps can point to changed code." : "",
    missing ? `${plural(missing, "step")} name a function that this graph no longer has. ${missing === 1 ? "It plays" : "They play"} without a lift.` : "",
  ].filter(Boolean).map(text => `<p class="co-warn">${esc(text)}</p>`).join("");
  return badge + `<p class="co-desc"></p><p class="co-meta">${count}</p>` + warn;
}

/** The cluster callout: the cluster badge, its description, its size, and its step ranges. */
export function clusterCallout(cluster: DeckCluster, plan: FlowPlan): string {
  return `<span class="pbadge" style="--c:${colorVar(cluster.color)}"><span>${esc(cluster.label)}</span></span>` +
    `<p class="co-desc">${esc(cluster.description)}</p>` +
    `<p class="co-meta">${plural(cluster.members.length, "function")} · steps ${rangesOf(plan, cluster.id)}</p>`;
}

// ============ stack markup ============

/** The empty stack. */
export const emptyDock = () => `<div class="dk-head"><span class="pbadge">Extracted stack</span></div>` +
  `<p class="dk-empty">Lifted functions stack here in step order. Nothing is lifted yet.</p>`;

/** The stack of the lifted cluster, in stack order, with one step pill for each visit. */
export function clusterDock(cluster: DeckCluster, plan: FlowPlan, current: number, fns: ReadonlyMap<string, DeckFunction>): string {
  const run = runAt(plan, current);
  const inRun = (i: number) => !!run && run.cluster === cluster.id && i >= run.start && i <= run.end;
  const span = !run ? "" : run.start === run.end ? `step ${run.start}` : `steps ${run.start}–${run.end}`;
  let html = `<div class="dk-head"><span class="pbadge">Extracted stack</span><span class="dk-span">${span}<span class="dk-long"> in this pass</span></span></div><ol class="dk-list">`;
  for (const id of dockOrder(plan, cluster, current)) {
    const fn = fns.get(id)!, visits = plan.visits.get(id) ?? [];
    const later = !visits.some(inRun);
    const near = nearestStep(plan, id, current);
    html += `<li class="ditem${later ? " later" : ""}" data-id="${esc(id)}" style="--c:${colorVar(cluster.color)}">` +
      `<button class="dbox" type="button" data-id="${esc(id)}" aria-label="${esc(fn.name)}${near ? `, go to step ${near}` : ""}">${PTR}<span class="lp">${esc(fn.file)}</span><span class="ln">${esc(fn.name)}</span></button>` +
      `<span class="pills">${visits.map(i => `<button class="pill${inRun(i) ? " run" : ""}" type="button" data-step="${i}" aria-label="Go to step ${i}">${i}</button>`).join("")}</span></li>`;
  }
  return html + "</ol>";
}

// ============ key markup ============

/** One key row for each cluster, with its step ranges. */
export const keyClusters = (flow: DeckFlow | null, plan: FlowPlan) => (flow?.clusters ?? []).filter(cluster => cluster.members.length)
  .map(cluster => `<li><button class="kp-c" type="button" data-c="${esc(cluster.id)}" style="--c:${colorVar(cluster.color)}" aria-label="${esc(cluster.label)}, steps ${rangesOf(plan, cluster.id)}">` +
    `<i class="sw" aria-hidden="true"></i><span class="ll">${esc(cluster.label)}</span><span class="rng">${rangesOf(plan, cluster.id)}</span></button></li>`).join("");

// ============ box details markup ============

/** The details of one function: name, file, visits, and static call counts. */
export function fnDetails(fn: DeckFunction, visits: readonly number[], current: number, callers: number, callees: number): string {
  const steps = visits.length ? `Steps ${visits.map(i => (i === current ? `<b>${i}</b>` : i)).join(" · ")}` : "Not in this flow";
  return `<div class="fp-name">${esc(fn.name)}</div><div class="fp-path">${esc(fn.file)}</div>` +
    `<div class="fp-meta">${steps}<span class="fp-sep" aria-hidden="true"></span>Called by ${callers} · calls ${callees}</div>`;
}

// ============ picker markup ============

/** The flow list of one codebase. */
export function flowPicker(flows: readonly FlowEntry[], current: string | null): string {
  const items = flows.map(flow => {
    const steps = flow.stepCount === undefined ? "" : `${plural(flow.stepCount, "step")} · `;
    return `<li><button class="pk-item" type="button" data-flow="${esc(flow.name)}" aria-current="${flow.name === current}">` +
      `<span class="pk-name">${esc(flow.name)}</span>${flow.stale ? '<span class="pk-flag">stale</span>' : ""}` +
      `<span class="pk-meta">${steps}${plural(flow.clusterCount ?? 0, "cluster")}</span></button></li>`;
  }).join("");
  return `<p class="pk-hd">flows</p><ul class="pk-list">${items}</ul>`;
}

/** The list of indexed codebases. `flowCount` counts the flows that the viewer shows. */
export function codebasePicker(artifacts: readonly ArtifactEntry[], current: string | null, flowCount: (entry: ArtifactEntry) => number): string {
  const items = artifacts.map(entry => `<li><button class="pk-item" type="button" data-artifact="${esc(entry.file)}" aria-current="${entry.file === current}">` +
    `<span class="pk-name">${esc(entry.label ?? entry.file)}</span><span class="pk-meta">${esc(entry.file)} · ${plural(flowCount(entry), "flow")}</span></button></li>`).join("");
  return `<p class="pk-hd">codebases</p><ul class="pk-list">${items}</ul>`;
}

// ============ right column ============

type HoverKind = "lid" | "row" | "dock";

export interface Panels {
  /** Shows the rest callout, or the callout of the cluster whose rows are slots. */
  renderCallout(): void;
  /** Shows the empty stack, or the stack of the cluster whose rows are slots. */
  renderDock(): void;
  /** Rebuilds the stack when the current step enters another pass; otherwise moves its focus only. */
  updateDock(): void;
  /** Refreshes the open box details for the current step. */
  refreshHover(): void;
  /** Closes box details that belong to a box or a stack item. Tree row details stay. */
  dropBoxHover(): void;
  /** Places the open box details next to its anchor. The frame loop calls it. */
  placeHover(): void;
  destroy(): void;
}

export function createPanels(ctx: DeckContext): Panels {
  const { model, state, signal } = ctx;
  const { CL, FN, rowById } = model;
  const callout = document.getElementById("callout")!, dock = document.getElementById("dock")!;
  const world = document.getElementById("world")!, fnPop = document.getElementById("fnPop")!;

  function renderCallout() {
    const C = state.layoutCluster ? CL.get(state.layoutCluster)! : null;
    callout.classList.remove("swap"); void callout.offsetWidth; if (!ctx.reducedMotion) callout.classList.add("swap");
    callout.innerHTML = C ? clusterCallout(C, model.plan) : restCallout(model.flow, ctx.options.flowCount, model.graph.repo);
    const pick = callout.querySelector<HTMLElement>("#flowPick");
    if (pick) pick.addEventListener("click", () => ctx.options.onFlowPick?.(pick), { signal });
  }

  let dockRun: Run | null = null;
  function renderDock() {
    const C = state.layoutCluster ? CL.get(state.layoutCluster)! : null;
    dockRun = C ? runAt(model.plan, state.cur) : null;
    if (!C) { dock.innerHTML = emptyDock(); return; }
    dock.innerHTML = clusterDock(C, model.plan, state.cur, FN);
    if (!ctx.reducedMotion) dock.querySelectorAll<HTMLElement>(".ditem").forEach((it, i) => {
      it.animate([{ opacity: 0, transform: "translateX(16px)" }, { opacity: 1, transform: "none" }],
        { duration: 420, delay: 120 + i * 65, easing: "cubic-bezier(.2,.9,.3,1)", fill: "backwards" });
    });
    updateDockFocus();
  }
  function updateDockFocus() {
    const s = model.steps[state.cur - 1];
    dock.querySelectorAll<HTMLElement>(".ditem").forEach(it => it.classList.toggle("focus", !!s && !s.missing && it.dataset.id === s.node));
    dock.querySelectorAll<HTMLElement>(".pill").forEach(p => p.classList.toggle("now", +p.dataset.step! === state.cur));
  }
  dock.addEventListener("click", e => {
    const target = e.target as Element;
    const p = target.closest<HTMLElement>(".pill");
    if (p) { ctx.go(+p.dataset.step!); return; }
    const b = target.closest<HTMLElement>(".dbox");
    if (b) { const n = nearestStep(model.plan, b.dataset.id!, state.cur); if (n) ctx.go(n); }
  }, { signal });

  // ============ box details ============
  // Hover or keyboard focus shows the details of one function. kind: "lid" (map box), "row" (tree row), or "dock" (stack box).

  let hover: { id: string; kind: HoverKind; el: Element } | null = null;
  let popSize = { w: 0, h: 0 };
  function fnPopHtml(id: string) {
    const cs = model.callsOf.get(id) ?? [];
    return fnDetails(FN.get(id)!, model.plan.visits.get(id) ?? [], state.cur, cs.filter(e => e.target === id).length, cs.filter(e => e.source === id).length);
  }
  function setHover(id: string | null, kind?: HoverKind, el?: Element) {
    if ((hover ? `${hover.id}|${hover.kind}` : "") === (id ? `${id}|${kind}` : "")) return;
    if (hover) {
      if (hover.kind === "lid") rowById.get(hover.id)?.tmainEl?.removeAttribute("aria-describedby");
      if (hover.kind === "dock") { hover.el.removeAttribute("aria-describedby"); rowById.get(hover.id)?.el.classList.remove("peek"); }
    }
    hover = id && kind && el ? { id, kind, el } : null;
    if (hover) {
      fnPop.innerHTML = fnPopHtml(hover.id);
      popSize = { w: fnPop.offsetWidth, h: fnPop.offsetHeight };
      if (hover.kind === "lid") rowById.get(hover.id)!.tmainEl!.setAttribute("aria-describedby", "fnPop");
      if (hover.kind === "dock") { hover.el.setAttribute("aria-describedby", "fnPop"); rowById.get(hover.id)!.el.classList.add("peek"); }
      placeFnPop();
    }
    fnPop.classList.toggle("show", !!hover);
  }
  // Map boxes and rows: the popover sits above the anchor, or below near the top edge. Stack boxes: to the left of the stack.
  function placeFnPop() {
    if (!hover) return;
    if (!hover.el.isConnected) { setHover(null); return; }
    const a = hover.el.getBoundingClientRect();
    let x: number, y: number, at: string;
    if (hover.kind === "dock") {
      at = "at-left";
      x = dock.getBoundingClientRect().left - popSize.w - 10;
      y = clamp(a.top + a.height / 2 - popSize.h / 2, 8, innerHeight - popSize.h - 8);
    } else {
      at = "at-top"; x = a.left; y = a.top - popSize.h - 9;
      if (y < 8) { at = "at-bottom"; y = a.bottom + 9; }
      x = clamp(x, 8, innerWidth - popSize.w - 8);
    }
    if (fnPop.dataset.at !== at) { fnPop.classList.remove("at-left", "at-top", "at-bottom"); fnPop.classList.add(at); fnPop.dataset.at = at; }
    fnPop.style.transform = `translate(${f1(x)}px,${f1(y)}px)`;
  }
  const keyboardFocus = (el: Element | null) => !!el && el.matches(":focus-visible");
  world.addEventListener("pointerover", e => {
    if (ctx.parts.input.dragging()) return;
    const target = e.target as Element;
    const lid = target.closest(".row.up .lid");
    if (lid) { setHover((lid.closest(".row") as HTMLElement).dataset.id!, "lid", lid); return; }
    const n = target.closest(".row.fn .node");
    if (n) { const r = rowById.get((n.parentElement as HTMLElement).dataset.id!)!; if (r.mode !== "stub" && r.mode !== "hidden") setHover(r.id, "row", n); }
  }, { signal });
  world.addEventListener("pointerout", e => {
    const t = (e.target as Element).closest(".lid, .row.fn .node");
    if (!t || t.contains(e.relatedTarget as Node | null)) return;
    if (t.contains(document.activeElement) && keyboardFocus(document.activeElement)) return;
    if (hover && hover.el === t) setHover(null);
  }, { signal });
  world.addEventListener("focusin", e => {
    const b = (e.target as Element).closest(".lid");
    if (!b || !keyboardFocus(e.target as Element)) return;
    setHover((b.closest(".row") as HTMLElement).dataset.id!, "lid", b);
  }, { signal });
  world.addEventListener("focusout", e => {
    const b = (e.target as Element).closest(".lid");
    if (!b) return;
    if (!(e.relatedTarget && b.contains(e.relatedTarget as Node)) && hover && hover.el === b) setHover(null);
  }, { signal });
  dock.addEventListener("pointerover", e => { const b = (e.target as Element).closest<HTMLElement>(".dbox"); if (b) setHover(b.dataset.id!, "dock", b); }, { signal });
  dock.addEventListener("pointerout", e => {
    const b = (e.target as Element).closest(".dbox");
    if (!b || b.contains(e.relatedTarget as Node | null)) return;
    if (b === document.activeElement && keyboardFocus(b)) return;
    if (hover && hover.el === b) setHover(null);
  }, { signal });
  dock.addEventListener("focusin", e => { const b = (e.target as Element).closest<HTMLElement>(".dbox"); if (b && keyboardFocus(b)) setHover(b.dataset.id!, "dock", b); }, { signal });
  dock.addEventListener("focusout", e => { const b = (e.target as Element).closest(".dbox"); if (b && hover && hover.el === b) setHover(null); }, { signal });

  return {
    renderCallout, renderDock,
    updateDock() { if (runAt(model.plan, state.cur) !== dockRun) renderDock(); else updateDockFocus(); },
    refreshHover() { if (hover) { fnPop.innerHTML = fnPopHtml(hover.id); popSize = { w: fnPop.offsetWidth, h: fnPop.offsetHeight }; } },
    dropBoxHover() { if (hover && hover.kind !== "row") setHover(null); },
    placeHover() { if (hover && hover.kind !== "dock") placeFnPop(); },
    destroy() { callout.innerHTML = ""; dock.innerHTML = ""; fnPop.classList.remove("show"); },
  };
}

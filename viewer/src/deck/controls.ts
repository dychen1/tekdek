/* Deck controls. This module owns the flow controls (previous and next, counter, cluster chip, step title with its
   cursor, annotation drawer, step strip and tips), the map tools, the key popover, and the theme variant.
   The controls keep one fixed size for each state.
   # Sections
   key popover, theme variant, step strip, annotation drawer, step display, buttons */
import { clamp, f1 } from "../layout/camera.ts";
import { esc, keyClusters } from "./panels.ts";
import { $, KEY_ANN, KEY_VARIANT, store, type DeckContext } from "./context.ts";

export interface Controls {
  /** Shows the current step in the counter, the chip, the title, the annotation, the strip, and the key. */
  update(): void;
  keyOpen(): boolean;
  setKey(open: boolean, returnFocus?: boolean): void;
  /** Applies the annotation drawer state to the controls. */
  syncAnnotation(): void;
  destroy(): void;
}

export function createControls(ctx: DeckContext): Controls {
  const { model, state, signal } = ctx;
  const { flow, steps, N, CL } = model;
  const controlsEl = $("#controls");

  // ============ key popover ============

  const keyBtn = $("#keyBtn"), keyPop = $("#keyPop"), kpClusters = $("#kpClusters");
  kpClusters.innerHTML = keyClusters(flow, model.plan);
  kpClusters.addEventListener("click", e => {
    const b = (e.target as Element).closest<HTMLElement>(".kp-c");
    if (!b) return;
    const rs = model.plan.runs.filter(r => r.cluster === b.dataset.c);
    const next = rs.find(r => r.start > state.cur) || rs[0];
    if (next) ctx.go(next.start);
  }, { signal });
  function updateLegend() {
    const cid = state.cur ? model.stepCluster(state.cur) : null;
    kpClusters.querySelectorAll<HTMLElement>(".kp-c").forEach(b => b.classList.toggle("on", b.dataset.c === cid));
  }
  const keyOpen = () => keyBtn.getAttribute("aria-expanded") === "true";
  function setKey(isOpen: boolean, returnFocus = false) {
    keyPop.classList.toggle("show", isOpen);
    keyBtn.setAttribute("aria-expanded", String(isOpen));
    if (!isOpen && returnFocus) keyBtn.focus();
  }
  keyBtn.addEventListener("click", () => setKey(!keyOpen()), { signal });
  document.addEventListener("pointerdown", e => {
    if (keyOpen() && !(e.target as Element).closest("#keyPop,#keyBtn")) setKey(false);
  }, { signal });

  // ============ theme variant ============
  // The variant swaps the palette tokens on the root. Night is the default. The choice is kept for each viewer.

  const VARIANTS = ["night", "storm", "moon"];
  const varBtns = [...document.querySelectorAll<HTMLElement>(".seg-b")];
  function setVariant(v: string | null, save: boolean) {
    const name = v && VARIANTS.includes(v) ? v : "night";
    if (name === "night") delete document.documentElement.dataset.variant; else document.documentElement.dataset.variant = name;
    varBtns.forEach(b => b.setAttribute("aria-pressed", String(b.dataset.variant === name)));
    if (save) store.set(KEY_VARIANT, name);
  }
  varBtns.forEach(b => b.addEventListener("click", () => setVariant(b.dataset.variant!, true), { signal }));
  setVariant(store.get(KEY_VARIANT), false);

  // ============ step strip ============

  const ticks = $("#ticks"), tip = $("#tip");
  const tickColor = (index: number) => { const cid = model.stepCluster(index); return cid ? CL.get(cid)!.cv : "var(--line-strong)"; };
  ticks.innerHTML = steps.map(s => `<button class="tick" type="button" tabindex="-1" data-step="${s.index}" style="--c:${tickColor(s.index)}" aria-label="Step ${s.index}: ${esc(s.title)}"></button>`).join("");
  const tickEls = [...ticks.querySelectorAll<HTMLElement>(".tick")];
  if (tickEls[0]) tickEls[0].tabIndex = 0;
  ticks.addEventListener("click", e => { const t = (e.target as Element).closest<HTMLElement>(".tick"); if (t) ctx.go(+t.dataset.step!); }, { signal });
  function showTip(t: HTMLElement) {
    const s = steps[+t.dataset.step! - 1];
    tip.innerHTML = `<i class="sw" style="--c:${tickColor(s.index)}"></i><b>${s.index}</b><span>${esc(s.title)}</span>`;
    const r = t.getBoundingClientRect(), cr = controlsEl.getBoundingClientRect();
    tip.classList.add("show");
    const w = tip.offsetWidth, h = tip.offsetHeight;
    tip.style.transform = `translate(${f1(clamp(r.left + r.width / 2 - w / 2, 8, innerWidth - w - 8))}px,${f1(cr.top - h - 6)}px)`;
  }
  const hideTip = () => tip.classList.remove("show");
  ticks.addEventListener("pointerover", e => { const t = (e.target as Element).closest<HTMLElement>(".tick"); if (t) showTip(t); }, { signal });
  ticks.addEventListener("pointerleave", () => { const a = document.activeElement; if (!(a && ticks.contains(a) && a.matches(":focus-visible"))) hideTip(); }, { signal });
  ticks.addEventListener("focusin", e => { if ((e.target as Element).matches(".tick:focus-visible")) showTip(e.target as HTMLElement); }, { signal });
  ticks.addEventListener("focusout", e => { if (!ticks.contains(e.relatedTarget as Node | null)) hideTip(); }, { signal });
  ticks.addEventListener("keydown", e => {
    if (e.key === "Home") { e.preventDefault(); ctx.go(1); }
    else if (e.key === "End") { e.preventDefault(); ctx.go(N); }
  }, { signal });

  // ============ annotation drawer ============

  const annBtn = $("#annBtn");
  function syncAnnotation() {
    controlsEl.classList.toggle("ann-open", state.annOpen);
    document.body.classList.toggle("ann-open", state.annOpen);
    annBtn.setAttribute("aria-expanded", String(state.annOpen));
    annBtn.setAttribute("aria-label", state.annOpen ? "Hide the full annotation" : "Show the full annotation");
  }
  function setAnnotation(isOpen: boolean) {
    state.annOpen = isOpen;
    store.set(KEY_ANN, isOpen ? "1" : "0");
    syncAnnotation();
    // The controls grow over 200 ms; measure after, then keep the focus box in view.
    ctx.later(ctx.D(230), () => {
      ctx.parts.viewport.measure();
      if (state.layoutCluster) ctx.parts.viewport.focus(); else ctx.parts.viewport.apply(true);
    });
  }
  annBtn.addEventListener("click", () => setAnnotation(!state.annOpen), { signal });

  // ============ step display ============

  function update() {
    const s = steps[state.cur - 1];
    $("#counter").textContent = `Step ${state.cur} / ${N}`;
    const cid = state.cur ? model.stepCluster(state.cur) : null, C = cid ? CL.get(cid)! : null;
    document.body.style.setProperty("--cc", C ? C.cv : "var(--pick)");
    document.body.classList.toggle("flow", !!s);
    const chip = $("#chip");
    chip.classList.toggle("none", !C);
    chip.style.setProperty("--c", C ? C.cv : "transparent");
    if (!flow) {
      $("#chipText").textContent = "No flow";
      $("#cTitle").textContent = model.graph.repo;
      $("#cNode").textContent = "No saved flows";
      $("#cAnn").textContent = "Browse the file tree. A saved flow lifts its functions onto a deck.";
    } else if (!s) {
      $("#chipText").textContent = "No cluster lifted";
      $("#cTitle").textContent = flow.name;
      $("#cNode").textContent = `${N} steps in ${model.clusters.length} clusters`;
      $("#cAnn").textContent = "The whole file tree is flat. Press the right arrow to lift the first cluster.";
    } else {
      $("#chipText").textContent = C ? C.label : "No cluster lifted";
      $("#cTitle").textContent = s.title;
      if (s.missing) $("#cNode").innerHTML = `not in this graph · <b>${esc(s.node)}</b>`;
      else { const f = model.FN.get(s.node)!; $("#cNode").innerHTML = `${esc(f.file)} · <b>${esc(f.name)}</b>`; }
      $("#cAnn").textContent = s.annotation;
    }
    $<HTMLButtonElement>("#prevBtn").disabled = state.cur === 0;
    $<HTMLButtonElement>("#nextBtn").disabled = state.cur === N;
    $<HTMLButtonElement>("#clearBtn").disabled = state.cur === 0;
    const hadFocus = ticks.contains(document.activeElement);
    tickEls.forEach((t, i) => {
      const n = i + 1;
      t.classList.toggle("past", n < state.cur); t.classList.toggle("now", n === state.cur);
      t.tabIndex = n === Math.max(state.cur, 1) ? 0 : -1;
      if (n === state.cur) t.setAttribute("aria-current", "step"); else t.removeAttribute("aria-current");
    });
    if (hadFocus && state.cur) tickEls[state.cur - 1].focus();
    updateLegend();
    restartBlink();
  }
  // A terminal cursor shows solid right after it moves, then blinks again.
  const cursorEl = $("#cursor");
  function restartBlink() { cursorEl.style.animation = "none"; void cursorEl.offsetWidth; cursorEl.style.animation = ""; }

  // ============ buttons ============

  $("#prevBtn").addEventListener("click", () => ctx.go(state.cur - 1), { signal });
  $("#nextBtn").addEventListener("click", () => ctx.go(state.cur + 1), { signal });
  $("#clearBtn").addEventListener("click", () => ctx.go(0), { signal });
  $("#fitBtn").addEventListener("click", () => ctx.parts.viewport.fit(true), { signal });
  $("#zoomIn").addEventListener("click", () => ctx.parts.viewport.zoomBy(1.25, state.view.cx, state.view.cy), { signal });
  $("#zoomOut").addEventListener("click", () => ctx.parts.viewport.zoomBy(0.8, state.view.cx, state.view.cy), { signal });

  return {
    update, keyOpen, setKey, syncAnnotation,
    destroy() {
      kpClusters.innerHTML = ""; ticks.innerHTML = "";
      tip.classList.remove("show"); setKey(false);
    },
  };
}

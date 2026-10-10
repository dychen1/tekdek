/* Deck tools. This module owns the map tools (compact, zoom, key), the key popover with its cluster legend and the
   theme switch, full screen code, and the top band layout. `main.ts` owns the flow badge in the top band.
   # Sections
   key popover, theme variant, map tools, full screen, top band */
import { rangesOf } from "../layout/plan.ts";
import { clamp, esc } from "../layout/util.ts";
import { icon } from "./markup.ts";
import { $, type DeckContext, type Tools } from "./context.ts";

const VARIANTS = ["night", "storm", "moon"];
const KEY_VARIANT = "tekdek.deck.variant";

/** Per-viewer conveniences. Blocked storage keeps the in-memory state. */
const store = {
  get(key: string) { try { return localStorage.getItem(key); } catch { return null; } },
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* storage is blocked */ } },
};

export function createTools(ctx: DeckContext): Tools {
  const { model, state, signal } = ctx;

  // ============ key popover ============
  // One legend row for each cluster that a step lifts. A row goes to the cluster's first step.

  const keyBtn = $("#keyBtn"), keyPop = $("#keyPop"), kpClusters = $("#kpClusters");
  kpClusters.innerHTML = model.clusters.filter(c => model.plan.runs.some(run => run.cluster === c.id)).map(c => {
    const ranges = rangesOf(model.plan, c.id);
    return `<li><button class="kp-c" type="button" data-c="${esc(c.id)}" style="--c:${esc(c.cv)}" aria-label="${esc(c.label)}, steps ${ranges}">` +
      `<i class="sw" aria-hidden="true"></i><span class="ll">${esc(c.label)}</span><span class="rng">${ranges}</span></button></li>`;
  }).join("");
  kpClusters.addEventListener("click", e => {
    const b = (e.target as Element).closest<HTMLElement>(".kp-c");
    const run = b ? model.plan.runs.find(r => r.cluster === b.dataset.c) : undefined;
    if (run) ctx.go(run.start);
  }, { signal });
  const keyOpen = () => keyBtn.getAttribute("aria-expanded") === "true";
  function setKey(open: boolean, returnFocus = false) {
    keyPop.classList.toggle("show", open);
    keyBtn.setAttribute("aria-expanded", String(open));
    if (!open && returnFocus) keyBtn.focus();
  }
  keyBtn.addEventListener("click", () => setKey(!keyOpen()), { signal });
  document.addEventListener("pointerdown", e => {
    if (keyOpen() && !(e.target as Element).closest("#keyPop,#keyBtn")) setKey(false);
  }, { signal });

  // ============ theme variant ============
  // The variant swaps the palette tokens on the root. Night is the default. The choice is kept for each viewer.

  const varBtns = [...document.querySelectorAll<HTMLElement>(".seg-b")];
  function setVariant(v: string | null, save: boolean) {
    const name = v && VARIANTS.includes(v) ? v : "night";
    if (name === "night") delete document.documentElement.dataset.variant; else document.documentElement.dataset.variant = name;
    varBtns.forEach(b => b.setAttribute("aria-pressed", String(b.dataset.variant === name)));
    if (save) store.set(KEY_VARIANT, name);
  }
  varBtns.forEach(b => b.addEventListener("click", () => setVariant(b.dataset.variant!, true), { signal }));
  setVariant(store.get(KEY_VARIANT), false);

  // ============ map tools ============

  const compactBtn = $<HTMLButtonElement>("#compactBtn"), zoomLbl = $("#zoomLbl");

  /** Shows the compact state, the zoom level, and the current cluster in the key. Compact is off at rest. */
  function sync() {
    compactBtn.disabled = !state.cur;
    compactBtn.setAttribute("aria-pressed", String(state.compact));
    zoomLbl.textContent = `${Math.round(state.zoom * 100)}%`;
    const id = model.stepCluster(state.cur);
    kpClusters.querySelectorAll<HTMLElement>(".kp-c").forEach(b => b.classList.toggle("on", !!id && b.dataset.c === id));
  }

  /** Scales the canvas around the file node, from 50% to 150%. */
  function setZoom(k: number) {
    state.zoom = clamp(Math.round(k * 100) / 100, 0.5, 1.5);
    ctx.place();
    sync();
  }

  /** Stubs or names the functions off the flow. Compact starts on at every load; nothing stores it. */
  function toggleCompact() {
    if (!state.cur) return;
    state.compact = !state.compact;
    ctx.parts.fileBox.render();
    ctx.place();
    sync();
  }

  compactBtn.addEventListener("click", toggleCompact, { signal });
  $("#zoomOut").addEventListener("click", () => setZoom(state.zoom - 0.1), { signal });
  $("#zoomIn").addEventListener("click", () => setZoom(state.zoom + 0.1), { signal });

  // ============ full screen ============

  const fullBtn = $("#fullBtn");
  /** Sets the page class and the full screen button for one mode. */
  function showFull(on: boolean) {
    document.body.classList.toggle("full", on);
    const label = on ? "Exit full screen (f)" : "Full screen (f)";
    fullBtn.setAttribute("aria-pressed", String(on)); fullBtn.title = label; fullBtn.setAttribute("aria-label", label);
    fullBtn.innerHTML = icon(on ? "exit" : "full");
  }
  /** Shows the code full screen over the canvas with no connectors, or returns to the canvas with its connectors drawn at once. */
  function setFull(on: boolean) {
    state.full = on;
    showFull(on);
    if (on) { ctx.parts.links.draw(); return; }
    layoutTop();
    // the explorer does not scroll to the current row in full screen: the canvas frame has no size there
    ctx.parts.explorer.reveal();
    ctx.place();
  }
  fullBtn.addEventListener("click", () => setFull(!state.full), { signal });

  // ============ top band ============

  /** Puts the flow band right of the map tools, and the canvas frame below both. */
  function layoutTop() {
    const mt = $("#maptools").getBoundingClientRect();
    document.documentElement.style.setProperty("--fh-l", `${Math.round(mt.right + 18)}px`);
    const fh = $("#fhTop").getBoundingClientRect();
    document.documentElement.style.setProperty("--top-band", `${Math.round(Math.max(mt.bottom, fh.bottom) + 12)}px`);
  }

  return {
    sync, layoutTop, setFull, toggleCompact, keyOpen, setKey,
    destroy() {
      setKey(false);
      kpClusters.replaceChildren();
      showFull(false);
    },
  };
}

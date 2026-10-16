/* Deck tools. This module owns the map tools (compact, zoom, key), the key popover with its cluster legend, the
   theme switch, full screen code, the shortcut switch, and the top band layout. `main.ts` owns the flow badge in the top band.
   # Sections
   key popover, theme variant, map tools, full screen, shortcuts, top band */
import { ZOOM_MIN } from "../layout/deck-geometry.ts";
import { shownZoom } from "../layout/model.ts";
import { rangesOf } from "../layout/plan.ts";
import { clamp, esc } from "../layout/util.ts";
import { icon } from "./markup.ts";
import { $, pageOrigin, type DeckContext, type Tools } from "./context.ts";

const VARIANTS = ["night", "storm", "moon"];
const KEY_VARIANT = "tekdek.deck.variant";
const KEY_SHORTCUTS = "tekdek.deck.shortcuts";

/** Per-viewer conveniences. The inline script in `index.html` reads them onto the root before first paint, and every
 *  deck starts from the root, so blocked storage keeps the choice of this page. */
const store = {
  set(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* storage is blocked */ } },
};

/** Scrolls the page, never the body, until a screen box shows in the window: a scrolled small window can hide the map tools. */
function scrollPageTo(r: { left: number; top: number; right: number; bottom: number }) {
  const w = document.documentElement.clientWidth, h = document.documentElement.clientHeight;
  const dx = r.left < 8 ? r.left - 8 : Math.max(0, r.right - w + 8), dy = r.top < 8 ? r.top - 8 : Math.max(0, r.bottom - h + 8);
  if (dx || dy) scrollBy(dx, dy);
}

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
  /** Opens or closes the key. With `moveFocus`, an open moves the focus into the key and a close returns it to the key button.
   *  A close also returns the focus that was inside the key, so it never falls to the page. */
  function setKey(open: boolean, moveFocus = false) {
    const inside = keyPop.contains(document.activeElement);
    keyPop.classList.toggle("show", open);
    keyBtn.setAttribute("aria-expanded", String(open));
    if (open) {
      const b = keyBtn.getBoundingClientRect(), p = keyPop.getBoundingClientRect();
      scrollPageTo({ left: Math.min(b.left, p.left), top: b.top, right: Math.max(b.right, p.right), bottom: p.bottom });
      if (moveFocus) keyPop.querySelector<HTMLElement>("button")?.focus();
    } else if (moveFocus || inside) keyBtn.focus({ preventScroll: true });
  }
  keyBtn.addEventListener("click", () => setKey(!keyOpen()), { signal });
  document.addEventListener("pointerdown", e => {
    if (keyOpen() && !(e.target as Element).closest("#keyPop,#keyBtn")) setKey(false);
  }, { signal });

  // ============ theme variant ============
  // The variant swaps the palette tokens on the root. Night is the default. The choice is kept for each viewer.

  const varBtns = [...document.querySelectorAll<HTMLElement>(".seg-b[data-variant]")];
  function setVariant(v: string | null, save: boolean) {
    const name = v && VARIANTS.includes(v) ? v : "night";
    if (name === "night") delete document.documentElement.dataset.variant; else document.documentElement.dataset.variant = name;
    varBtns.forEach(b => b.setAttribute("aria-pressed", String(b.dataset.variant === name)));
    if (save) store.set(KEY_VARIANT, name);
  }
  varBtns.forEach(b => b.addEventListener("click", () => setVariant(b.dataset.variant!, true), { signal }));
  setVariant(document.documentElement.dataset.variant ?? null, false);

  // ============ map tools ============

  const compactBtn = $<HTMLButtonElement>("#compactBtn"), zoomLbl = $("#zoomLbl");
  const zoomOut = $<HTMLButtonElement>("#zoomOut"), zoomIn = $<HTMLButtonElement>("#zoomIn");

  /** Shows the compact state, the zoom level and its limits, and the current cluster in the key.
   *  Compact is off at rest. Compact and zoom are off in full screen, where the map does not show. */
  function sync() {
    const k = shownZoom(state);
    compactBtn.disabled = !state.cur || state.full;
    compactBtn.setAttribute("aria-pressed", String(state.compact));
    zoomLbl.textContent = `${Math.round(k * 100)}%`;
    zoomOut.disabled = state.full || k <= ZOOM_MIN + 1e-3;
    zoomIn.disabled = state.full || k >= state.zoomMax - 1e-3;
    const id = model.stepCluster(state.cur);
    kpClusters.querySelectorAll<HTMLElement>(".kp-c").forEach(b => b.classList.toggle("on", !!id && b.dataset.c === id));
  }

  /** Sets the user's zoom, from 50% to the largest zoom at which the deck fits the frame. The canvas scales around the file node. */
  function setZoom(k: number) {
    state.zoom = clamp(k, ZOOM_MIN, state.zoomMax);
    ctx.place();
    sync();
  }

  /** Stubs or names the functions off the flow. Compact starts on at every load; nothing stores it. */
  function toggleCompact() {
    if (!state.cur || state.full) return;
    state.compact = !state.compact;
    ctx.parts.fileBox.render();
    ctx.place();
    sync();
  }

  compactBtn.addEventListener("click", toggleCompact, { signal });
  // The buttons step to the next tenth, also after a wheel zoom left the level between tenths.
  zoomOut.addEventListener("click", () => setZoom(Math.round(shownZoom(state) * 10 - 1) / 10), { signal });
  zoomIn.addEventListener("click", () => setZoom(Math.round(shownZoom(state) * 10 + 1) / 10), { signal });

  // Cmd + scroll over the canvas frame zooms continuously; a trackpad pinch sends the wheel with Ctrl.
  // The scroll of one frame applies once, so a fast trackpad places the map at most once a frame.
  const frame = $("#frame");
  let wheel = 0, pending = 0;
  document.addEventListener("wheel", e => {
    if (!e.metaKey && !e.ctrlKey) return;
    const r = frame.getBoundingClientRect();
    if (state.full || e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return;
    e.preventDefault();
    wheel += e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaY * 33 : e.deltaY;
    pending ||= requestAnimationFrame(() => {
      pending = 0;
      setZoom(shownZoom(state) * Math.exp(-wheel * 0.002));
      wheel = 0;
    });
  }, { passive: false, signal });
  signal.addEventListener("abort", () => cancelAnimationFrame(pending));

  // ============ full screen ============

  const fullBtn = $("#fullBtn");
  /** Sets the page class and the full screen button for one mode. The button names its key while the shortcuts are on. */
  function showFull(on: boolean) {
    document.body.classList.toggle("full", on);
    const label = `${on ? "Exit full screen" : "Full screen"}${document.documentElement.dataset.shortcuts === "off" ? "" : " (f)"}`;
    fullBtn.setAttribute("aria-pressed", String(on)); fullBtn.title = label; fullBtn.setAttribute("aria-label", label);
    fullBtn.innerHTML = icon(on ? "exit" : "full");
  }
  /** Shows the code full screen over the canvas with no connectors, or returns to the canvas with its connectors drawn at once.
   *  At rest there is no code, so full screen does not open. */
  function setFull(on: boolean) {
    if (on && !state.cur) return;
    state.full = on;
    showFull(on);
    if (on) ctx.parts.links.draw();
    else {
      layoutTop();
      // the explorer does not scroll to the current row in full screen: the canvas frame has no size there
      ctx.parts.explorer.reveal();
      ctx.place();
    }
    sync();
  }
  fullBtn.addEventListener("click", () => setFull(!state.full), { signal });

  // ============ shortcuts ============
  // The switch turns the one-character keys (j, k, f, c, ?) on or off. `input.ts` reads the root flag, and the
  // help line, the key list, the rest text, and the button names hide those keys while it is off. The choice is kept for each viewer.

  const keyBtns = [...document.querySelectorAll<HTMLElement>(".seg-b[data-shortcuts]")];
  function setShortcuts(on: boolean, save: boolean) {
    if (on) delete document.documentElement.dataset.shortcuts; else document.documentElement.dataset.shortcuts = "off";
    keyBtns.forEach(b => b.setAttribute("aria-pressed", String((b.dataset.shortcuts === "on") === on)));
    compactBtn.title = `Fold the functions off the flow path${on ? " (c)" : ""}`;
    showFull(state.full);
    if (save) store.set(KEY_SHORTCUTS, on ? "on" : "off");
  }
  keyBtns.forEach(b => b.addEventListener("click", () => setShortcuts(b.dataset.shortcuts === "on", true), { signal }));
  setShortcuts(document.documentElement.dataset.shortcuts !== "off", false);

  // ============ top band ============

  /** Puts the flow band right of the map tools, and the canvas frame below both. */
  function layoutTop() {
    const o = pageOrigin(), mt = $("#maptools").getBoundingClientRect();
    document.documentElement.style.setProperty("--fh-l", `${Math.round(mt.right - o.x + 18)}px`);
    const fh = $("#fhTop").getBoundingClientRect();
    document.documentElement.style.setProperty("--top-band", `${Math.round(Math.max(mt.bottom, fh.bottom) - o.y + 12)}px`);
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

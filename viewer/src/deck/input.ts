/* Deck input. This module owns the deck keys and the window resize. `main.ts` closes an open picker before these
   keys see Escape. The URL step is `?step=`, and `main.ts` writes it.
   # Sections
   keyboard, resize */
import type { DeckContext, Input } from "./model.ts";

/** True when a key goes to a text field. */
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));

export function createInput(ctx: DeckContext): Input {
  const { model, state, signal } = ctx;

  // ============ keyboard ============
  // j and Left go back, k and Right go forward; neither wraps. Escape closes the key, then leaves full screen, then clears the flow.

  addEventListener("keydown", e => {
    if (e.altKey || e.metaKey || e.ctrlKey || typing(e.target)) return;
    const { tools } = ctx.parts;
    const k = e.key;
    if (k === "Escape") {
      if (tools.keyOpen()) tools.setKey(false, true);
      else if (state.full) tools.setFull(false);
      else ctx.go(0);
      return;
    }
    if (k === "?") { e.preventDefault(); tools.setKey(!tools.keyOpen()); return; }
    if (k === "j" || k === "ArrowLeft") { e.preventDefault(); if (state.cur > 1) ctx.go(state.cur - 1); return; }
    if (k === "k" || k === "ArrowRight") { e.preventDefault(); if (state.cur < model.N) ctx.go(state.cur + 1); return; }
    if (k === "f") { tools.setFull(!state.full); return; }
    if (k === "c") tools.toggleCompact();
  }, { signal });

  // ============ resize ============

  let resizeRaf = 0;
  addEventListener("resize", () => {
    if (resizeRaf) return;
    resizeRaf = requestAnimationFrame(() => { resizeRaf = 0; ctx.parts.tools.layoutTop(); ctx.place(); ctx.kick(600); });
  }, { signal });

  return {
    destroy() {
      if (resizeRaf) cancelAnimationFrame(resizeRaf);
      resizeRaf = 0;
    },
  };
}

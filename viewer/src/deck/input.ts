/* Deck input. This module owns the deck keys. `main.ts` closes an open picker before these keys see Escape.
   The URL step is `?step=`, and `main.ts` writes it. */
import { nextStep } from "../layout/model.ts";
import { nextRunStart, prevRunStart } from "../layout/plan.ts";
import type { DeckContext } from "./context.ts";

/** True when a key goes to a text field. */
const typing = (target: EventTarget | null) =>
  target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));

/** Listens for the deck keys until the deck's signal aborts.
 *  j and Left go back, k and Right go forward; neither wraps. Forward from the rest state continues at the step that a clear left.
 *  h and l go to the first step of the previous and the next section, where the cluster changes.
 *  Escape closes the key, then leaves full screen, then clears the flow. */
export function createInput(ctx: DeckContext): void {
  const { model, state, signal } = ctx;

  addEventListener("keydown", e => {
    if (e.altKey || e.metaKey || e.ctrlKey || typing(e.target)) return;
    const { tools } = ctx.parts;
    const k = e.key;
    // The key popover can turn off the one-character keys (WCAG 2.1.4). The arrows and Escape stay on.
    if (k.length === 1 && document.documentElement.dataset.shortcuts === "off") return;
    if (k === "Escape") {
      if (tools.keyOpen()) tools.setKey(false, true);
      else if (state.full) tools.setFull(false);
      else ctx.go(0);
      return;
    }
    if (k === "?") { e.preventDefault(); tools.setKey(!tools.keyOpen(), true); return; }
    if (k === "j" || k === "ArrowLeft") { e.preventDefault(); if (state.cur > 1) ctx.go(state.cur - 1); return; }
    if (k === "k" || k === "ArrowRight") { e.preventDefault(); if (state.cur < model.N) ctx.go(nextStep(state)); return; }
    if (k === "h" || k === "l") {
      const to = k === "h" ? prevRunStart(model.plan, state.cur) : nextRunStart(model.plan, state.cur);
      if (to !== null) ctx.go(to);
      return;
    }
    if (k === "f") { tools.setFull(!state.full); return; }
    if (k === "c") tools.toggleCompact();
  }, { signal });
}

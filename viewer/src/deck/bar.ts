/* Deck bar. This module owns the bottom bar: the back, forward, and clear buttons, the step counter, the cluster
   explainer (the chip, the size and step ranges, the description), and the step strip with its tips.
   The bar keeps one fixed size in each state.
   # Sections
   step strip, explainer, buttons */
import { rangesOf } from "../layout/plan.ts";
import { clamp, esc, f1, plural } from "../layout/util.ts";
import { md } from "./markup.ts";
import { $, type Bar, type DeckContext } from "./context.ts";

export function createBar(ctx: DeckContext): Bar {
  const { model, state, signal } = ctx;
  const { steps, N, CL } = model;
  const controls = $("#controls"), ticks = $("#ticks"), tip = $("#tip");
  const chip = $("#chip"), cExpl = $("#cExpl"), cDesc = $("#cDesc");
  const prevBtn = $<HTMLButtonElement>("#prevBtn"), nextBtn = $<HTMLButtonElement>("#nextBtn"), clearBtn = $<HTMLButtonElement>("#clearBtn");

  // ============ step strip ============
  // One cell per step in its cluster color. A cell goes to its step; hover or focus names the step.

  const tickColor = (index: number) => { const id = model.stepCluster(index); return id ? CL.get(id)!.cv : "var(--line-strong)"; };
  ticks.innerHTML = steps.map(s => `<button class="tick" type="button" tabindex="-1" data-step="${s.index}" style="--c:${esc(tickColor(s.index))}" aria-label="Step ${s.index}: ${esc(s.title)}"></button>`).join("");
  const tickEls = [...ticks.querySelectorAll<HTMLElement>(".tick")];
  if (tickEls[0]) tickEls[0].tabIndex = 0;

  /** Names the step of a cell in the tip above the bar. */
  function showTip(t: HTMLElement) {
    const s = steps[+t.dataset.step! - 1];
    tip.innerHTML = `<i class="sw" style="--c:${esc(tickColor(s.index))}"></i><b>${s.index}</b><span>${esc(s.title)}</span>`;
    const r = t.getBoundingClientRect(), cr = controls.getBoundingClientRect();
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

  // ============ explainer ============

  function render(clusterChanged: boolean) {
    const id = model.stepCluster(state.cur), C = id ? CL.get(id) ?? null : null;
    $("#counter").textContent = `Step ${state.cur} / ${N}`;
    chip.classList.toggle("none", !C);
    chip.style.setProperty("--c", C ? C.cv : "transparent");
    $("#chipText").textContent = C ? C.label : model.flow ? "No cluster lifted" : "No flow";
    $("#cMeta").textContent = C ? `${plural(model.members.get(C.id)?.length ?? 0, "function")} · steps ${rangesOf(model.plan, C.id)}`
      : model.flow ? `${plural(N, "step")} in ${plural(model.clusters.length, "cluster")}` : "";
    cDesc.innerHTML = C ? md(C.description) : model.flow && !state.cur ? "Step forward to lift the first cluster onto its deck." : "";
    if (clusterChanged && !document.body.classList.contains("booting")) {
      for (const el of [cExpl, cDesc]) { el.classList.remove("swap"); void el.offsetWidth; el.classList.add("swap"); }
    }
    prevBtn.disabled = state.cur <= 1;
    nextBtn.disabled = state.cur >= N;
    clearBtn.disabled = !state.cur;
    const hadFocus = ticks.contains(document.activeElement);
    tickEls.forEach((t, i) => {
      const n = i + 1;
      t.classList.toggle("past", n < state.cur);
      t.classList.toggle("now", n === state.cur);
      t.tabIndex = (state.cur ? n === state.cur : i === 0) ? 0 : -1;
      if (n === state.cur) t.setAttribute("aria-current", "step"); else t.removeAttribute("aria-current");
    });
    if (hadFocus && state.cur) tickEls[state.cur - 1].focus();
  }

  // ============ buttons ============

  prevBtn.addEventListener("click", () => ctx.go(state.cur - 1), { signal });
  nextBtn.addEventListener("click", () => ctx.go(state.cur + 1), { signal });
  clearBtn.addEventListener("click", () => ctx.go(0), { signal });

  return {
    render,
    destroy() {
      ticks.replaceChildren();
      hideTip();
      cExpl.classList.remove("swap"); cDesc.classList.remove("swap");
      $("#counter").textContent = "Step 0 / 0";
      chip.classList.add("none");
      $("#chipText").textContent = "No flow";
      $("#cMeta").textContent = "";
      cDesc.textContent = "";
    },
  };
}

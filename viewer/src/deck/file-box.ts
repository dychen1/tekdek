/* Deck file box. This module owns the box of one file: a file node, then its functions on elbow guides.
   The layout comes from `layout/file-box.ts`. A slot, a row, or a step pill goes to its step.
   The chevron folds the rows that are not in the flow. */
import { fileBox, type FileBox } from "../layout/file-box.ts";
import { f1, plural } from "../layout/util.ts";
import { esc, icon } from "./markup.ts";
import { $, shownFile, type DeckContext, type FileBoxView } from "./model.ts";

export function createFileBox(ctx: DeckContext): FileBoxView {
  const { model, state, signal } = ctx;
  const { CL } = model;
  const fileblk = $("#fileblk"), fbIn = $("#fbIn");
  let shown: { file: string; box: FileBox } | null = null;

  /** Draws the box of the shown file, or hides it when no file shows. */
  function render() {
    const target = shownFile(model, state);
    if (!target) { fbIn.innerHTML = ""; fileblk.hidden = true; shown = null; return; }
    fileblk.hidden = false;
    fileblk.setAttribute("aria-label", `Functions of ${target.file}`);
    const folded = state.folded.has(target.file);
    const box = fileBox({
      fns: model.fnsByFile.get(target.file) ?? [], visits: model.plan.visits,
      currentCluster: target.cluster, currentKey: target.key, compact: state.compact, folded,
    });
    let html = `<div class="row file curfile${folded ? " folded" : ""}"><div class="node" id="fileNode">` +
      `<button class="fold" type="button" id="fileFold" aria-expanded="${!folded}" aria-label="${folded ? "Show" : "Fold"} the functions off the flow">${icon("chev")}</button>` +
      `<span class="lbl">${plural(box.total, "function")}<span class="fnote">${folded && box.offFlow ? `${box.offFlow} folded` : ""}</span></span></div></div>`;
    let prevMid = 20;
    for (const x of box.rows) {
      const { mode, flow, current: focus, h, y } = x;
      const mid = y + h / 2, C = flow && x.cluster ? CL.get(x.cluster) : null;
      const stepList = x.steps.join(", ");
      let node: string;
      if (mode === "slot") {
        node = `<button class="node" type="button" data-step="${x.steps[0]}" style="width:${x.w}px" aria-label="Slot of ${esc(x.name)}, step ${stepList}"><span class="nm">${esc(x.name)}</span></button>`;
      } else if (flow) {
        node = `<button class="node" type="button" data-step="${x.steps[0]}" aria-label="${esc(x.name)}, step ${stepList}"><span class="nm">${esc(x.name)}</span></button>`;
      } else if (mode === "stub") {
        node = `<div class="node" style="width:${x.w}px" title="${esc(x.name)}"><span class="nm">${esc(x.short)}</span></div>`;
      } else {
        node = `<div class="node" title="${esc(x.name)}"><span class="nm">${esc(x.short)}</span></div>`;
      }
      // every flow row carries its step pill, except the current slot: its tether and box carry the step
      const pills = flow && !focus ? x.steps.map(i => `<button class="spill" type="button" data-step="${i}" aria-label="Go to step ${i}">${i}</button>`).join("") : "";
      html += `<div class="row fn ${mode === "slot" ? "ghost" : mode}${flow ? " m" : ""}${focus ? " focus" : ""}" data-key="${esc(x.key)}" style="transform:translate(26px,${y}px);${C ? `--c:${esc(C.cv)}` : ""}">` +
        `<i class="elbow" style="top:${f1(prevMid - y)}px;height:${f1(mid - prevMid)}px"></i><div class="rowin" style="height:${h}px">${node}${pills}</div></div>`;
      prevMid = mid;
    }
    fbIn.innerHTML = html;
    shown = { file: target.file, box };
  }

  fbIn.addEventListener("click", e => {
    const t = (e.target as Element).closest<HTMLElement>("[data-step], #fileFold");
    if (!t) return;
    if (t.id === "fileFold") {
      if (!shown) return;
      if (state.folded.has(shown.file)) state.folded.delete(shown.file); else state.folded.add(shown.file);
      render();
      ctx.place();
      ctx.kick(800);
    } else ctx.go(+t.dataset.step!);
  }, { signal });

  return {
    render,
    height: () => shown?.box.height ?? 0,
    nodeEl: () => fbIn.querySelector<HTMLElement>("#fileNode"),
    slotEl: () => fbIn.querySelector<HTMLElement>(".row.fn.focus .node"),
    rowsRight: () => Math.max(0, ...[...fbIn.querySelectorAll<HTMLElement>(".rowin")].map(el => el.getBoundingClientRect().right)),
    destroy() { fbIn.replaceChildren(); fileblk.hidden = true; shown = null; },
  };
}

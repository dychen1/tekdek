/* Deck explorer. This module owns the transparent code explorer: directories and files in tree order,
   their tree guides, the current-file pointer, and the cluster markers. A directory click folds or opens it.
   A file click goes to the file's next flow step, or shows the file box of the file.
   # Sections
   rows, marks, scroll, clicks and keys */
import { anchorRow, dirNote, guideKinds, isShown, type ExplorerRow } from "../layout/explorer.ts";
import { nextStepOfFile } from "../layout/plan.ts";
import { clamp } from "../layout/util.ts";
import { esc, PTR } from "./markup.ts";
import { shownFile } from "../layout/model.ts";
import { $, type DeckContext, type Explorer } from "./context.ts";

/** A row and the elements that show it. */
interface RowView { row: ExplorerRow; el: HTMLButtonElement; note: HTMLElement | null }

/** Returns the tree guide cells of a row, or "" for a row at the top level. */
function guides(row: ExplorerRow): string {
  const kinds = guideKinds(row);
  return kinds.length ? `<span class="gds" aria-hidden="true">${kinds.map(kind => `<i class="gd ${kind}"></i>`).join("")}</span>` : "";
}

export function createExplorer(ctx: DeckContext): Explorer {
  const { model, state, signal } = ctx;
  const sbTree = $("#sbTree");

  // ============ rows ============

  const views: RowView[] = [];
  const viewByPath = new Map<string, RowView>();
  for (const row of model.explorer) {
    const el = document.createElement("button");
    el.type = "button"; el.className = "sl " + row.kind; el.tabIndex = -1; el.dataset.path = row.path;
    let meta = "";
    if (row.kind === "file") {
      const held = model.clustersByFile.get(row.path) ?? [];
      meta = held.length ? `<span class="meta"><span class="dots" aria-hidden="true">${held.map(id => `<i data-c="${esc(id)}" style="--c:${esc(model.CL.get(id)!.cv)}"></i>`).join("")}</span></span>` : "";
      const steps = model.plan.flowFiles.get(row.path);
      el.setAttribute("aria-label", `${row.path}${steps ? `, flow steps ${steps.join(", ")}` : ""}`);
    } else meta = '<span class="dnote"></span>';
    el.innerHTML = `${PTR}${guides(row)}<span class="snm">${esc(row.name)}</span>${meta}`;
    const view: RowView = { row, el, note: el.querySelector<HTMLElement>(".dnote") };
    el.addEventListener("click", () => (row.kind === "dir" ? toggleDir(row) : openFile(row)), { signal });
    views.push(view);
    viewByPath.set(row.path, view);
    sbTree.appendChild(el);
  }
  if (views[0]) views[0].el.tabIndex = 0;

  // ============ marks ============

  const currentFile = () => shownFile(model, state)?.file ?? null;

  function sync() {
    const cid = state.cur ? model.stepCluster(state.cur) : null;
    const file = currentFile();
    for (const { row, el, note } of views) {
      el.hidden = !isShown(row, state.open);
      if (row.kind === "dir") {
        const open = state.open.has(row.path);
        el.setAttribute("aria-expanded", String(open));
        if (note) note.textContent = dirNote(row, state.open);
        el.classList.toggle("inact", !!cid && !row.files.some(f => (model.clustersByFile.get(f) ?? []).includes(cid)));
      } else {
        const inC = !!cid && (model.clustersByFile.get(row.path) ?? []).includes(cid);
        el.classList.toggle("inc", inC);
        el.classList.toggle("inact", !!cid && !inC);
        const isCur = row.path === file;
        el.classList.toggle("cur", isCur);
        if (isCur) el.setAttribute("aria-current", "location"); else el.removeAttribute("aria-current");
        el.querySelectorAll<HTMLElement>(".dots i").forEach(dot => dot.classList.toggle("on", dot.dataset.c === cid));
      }
    }
  }

  /** The element of the current file's row, or of its nearest shown directory. */
  function anchorEl(): HTMLElement | null {
    const file = currentFile();
    const row = file ? model.rowByPath.get(file) : undefined;
    return row ? viewByPath.get(anchorRow(row, state.open).path)?.el ?? null : null;
  }

  // ============ scroll ============

  function reveal() {
    const el = anchorEl();
    if (!el || sbTree.scrollHeight <= sbTree.clientHeight) return;
    const fr = $("#frame").getBoundingClientRect(), t = sbTree.getBoundingClientRect(), rr = el.getBoundingClientRect(), y = rr.top + rr.height / 2;
    const lo = Math.max(t.top + 8, fr.top + 12), hi = Math.min(t.bottom - 8, fr.bottom - 12);
    if (y < lo || y > hi) sbTree.scrollTop += y - (lo + hi) / 2;
  }

  let scrollRaf = 0;
  sbTree.addEventListener("scroll", () => {
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => { scrollRaf = 0; ctx.place(); ctx.kick(600); });
  }, { signal });

  // ============ clicks and keys ============

  function toggleDir(row: ExplorerRow) {
    if (state.open.has(row.path)) state.open.delete(row.path); else state.open.add(row.path);
    sync();
    ctx.place();
    ctx.kick(800);
  }

  /** A file click goes to its next flow step, from the current step on. At rest, or for a file with no step, it shows the file box. */
  function openFile(row: ExplorerRow) {
    const next = state.cur ? nextStepOfFile(model.plan, row.path, state.cur) : null;
    if (next !== null) { ctx.go(next); return; }
    state.browseFile = row.path;
    sync();
    ctx.parts.fileBox.render();
    ctx.place();
  }

  sbTree.addEventListener("keydown", e => {
    const list = views.filter(view => !view.el.hidden), i = list.findIndex(view => view.el === document.activeElement);
    if (i < 0 || !["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    e.preventDefault(); e.stopPropagation();
    const j = clamp(e.key === "ArrowDown" ? i + 1 : e.key === "ArrowUp" ? i - 1 : e.key === "Home" ? 0 : list.length - 1, 0, list.length - 1);
    list[i].el.tabIndex = -1; list[j].el.tabIndex = 0; list[j].el.focus();
  }, { signal });

  return {
    sync, reveal, anchorEl,
    destroy() {
      if (scrollRaf) cancelAnimationFrame(scrollRaf);
      scrollRaf = 0;
      sbTree.replaceChildren();
    },
  };
}

/* Deck explorer. This module owns the transparent code explorer: directories and files in tree order,
   their tree guides, the current-file pointer, and the cluster markers. A directory click folds or opens it.
   A file click goes to the file's next flow step, or shows the file box of the file. A row is built the first
   time it shows, so a large codebase starts with only the rows of its open directories.
   # Sections
   rows, marks, scroll, clicks and keys */
import { anchorRow, dirNote, guideKinds, isShown, type ExplorerRow } from "../layout/explorer.ts";
import { nextStepOfFile } from "../layout/plan.ts";
import { clamp, esc } from "../layout/util.ts";
import { PTR } from "./markup.ts";
import { shownFile } from "../layout/model.ts";
import { $, type DeckContext, type Explorer } from "./context.ts";

/** A row and the elements that show it, or null elements until the row first shows. */
interface RowView { row: ExplorerRow; el: HTMLElement | null; note: HTMLElement | null }

/** The characters at the end of a name that never cut, so siblings with one long prefix stay apart. */
const TAIL = 6;

/** Returns the tree guide cells of a row, or "" for a row at the top level. */
function guides(row: ExplorerRow): string {
  const kinds = guideKinds(row);
  return kinds.length ? `<span class="gds" aria-hidden="true">${kinds.map(kind => `<i class="gd ${kind}"></i>`).join("")}</span>` : "";
}

/** The name of a row: the head cuts with an ellipsis, and the last characters stay whole. */
const nameHtml = (name: string) =>
  `<span class="snm"><span class="sh">${esc(name.slice(0, -TAIL))}</span><span class="st">${esc(name.slice(-TAIL))}</span></span>`;

export function createExplorer(ctx: DeckContext): Explorer {
  const { model, state, signal } = ctx;
  const sbTree = $("#sbTree");

  // ============ rows ============

  const views: RowView[] = model.explorer.map(row => ({ row, el: null, note: null }));
  const viewByPath = new Map(views.map(view => [view.row.path, view]));

  /** Builds the element of one row. The first row holds the tree's tab stop. */
  function build(view: RowView): HTMLElement {
    const { row } = view;
    const el = document.createElement("div");
    el.className = "sl " + row.kind; el.tabIndex = view === views[0] ? 0 : -1; el.dataset.path = row.path;
    el.setAttribute("role", "treeitem"); el.setAttribute("aria-level", String(row.depth + 1));
    el.setAttribute("aria-posinset", String(row.pos)); el.setAttribute("aria-setsize", String(row.size));
    let meta = "";
    if (row.kind === "file") {
      const held = model.clustersByFile.get(row.path) ?? [];
      meta = held.length ? `<span class="meta"><span class="dots" aria-hidden="true">${held.map(id => `<i data-c="${esc(id)}" style="--c:${esc(model.CL.get(id)!.cv)}"></i>`).join("")}</span></span>` : "";
      const steps = model.plan.flowFiles.get(row.path);
      el.setAttribute("aria-label", `${row.path}${steps ? `, flow steps ${steps.join(", ")}` : ""}`);
    } else meta = '<span class="dnote"></span>';
    el.innerHTML = `${PTR}${guides(row)}${nameHtml(row.name)}${meta}`;
    view.note = el.querySelector<HTMLElement>(".dnote");
    el.addEventListener("click", () => (row.kind === "dir" ? toggleDir(row) : openFile(row)), { signal });
    return el;
  }

  // ============ marks ============

  const currentFile = () => shownFile(model, state)?.file ?? null;

  function sync() {
    const cid = model.stepCluster(state.cur);
    const file = currentFile();
    let prev: HTMLElement | null = null;
    for (const view of views) {
      const { row } = view;
      const shown = isShown(row, state.open);
      if (!view.el) {
        if (!shown) continue;
        view.el = build(view);
        if (prev) prev.after(view.el); else sbTree.prepend(view.el);
      }
      const el = prev = view.el;
      el.hidden = !shown;
      if (row.kind === "dir") {
        const open = state.open.has(row.path);
        el.setAttribute("aria-expanded", String(open));
        const text = dirNote(row, state.open);
        view.note!.textContent = text;
        // the name parts are flex items, which a screen reader would read apart: the row names itself
        el.setAttribute("aria-label", text ? `${row.path} ${text}` : row.path);
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
    // a collapse can hide the row with the tab stop: the stop, and the focus it had, move to its nearest shown directory
    const stop = views.find(view => view.el?.tabIndex === 0);
    if (stop?.el?.hidden) {
      const to = viewByPath.get(anchorRow(stop.row, state.open).path)!.el!;
      moveStop(stop.el, to, document.activeElement === stop.el);
    }
  }

  /** Moves the tree's one tab stop from a row to another, and the focus with it when `focus` is set. */
  function moveStop(from: HTMLElement, to: HTMLElement, focus: boolean) {
    from.tabIndex = -1; to.tabIndex = 0;
    if (focus) to.focus();
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

  sbTree.addEventListener("scroll", ctx.placeSoon, { signal });

  // ============ clicks and keys ============

  function toggleDir(row: ExplorerRow) {
    if (state.open.has(row.path)) state.open.delete(row.path); else state.open.add(row.path);
    sync();
    ctx.place();
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

  // The tree keys: Up and Down move between the shown rows, Home and End go to the ends, Right opens a directory or goes to
  // its first row, Left closes a directory or goes to the parent, and Enter and Space click a row. The deck keys never see them.
  const TREE_KEYS = ["ArrowDown", "ArrowUp", "Home", "End", "ArrowRight", "ArrowLeft", "Enter", " "];
  sbTree.addEventListener("keydown", e => {
    const shown = views.filter(view => view.el && !view.el.hidden), i = shown.findIndex(view => view.el === document.activeElement);
    if (i < 0 || !TREE_KEYS.includes(e.key)) return;
    e.preventDefault(); e.stopPropagation();
    const { row, el } = shown[i], open = state.open.has(row.path);
    if (e.key === "Enter" || e.key === " ") { el!.click(); return; }
    if (row.kind === "dir" && ((e.key === "ArrowRight" && !open) || (e.key === "ArrowLeft" && open))) { toggleDir(row); return; }
    let j = i;
    if (e.key === "ArrowLeft") j = row.parent ? shown.findIndex(view => view.row === row.parent) : i;
    else if (e.key === "ArrowRight") j = row.kind === "dir" ? i + 1 : i;
    else j = clamp(e.key === "ArrowDown" ? i + 1 : e.key === "ArrowUp" ? i - 1 : e.key === "Home" ? 0 : shown.length - 1, 0, shown.length - 1);
    moveStop(el!, shown[j].el!, true);
  }, { signal });

  return {
    sync, reveal, anchorEl,
    destroy() { sbTree.replaceChildren(); },
  };
}

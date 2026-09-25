/* Deck sidebar. This module owns the transparent code explorer: directories and files only, with tree guides,
   the current-file pointer, cluster markers on hover or focus, and the bracket of the part of the tree in view.
   A click opens the path to a row as a user choice and pans the map to it. */
import { openPath, type Row } from "../layout/tree.ts";
import { clamp, RAD } from "../layout/camera.ts";
import { esc, PTR } from "./panels.ts";
import { $, mk, type DeckContext, type RowView } from "./context.ts";

export interface Sidebar {
  /** Marks the current file and the files of the lifted cluster for the current step. */
  update(): void;
  /** Moves the bracket to the part of the tree that the map shows now. */
  updateViewportBar(): void;
  destroy(): void;
}

export function createSidebar(ctx: DeckContext): Sidebar {
  const { model, state, signal } = ctx;
  const { rows, sideRows, clusters, CL, FN } = model;
  const sbTree = $("#sbTree"), vpBar = $("#vpBar");

  // Tree guides: one cell per ancestor (pipe or gap), then a tee or a rounded end for the row itself.
  const sideKids = (p: Row) => p.children.filter(k => k.kind !== "fn");
  const rootKids = rows.filter(r => !r.parent);
  const lastKid = (r: Row) => { const s = r.parent ? sideKids(r.parent) : rootKids; return s[s.length - 1] === r; };
  function guides(r: Row) {
    if (!r.parent) return "";
    const g: string[] = [];
    for (let a = r.parent; a.parent; a = a.parent) g.unshift(lastKid(a) ? "gap" : "pipe");
    g.push(lastKid(r) ? "end" : "tee");
    return `<span class="gds" aria-hidden="true">${g.map(k => `<i class="gd ${k}"></i>`).join("")}</span>`;
  }

  for (const r of sideRows) {
    const line = mk("button", "sl " + r.kind) as HTMLButtonElement;
    line.type = "button"; line.tabIndex = -1;
    let meta = "";
    if (r.kind === "file") {
      const n = model.fnsByFile.get(r.file!)?.length ?? 0;
      const has = model.clustersByFile.get(r.file!);
      const dots = has ? clusters.filter(c => has.has(c.id)).map(c => `<i data-c="${esc(c.id)}" style="--c:${c.cv}"></i>`).join("") : "";
      meta = `<span class="meta"><span class="dots" aria-hidden="true">${dots}</span><span class="cnt">${n} fn<span class="vh">, ${n === 1 ? "function" : "functions"}</span></span></span><span class="sfn"></span>`;
    }
    line.innerHTML = `${PTR}${guides(r)}<span class="snm">${esc(r.name)}</span>${meta}`;
    line.addEventListener("click", () => { if (openPath(state.open, r)) ctx.parts.world.refreshTree(); panToRow(r); }, { signal });
    r.sl = line; r.sfn = line.querySelector<HTMLElement>(".sfn");
    sbTree.appendChild(line);
  }
  if (sideRows[0]) sideRows[0].sl!.tabIndex = 0;
  sbTree.addEventListener("keydown", e => {
    const i = sideRows.findIndex(r => r.sl === document.activeElement);
    if (i < 0) return;
    const j = e.key === "ArrowDown" ? i + 1 : e.key === "ArrowUp" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? sideRows.length - 1 : null;
    if (j === null) return;
    e.preventDefault(); e.stopPropagation();
    const k = clamp(j, 0, sideRows.length - 1);
    sideRows[i].sl!.tabIndex = -1; sideRows[k].sl!.tabIndex = 0; sideRows[k].sl!.focus();
  }, { signal });

  function update() {
    const s = model.steps[state.cur - 1], cid = state.cur ? model.stepCluster(state.cur) : null;
    const fn = s && !s.missing ? FN.get(s.node)! : null, ff = fn ? fn.file : null;
    for (const r of sideRows) {
      if (r.kind === "file") {
        const inC = !!cid && !!model.clustersByFile.get(r.file!)?.has(cid);
        r.sl!.classList.toggle("inc", inC);
        r.sl!.classList.toggle("inact", !!cid && !inC);
        const isCur = r.file === ff;
        r.sl!.classList.toggle("cur", isCur);
        r.sfn!.textContent = isCur ? fn!.name : "";
        if (isCur) r.sl!.setAttribute("aria-current", "location"); else r.sl!.removeAttribute("aria-current");
        r.sl!.querySelectorAll<HTMLElement>(".dots i").forEach(i => i.classList.toggle("on", i.dataset.c === cid));
        if (isCur && sbTree.scrollHeight > sbTree.clientHeight) {
          const top = r.sl!.offsetTop, bot = top + r.sl!.offsetHeight;
          if (top < sbTree.scrollTop || bot > sbTree.scrollTop + sbTree.clientHeight) sbTree.scrollTop = top - sbTree.clientHeight / 2;
        }
      } else {
        const prefix = r.id.slice(4);
        const holds = !!cid && CL.get(cid)!.members.some(id => id.startsWith(prefix));
        r.sl!.classList.toggle("inact", !!cid && !holds);
      }
    }
  }

  function updateViewportBar() {
    const { cam, view } = state;
    const kc = cam.k * Math.cos(cam.tilt * RAD);
    const y0 = cam.Fy + (view.top - view.cy) / kc, y1 = cam.Fy + (view.bottom - view.cy) / kc;
    let first: RowView | null = null, last: RowView | null = null;
    for (const r of sideRows) if (r.blockEnd >= y0 && r.y <= y1 && r.mode !== "hidden") { if (!first) first = r; last = r; }
    if (!first || !last) { vpBar.style.opacity = "0"; return; }
    vpBar.style.opacity = "1";
    vpBar.style.transform = `translateY(${first.sl!.offsetTop}px) scaleY(${last.sl!.offsetTop + last.sl!.offsetHeight - first.sl!.offsetTop})`;
  }

  function panToRow(r: RowView) {
    const { cam } = state;
    const kc = Math.cos(cam.tilt * RAD);
    state.cam = { ...cam, Fx: r.x + 160, Fy: r.y + (state.liftedCluster ? -ctx.tokens.deckZ * Math.tan(cam.tilt * RAD) * 0.3 : 0) + 120 / (cam.k * kc) };
    ctx.parts.viewport.apply(true);
    if (!ctx.reducedMotion) { r.nodeEl.classList.remove("pulse"); void r.nodeEl.offsetWidth; r.nodeEl.classList.add("pulse"); }
  }

  return { update, updateViewportBar, destroy() { sbTree.replaceChildren(vpBar); } };
}

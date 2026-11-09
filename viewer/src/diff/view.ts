/* PR diff view. This module owns the overview of one PR diff (the summary with its legend, the Before | After rows,
   the step counts, the notes, and the open row) and the diff mode of the deck (the step diff in the code column,
   the overview button, the help line, and the bar text at rest). The deck plays the After flow. Step 0 of the deck
   shows the overview, and every other step shows the deck, so Escape and the clear button return to the overview.
   # Sections
   markup, geometry, drawing, note, deck entry, keys, code view, mode */
import type { DeckStep, DiffArtifact, DiffRow } from "../data/models.ts";
import type { CodeView } from "../deck/context.ts";
import { whereHtml } from "../deck/code.ts";
import { ledgerHtml, md } from "../deck/markup.ts";
import { highlight } from "../layout/highlight.ts";
import { clamp, esc, plural } from "../layout/util.ts";
import { buildDiffModel, KINDS, movedText, numberLines, slotsOf, type Counted, type OverviewRow, type Side, type SideStep, type Slot } from "./model.ts";

export interface DiffView {
  /** Shows the overview at step 0 and the deck at every other step. */
  sync(step: number): void;
  /** The step diff that replaces the source in the code column. */
  codeView(step: DeckStep): CodeView | null;
  /** Adds the overview button to the flow band of the deck. */
  band(): void;
  destroy(): void;
}

const $ = <T extends Element = HTMLElement>(selector: string) => document.querySelector(selector) as T;

/** The summary word of each counted kind, and the legend word. */
const WORD: Record<Counted, string> = { new: "new", removed: "removed", changed: "changed", moved: "moved" };
const LEGEND: [Counted, string][] = [["new", "new"], ["removed", "removed"], ["changed", "changed"], ["moved", "moved file"]];
/** Geometry of the overview: bar height, row gap, diagram top, numbers column width, and gutter width. */
const G = { bar: 54, gap: 18, top: 62, stats: 236, gutter: 86 };
/** Geometry of an open row: title bar, padding, step node, step node with a moved-file line, and the gap that holds each arrowhead. */
const R = { title: 42, pad: 20, node: 36, tall: 54, gap: 26 };
/** JetBrains Mono advances 0.6 em: the characters that fit a width at a font size. */
const fit = (width: number, px: number) => Math.max(4, Math.floor(width / (0.6 * px)));
const cut = (text: string, n: number) => (text.length > n ? `${text.slice(0, n - 1)}…` : text);
const fmt = (n: number) => n.toLocaleString("en-US");
const SIDE: Record<Side, "b" | "a"> = { before: "b", after: "a" };

/** The +added −removed line delta, each number in its git color. */
const delta = (added: number, removed: number) =>
  `${added ? `<tspan class="ln-add">+${fmt(added)}</tspan>` : ""}${added && removed ? " " : ""}${removed ? `<tspan class="ln-del">−${fmt(removed)}</tspan>` : ""}`;

/** The language of a path for the highlighter, or undefined for plain text. */
function languageOf(path: string | undefined): string | undefined {
  const ext = path?.slice(path.lastIndexOf(".") + 1);
  return ext === "rs" ? "rust" : ext === "py" ? "python" : ext && /^(ts|tsx|js|jsx|mjs|cjs)$/.test(ext) ? "typescript" : undefined;
}

export function createDiffView(diff: DiffArtifact, go: (step: number) => void): DiffView {
  const model = buildDiffModel(diff);
  const abort = new AbortController(), signal = abort.signal;
  let visible = false, last = 0, sel = -1, open = -1, tipRow = -1, tipTimer = 0;
  let geo: { W: number; H: number; side: number; bx: number; ax: number; bw: number; statsX: number; pos: { top: number; h: number; ty: number; end: number }[] } | null = null;
  const help = $("#help"), helpDeck = help.innerHTML;

  // ============ markup ============

  /** One sentence of step facts. */
  function summary(): string {
    const b = model.before.length, a = model.after.length, t = model.totals;
    const parts = KINDS.filter(k => t[k]).map(k => (k === "moved" ? `${t[k]} moved ${t[k] === 1 ? "file" : "files"}` : `${t[k]} ${WORD[k]}`));
    return [`<b>${b} steps ${b === a ? "stay" : "became"} ${a}</b>`, ...parts].join(" · ");
  }
  const samples = LEGEND.map(([k, text]) => `<span><i class="sample k-${k}"></i>${text}</span>`).join("");
  const short = (commit: string) => esc(commit.slice(0, 7));
  const root = document.createElement("div");
  root.className = "dv";
  root.hidden = true;
  root.innerHTML = `<header class="dv-band"><span class="pbadge"><span>${esc(diff.head.flow.flowId)}</span></span>`
    + `<span class="fh-meta">diff ${esc(diff.name)} · ${short(diff.base.commit)} → ${short(diff.head.commit)}</span></header>`
    + `<div class="dv-body"><div class="ov"><div class="osum-wrap" tabindex="0" aria-describedby="dvLegend"><p class="osum">${summary()}</p>`
    + `<div class="legend" id="dvLegend" role="tooltip"><div class="kinds">${samples}</div><div class="hint">chevron / space: open the row as a subgraph</div></div></div>`
    + `<div class="dwrap"><svg class="mp" role="group" aria-label="Before and After flows"></svg><div class="ntip" role="tooltip" hidden></div></div></div></div>`;
  document.body.appendChild(root);
  const body = root.querySelector<HTMLElement>(".dv-body")!, svg = root.querySelector<SVGSVGElement>(".mp")!, tip = root.querySelector<HTMLElement>(".ntip")!;

  // ============ geometry ============

  /** The step slots of an open row, and the height of a slot: a moved-file line makes it tall on both sides. */
  const slotsOfOpen = () => (open >= 0 ? slotsOf(model, model.rows[open]!) : []);
  const movedOn = (slot: Slot): Side | null => (movedText((slot.after ?? slot.before)?.row ?? null) ? (slot.after ? "after" : "before") : null);
  const slotH = (slot: Slot) => (movedOn(slot) ? R.tall : R.node);

  function layout() {
    const W = Math.max(960, body.clientWidth - 44), side = (W - G.gutter - G.stats) / 2;
    let y = G.top;
    const pos = model.rows.map((_, i) => {
      const slots = i === open ? slotsOfOpen() : null;
      const h = slots ? R.title + R.pad * 2 + slots.reduce((t, s) => t + slotH(s), 0) + Math.max(0, slots.length - 1) * R.gap : G.bar;
      const p = { top: y, h, ty: y + (slots ? R.title / 2 : h / 2), end: y + h };
      y = p.end + (slots ? G.gap + 4 : G.gap);
      return p;
    });
    return { W, H: (pos[pos.length - 1]?.end ?? G.top) + 26, side, bx: side / 2, ax: side + G.gutter + side / 2, bw: side - 34, statsX: W - G.stats + 26, pos };
  }

  // ============ drawing ============

  /** Draws the diagram: the side frames, the flow arrows between rows, the rows, and the numbers. */
  function draw() {
    const f = geo = layout();
    let s = `<defs><marker id="dvArrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto"><path d="M0 0 L10 5 L0 10"/></marker></defs>`;
    for (const [x, title, n] of [[f.bx, "Before", model.before.length], [f.ax, "After", model.after.length]] as const) {
      s += `<rect class="sg" x="${x - f.side / 2}" y=".5" width="${f.side}" height="${f.H - 1}"/><text class="column-label" x="${x - 30}" y="25">${title}</text><text class="column-count" x="${x + 45}" y="25">${plural(n, "step")}</text>`;
    }
    for (const side of ["before", "after"] as const) {
      let prev = -1;
      model.rows.forEach((row, i) => {
        if (!row[side]) return;
        if (prev >= 0) s += `<path class="fe" d="M${side === "before" ? f.bx : f.ax} ${f.pos[prev]!.end} V${f.pos[i]!.top}" marker-end="url(#dvArrow)"/>`;
        prev = i;
      });
    }
    model.rows.forEach((row, i) => { s += (i === open ? openRow(row, i) : bars(row, i)) + stats(row, i); });
    svg.setAttribute("width", String(f.W)); svg.setAttribute("height", String(f.H)); svg.setAttribute("viewBox", `0 0 ${f.W} ${f.H}`);
    svg.innerHTML = s;
    markSel(false);
  }

  const labelOf = (row: OverviewRow, side: Side) => (side === "after" && row.continues ? `${row.after!.label} · continues` : row[side]!.label);

  /** A chevron that opens or closes the row. */
  function chevron(row: OverviewRow, i: number, side: Side, x: number, y: number, isOpen: boolean) {
    const name = esc((row.after ?? row.before)!.label);
    return `<g class="expand-control" data-expand="${i}" data-side="${SIDE[side]}" tabindex="0" role="button" aria-expanded="${isOpen}" aria-label="${isOpen ? "Close" : "Open"} ${name}">`
      + `<rect x="${x - 12}" y="${y - 11}" width="24" height="22"/><path d="${isOpen ? `M${x - 4} ${y - 2} l4 4 l4 -4` : `M${x - 2} ${y - 4} l4 4 l-4 4`}"/></g>`;
  }

  /** A closed row: one neutral bar for each side that has a run. */
  function bars(row: OverviewRow, i: number) {
    const f = geo!, p = f.pos[i]!;
    let s = "";
    for (const side of ["before", "after"] as const) {
      if (!row[side]) continue;
      const c = side === "after" ? f.ax : f.bx, x = c - f.bw / 2, label = labelOf(row, side);
      s += `<g class="unit" data-row="${i}" data-side="${SIDE[side]}"><g class="nd" data-row="${i}" data-side="${SIDE[side]}" tabindex="0" role="button" aria-label="${esc(label)}. Enter opens the deck. Space opens the row.">`
        + `<rect class="ring" x="${x - 4}" y="${p.top - 4}" width="${f.bw + 8}" height="${p.h + 8}"/><rect class="box" x="${x}" y="${p.top}" width="${f.bw}" height="${p.h}"/>`
        + `<text class="node-name" x="${c - 6}" y="${p.top + p.h / 2}">${esc(cut(label, fit(f.bw - 64, 12)))}</text></g>${chevron(row, i, side, x + f.bw - 13, p.top + p.h / 2, false)}</g>`;
    }
    return s;
  }

  /** An open row: a neutral frame for each side, its step count in the title bar, and one line for each step slot. */
  function openRow(row: OverviewRow, i: number) {
    const f = geo!, p = f.pos[i]!, slots = slotsOfOpen(), cy = p.top + R.title / 2;
    let s = "";
    for (const side of ["before", "after"] as const) {
      const x = (side === "after" ? f.ax : f.bx) - f.bw / 2, run = row[side];
      const label = run ? labelOf(row, side) : side === "after" ? "No cluster after" : "No cluster before";
      s += `<g class="unit" data-row="${i}" data-side="${SIDE[side]}"><g class="nd frame" data-row="${i}" data-side="${SIDE[side]}" tabindex="0" role="button" aria-label="${esc(label)}, open. Enter opens the deck.">`
        + `<rect class="ring" x="${x - 4}" y="${p.top - 4}" width="${f.bw + 8}" height="${p.h + 8}"/><rect class="frame-body" x="${x}" y="${p.top}" width="${f.bw}" height="${p.h}"/>`
        + `<rect class="box" x="${x}" y="${p.top}" width="${f.bw}" height="${R.title}"/><path class="title-rule" d="M${x} ${p.top + R.title} H${x + f.bw}"/>`
        + `<text class="frame-title${run ? "" : " none"}" x="${x + 18}" y="${cy}">${esc(cut(label, fit(f.bw - 120, 13)))}</text>`
        + (run ? `<text class="frame-count" x="${x + f.bw - 34}" y="${cy}">${plural(run.steps.length, "step")}</text>` : "")
        + `</g>${chevron(row, i, side, x + f.bw - 14, cy, true)}</g>`;
      // the spine runs down the left of the side and skips empty slots
      const cx = x + 18, w = f.bw - 36, spine = cx + 22;
      let y = p.top + R.title + R.pad, prev: number | null = null;
      for (const slot of slots) {
        const h = slotH(slot), step = slot[side];
        if (!step) s += `<rect class="slot" x="${cx + 44}" y="${y + 0.5}" width="${w - 44}" height="${h - 1}"/>`;
        else {
          if (prev !== null) s += `<path class="fe" d="M${spine} ${prev} V${y}" marker-end="url(#dvArrow)"/>`;
          s += stepNode(step, cx, y, w, h, movedOn(slot) === side);
          prev = y + h;
        }
        y += h + R.gap;
      }
    }
    return s;
  }

  /** One step node in an open row: the name at left, the line delta at right, and the moved-file line under the name. */
  function stepNode(step: SideStep, x: number, y: number, w: number, h: number, moved: boolean) {
    const row = step.row, text = moved ? movedText(row) : null, ty = text ? y + 18 : y + h / 2;
    const lines = row && (step.side === "after" || step.kind === "removed") ? delta(row.added, row.removed) : "";
    const room = fit(w - 28 - (lines ? 110 : 0), 12.5);
    return `<g class="sn k-${step.kind}" data-side="${SIDE[step.side]}" data-step-id="${esc(step.id)}" tabindex="0" role="button" aria-label="${esc(step.title)}, ${esc(text ?? step.kind)}">`
      + `<rect x="${x}" y="${y}" width="${w}" height="${h}"/><text class="sname" x="${x + 14}" y="${ty}">${esc(cut(step.title, room))}</text>`
      + `<text class="sdelta" x="${x + w - 12}" y="${ty}">${lines}</text>`
      + (text ? `<text class="smove" x="${x + 14}" y="${y + 37}">${esc(cut(text, fit(w - 28, 11)))}</text>` : "") + `</g>`;
  }

  /** The numbers of one row, centered on its title line: the changed steps (two kinds a line), then the line delta.
   *  A row with no changed step shows nothing. */
  function stats(row: OverviewRow, i: number) {
    if (!row.changed) return "";
    const f = geo!, ks = KINDS.filter(k => row.counts[k]), out: string[] = [];
    for (let j = 0; j < ks.length; j += 2) out.push(ks.slice(j, j + 2).map(k => `<tspan class="n-${k}">${row.counts[k]}</tspan> ${WORD[k]}`).join(" · "));
    if (row.added || row.removed) out.push(delta(row.added, row.removed));
    return out.map((t, j) => `<text class="row-count" x="${f.statsX}" y="${f.pos[i]!.ty + (j - (out.length - 1) / 2) * 15}">${t}</text>`).join("");
  }

  /** Scrolls the overview body so that an element shows. The page and the band never move. */
  function reveal(el: Element | null) {
    if (!el) return;
    const a = el.getBoundingClientRect(), b = body.getBoundingClientRect();
    if (a.top < b.top + 12) body.scrollTop += a.top - b.top - 12;
    else if (a.bottom > b.bottom - 12) body.scrollTop += Math.min(a.bottom - b.bottom + 12, a.top - b.top - 12);
  }

  /** Rings the current row in blue, on its After bar when it has one. */
  function markSel(focus: boolean) {
    svg.querySelectorAll(".nd.cur").forEach(el => el.classList.remove("cur"));
    const row = model.rows[sel];
    if (!row) return;
    const el = svg.querySelector(`.nd[data-row="${sel}"][data-side="${row.after ? "a" : "b"}"]`);
    el?.classList.add("cur");
    if (focus) { (el as SVGElement | null)?.focus({ preventScroll: true }); reveal(el); }
  }

  /** Opens or closes one row. One row stays open at a time. */
  function toggle(i: number) {
    hideTip();
    sel = i; open = open === i ? -1 : i;
    draw();
    (svg.querySelector<SVGElement>(`[data-expand="${i}"][data-side="a"]`) ?? svg.querySelector<SVGElement>(`[data-expand="${i}"]`))?.focus({ preventScroll: true });
    if (open >= 0) reveal(svg.querySelector(`.nd[data-row="${i}"]`));
  }

  // ============ note ============

  /** Shows the note of a row in the numbers column, centered on its title line. The side under the pointer comes first. */
  function showTip(i: number, side: Side) {
    const row = model.rows[i], p = geo?.pos[i], other: Side = side === "after" ? "before" : "after";
    const pick = row?.notes[side] ? side : row?.notes[other] ? other : null;
    if (!row || !p || !pick) return;
    // a note that starts with its own word, such as "Now:", keeps it
    const m = /^(\w+):\s*([\s\S]*)$/.exec(row.notes[pick]!);
    tip.innerHTML = `<span class="note-word">${esc(m?.[1] ?? (pick === "after" ? "Now" : "Before"))}:</span> ${esc(m?.[2] ?? row.notes[pick])}`;
    tip.hidden = false;
    tip.style.left = `${geo!.statsX - 12}px`;
    tip.style.top = `${Math.round(clamp(p.ty - tip.offsetHeight / 2, 0, geo!.H - tip.offsetHeight))}px`;
    tipRow = i;
  }
  function queueTip(i: number, side: Side) {
    clearTimeout(tipTimer);
    if (tipRow !== i) tipTimer = window.setTimeout(() => showTip(i, side), 150);
  }
  function hideTip() { clearTimeout(tipTimer); tipRow = -1; tip.hidden = true; }
  const unitOf = (target: EventTarget | null) => (target as Element | null)?.closest?.<SVGElement>(".unit") ?? null;
  const sideOf = (el: Element) => ((el as SVGElement).dataset.side === "a" ? "after" : "before") as Side;
  svg.addEventListener("pointerover", e => { const u = unitOf(e.target); if (u) queueTip(+u.dataset.row!, sideOf(u)); else hideTip(); }, { signal });
  svg.addEventListener("pointerleave", hideTip, { signal });
  svg.addEventListener("focusin", e => { const u = unitOf(e.target); if (u) showTip(+u.dataset.row!, sideOf(u)); else hideTip(); }, { signal });
  svg.addEventListener("focusout", e => { if (!unitOf(e.relatedTarget)) hideTip(); }, { signal });

  // ============ deck entry ============
  // A bar opens the deck at the first changed After step of its row. A step node opens its After step. A removed step has
  // no After step: the deck opens at the entry of its row, and the code column shows the removed step and its diff.

  function enterRow(i: number) {
    const row = model.rows[i];
    if (!row) return;
    sel = i;
    go(row.entry);
  }

  function enterStep(el: SVGElement) {
    const list = el.dataset.side === "a" ? model.after : model.before;
    const step = list.find(s => s.id === el.dataset.stepId);
    if (!step) return;
    const after = model.afterOf(step);
    if (after) { go(after.index); return; }
    const at = model.rows[open]?.entry ?? 0;
    go(at);
    if (step.row && !visible) showRemoved(step, at);
  }

  /** Shows a removed step in the code column. The deck stays at its step; the next step replaces this view. */
  function showRemoved(step: SideStep, at: number) {
    const view = diffCode(step.row!);
    $("#cdHead").innerHTML = `<h2 class="cd-title"><span class="ctt">${esc(step.title)}</span><i class="cursor" aria-hidden="true"></i></h2>`;
    $("#cdStep").innerHTML = `${ledgerHtml(step.ledger, view.tag)}<p class="cd-note">Before step ${step.index} is not in the After flow. The map shows After step ${at}.</p>`;
    $("#where").innerHTML = view.where;
    const code = $("#code");
    code.innerHTML = view.html;
    code.scrollLeft = 0;
    const land = code.querySelector<HTMLElement>("[data-land]");
    code.scrollTop = land ? Math.max(0, land.offsetTop - 2 * land.offsetHeight) : 0;
  }

  svg.addEventListener("click", e => {
    const t = e.target as Element;
    const chev = t.closest<SVGElement>(".expand-control");
    if (chev) { toggle(+chev.dataset.expand!); return; }
    const sn = t.closest<SVGElement>(".sn");
    if (sn) { enterStep(sn); return; }
    const nd = t.closest<SVGElement>(".nd");
    if (nd) enterRow(+nd.dataset.row!);
  }, { signal });
  // Forward from the overview opens the first changed row.
  document.addEventListener("click", e => {
    if (!visible || !(e.target as Element).closest("#nextBtn")) return;
    e.stopImmediatePropagation();
    enterRow(Math.max(0, model.rows.findIndex(row => row.changed)));
  }, { capture: true, signal });

  // ============ keys ============
  // On the overview, j and k choose a row, Enter opens the deck, Space opens the row, and Escape closes it.
  // No other deck key acts while the overview shows.

  addEventListener("keydown", e => {
    if (!visible || e.altKey || e.metaKey || e.ctrlKey) return;
    const k = e.key, inView = root.contains(document.activeElement) || document.activeElement === document.body;
    if ((k === "Enter" || k === " ") && !inView) return;
    if (k.length !== 1 && !["Enter", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(k)) return;
    e.preventDefault(); e.stopImmediatePropagation();
    const focus = document.activeElement as SVGElement | null, row = focus?.dataset?.row ?? focus?.dataset?.expand;
    if (k === "k" || k === "ArrowDown" || k === "j" || k === "ArrowUp") {
      sel = clamp(sel + (k === "k" || k === "ArrowDown" ? 1 : -1), 0, model.rows.length - 1);
      markSel(true);
    } else if (k === "Enter") {
      if (focus?.matches?.(".sn")) enterStep(focus);
      else if (focus?.matches?.(".expand-control")) toggle(+row!);
      else enterRow(row !== undefined ? +row : Math.max(sel, 0));
    } else if (k === " ") toggle(row !== undefined ? +row : Math.max(sel, 0));
    else if (k === "Escape" && open >= 0) toggle(open);
  }, { capture: true, signal });
  addEventListener("resize", () => { if (visible) { hideTip(); draw(); } }, { signal });

  // ============ code view ============

  /** The step diff for the code column: path:line and the line delta in the header, then the unified diff with
   *  +/− marks and line numbers. The view lands on the first changed line. */
  function diffCode(row: DiffRow): CodeView {
    const span = row.headSpan ?? row.baseSpan, lines = numberLines(row), html = highlight(lines.map(l => l.text), languageOf(span?.path));
    const first = lines.findIndex(l => l.op !== "context");
    const body = lines.map((l, i) => {
      const op = l.op === "added" ? " ad" : l.op === "removed" ? " rm" : "";
      return `<div class="cl${op}"${i === first ? " data-land" : ""}><span class="no">${l.no ?? ""}</span><span class="mk">${l.op === "added" ? "+" : l.op === "removed" ? "−" : ""}</span><span class="tx">${html[i] || " "}</span></div>`;
    }).join("");
    const counts = row.added || row.removed ? `<span class="dstat">${delta(row.added, row.removed).replace(/tspan/g, "span")}</span>` : "";
    const text = movedText(row);
    return {
      where: span ? `<span class="dpath">${whereHtml(span.path, span.start)}</span>${counts}` : counts,
      html: lines.length ? `<div class="code-in diff">${body}</div>` : '<p class="cd-empty">The diff has no lines for this function.</p>',
      tag: row.kind === "unchanged" ? "" : `<span class="tag k-${row.kind}">${esc(text ?? row.kind)}</span>`,
    };
  }

  const afterById = new Map(model.after.map(step => [step.id, step]));

  // ============ mode ============

  const OVERVIEW_HELP = [["j/k", "choose"], ["enter", "open"], ["space", "open the row"], ["click", "a cluster or a step"]]
    .map(([k, t]) => `<span class="hk">${k}</span>${t}`).join('<i class="hsep" aria-hidden="true"></i>');

  /** The bar text on the overview. It runs after the deck renders its rest state. */
  function overviewBar() {
    if (!visible) return;
    const touched = model.after.filter(step => step.kind !== "unchanged").length;
    $("#counter").textContent = "Overview";
    $("#chip").classList.add("none");
    $("#chipText").textContent = `diff ${diff.name}`;
    $("#cMeta").textContent = `${touched} of ${plural(model.after.length, "step")} touched`;
    $("#cDesc").textContent = "Hover a cluster for its note. Click a chevron to open it. Click a cluster or a step to enter the deck.";
    $<HTMLButtonElement>("#nextBtn").disabled = !model.after.length;
  }

  function sync(step: number) {
    const was = visible;
    visible = step === 0;
    root.hidden = !visible;
    document.body.classList.toggle("dv-over", visible);
    for (const id of ["#card", "#maptools", "#fhTop", "#cv"]) ($(id) as HTMLElement & { inert: boolean }).inert = visible;
    $("#clearBtn").hidden = visible;
    help.innerHTML = visible ? OVERVIEW_HELP : helpDeck.replace(/(<span class="hk">esc<\/span>)clear/, "$1overview");
    if (visible) {
      if (!was && last) sel = model.rowOfAfter(last);
      draw();
      if (!was && last) markSel(true);
      queueMicrotask(overviewBar);
    } else hideTip();
    last = step;
  }

  return {
    sync,
    codeView(step) {
      const row = afterById.get(step.id)?.row;
      return row ? diffCode(row) : null;
    },
    band() {
      const meta = $("#fhMeta");
      meta.insertAdjacentHTML("beforeend", ` <button class="fh-back" type="button" id="dvBack" aria-label="Back to the overview (esc)">← overview <kbd>esc</kbd></button>`);
      $("#dvBack").addEventListener("click", () => go(0), { signal });
    },
    destroy() {
      abort.abort();
      clearTimeout(tipTimer);
      root.remove();
      help.innerHTML = helpDeck;
      $("#clearBtn").hidden = false;
      document.body.classList.remove("dv-over");
      for (const id of ["#card", "#maptools", "#fhTop", "#cv"]) ($(id) as HTMLElement & { inert: boolean }).inert = false;
    },
  };
}

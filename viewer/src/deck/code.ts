/* Deck code column. This module owns the code column: the step header, the annotation, the fact block, the code
   header (`path:line`), and the code viewer with its line numbers, colors, and margin marks. The facts come from
   `layout/step-facts.ts` and the lines from the `/source` answer. Every data string goes through `esc`.
   # Sections
   markup, empty states, facts, code viewer, step, events */
import type { DeckStep, SourceView } from "../data/models.ts";
import { displayName } from "../data/adapt.ts";
import { highlight } from "../layout/highlight.ts";
import { nearestStep } from "../layout/plan.ts";
import { callRefs, calleesOf, callersOf, lineSpan, oneLineSignature, otherVisits, sourceNote, transitionFact, whole, type CallRef } from "../layout/step-facts.ts";
import { clamp, plural } from "../layout/util.ts";
import { esc, md } from "./markup.ts";
import { $, type CodeColumn, type DeckContext } from "./context.ts";

// ============ markup ============

const SEP = '<span class="sep">, </span>';
const REST = "The flow is clear. Press `k` to start it.";
const NO_FLOW = "No saved flow for this codebase. Write one with `/tekdek:flow`, then reload.";

/** The header title with its cursor. */
const title = (text: string) => `<h2 class="cd-title"><span class="ctt">${esc(text)}</span><i class="cursor" aria-hidden="true"></i></h2>`;

/** The note that replaces code. It may hold `code` spans. */
const noteHtml = (text: string) => `<p class="cd-empty">${md(text)}</p>`;

/** The code header path: the folder dimmed, then the declaration line in orange when the code shows. */
function whereHtml(path: string, line: number | null): string {
  const cut = path.lastIndexOf("/") + 1;
  return `<span class="dir">${esc(path.slice(0, cut))}</span>${esc(path.slice(cut))}${line === null ? "" : `<span class="at">:${esc(line)}</span>`}`;
}

/** A call list: names as the graph gives them, file hints dimmed, never merged. A current cluster member takes its color and goes to its step. */
function refList(refs: readonly CallRef[]): string {
  if (!refs.length) return '<span class="none">none</span>';
  return refs.map(ref => {
    const label = `${esc(ref.label)}${ref.hint ? `<i> (${esc(ref.hint)})</i>` : ""}`;
    if (ref.step !== null) return `<button class="ref hit" type="button" data-step="${ref.step}" title="Go to step ${ref.step}">${label}</button>`;
    return `<span class="ref${ref.inCluster ? " hit" : ""}">${label}</span>`;
  }).join(SEP);
}

export function createCode(ctx: DeckContext): CodeColumn {
  const { model, state, signal } = ctx;
  const head = $("#cdHead"), stepEl = $("#cdStep"), whereEl = $("#where"), codeEl = $("#code");

  // ============ empty states ============

  /** The rest state, or the state of a codebase with no flow. */
  function renderEmpty() {
    head.innerHTML = title(model.flow ? "No step" : "No flow");
    stepEl.innerHTML = `<p class="cd-empty">${md(model.flow ? REST : NO_FLOW)}</p>`;
    whereEl.innerHTML = "";
    codeEl.innerHTML = "";
  }

  // ============ facts ============

  /** The display name of a step function, also when the graph no longer has it. */
  const nameOf = (step: DeckStep) => model.FN.get(step.node)?.name ?? displayName(step.node, step.node);

  /** The "from" fact: the flow start tag, or the transition from the previous step. Null when the flow gives no transition. */
  function fromFact(step: DeckStep): [string, string] | null {
    const prev = model.steps[step.index - 2];
    const fact = transitionFact(step.index === 1 ? "flow_start" : step.transition, prev ? nameOf(prev) : null, nameOf(step));
    if (fact.tag === "flow start") return ["from", '<span class="tag">flow start</span>'];
    if (!fact.tag && !fact.text) return null;
    return [`from ${step.index - 1}`, `${fact.tag === "gap" ? '<span class="tag gap">gap</span>' : ""}${esc(fact.text)}`];
  }

  /** The call lists and the other visits of the current step function. */
  function callFacts(step: DeckStep): [string, string][] {
    const cluster = model.stepCluster(state.cur);
    const members = new Set(cluster ? model.members.get(cluster) ?? [] : []);
    const stepOf = (key: string) => nearestStep(model.plan, key, state.cur);
    const refs = (keys: string[]) => refList(callRefs(keys, model.FN, members, stepOf, state.cur));
    const others = otherVisits(model.plan.visits.get(step.node) ?? [], step.index);
    return [
      ["called by", refs(callersOf(step.node, model.graph.calls))],
      ["calls", refs(calleesOf(step.node, model.graph.calls))],
      ["visits", others.length
        ? `this step and ${others.map(i => `<button class="ref hit" type="button" data-step="${i}">step ${i}</button>`).join(SEP)}`
        : "once, at this step"],
    ];
  }

  // ============ code viewer ============

  /** Draws the lines of an `ok` source and returns the margin marks that it drew. It lands on the declaration. */
  function renderCode(view: SourceView, lines: readonly string[]): { doc: boolean; sig: boolean } {
    const first = whole(view.firstLine, 1), more = whole(view.more, 0);
    const declIdx = clamp(whole(view.declarationLine, first) - first, 0, lines.length - 1);
    const html = highlight(lines, view.language);
    const rows = lines.map((_, i) => `<div class="cl${i === declIdx ? " where" : ""}"><span class="no">${first + i}</span><span class="mk"></span><span class="tx">${html[i] || " "}</span></div>`);
    if (more > 0) rows.push(`<div class="cl end"><span class="no">⋮</span><span class="mk"></span><span class="tx">${plural(more, "more line")} in this function</span></div>`);
    codeEl.innerHTML = `<div class="code-in"><div class="gut"></div>${rows.join("")}</div>`;
    const gut = codeEl.querySelector(".gut")!, els = codeEl.querySelectorAll<HTMLElement>(".cl");
    /** Draws one margin mark over a line range. Returns false when the range is not in the lines. */
    const mark = (range: readonly [number, number] | null | undefined, name: "doc" | "signature"): boolean => {
      const span = lineSpan(range, first, lines.length);
      if (!span) return false;
      const { from, to } = span, count = to - from + 1;
      const a = els[from], b = els[to];
      const m = document.createElement("div");
      m.className = "mark";
      m.style.top = `${a.offsetTop + 2}px`;
      m.style.height = `${Math.max(8, b.offsetTop + b.offsetHeight - a.offsetTop - 4)}px`;
      m.title = `${name}: line${count > 1 ? "s" : ""} ${first + from}${count > 1 ? `–${first + to}` : ""}`;
      m.dataset.mark = name;
      m.innerHTML = `<span>${name === "signature" && count < 3 ? "sig" : name}</span>`;
      gut.appendChild(m);
      return true;
    };
    const marks = { doc: mark(view.docLines, "doc"), sig: mark(view.signatureLines, "signature") };
    // land on the declaration, with up to two lines of context above it
    const decl = els[declIdx];
    codeEl.scrollLeft = 0;
    codeEl.scrollTop = Math.max(0, decl.offsetTop - Math.min(2, declIdx) * decl.offsetHeight);
    return marks;
  }

  // ============ step ============

  /** Restarts the 240 ms fade of the step text and the code on a step change, except at boot or with reduced motion. */
  function swap(prev: number) {
    if (prev === state.cur || ctx.reducedMotion || document.body.classList.contains("booting")) return;
    for (const el of [stepEl, codeEl]) { el.classList.remove("swap"); void el.offsetWidth; el.classList.add("swap"); }
  }

  function render(prev: number) {
    const step = model.stepAt(state.cur);
    if (!step) { renderEmpty(); return; }
    head.innerHTML = `<span class="cd-no" aria-label="Step ${step.index}">${step.index}</span>${title(step.title)}`;
    const facts: [string, string][] = [];
    const from = fromFact(step);
    if (from) facts.push(from);
    const fn = model.fnAt(state.cur);
    if (!fn) {
      // a step function that the graph no longer has: the note in place of code, and no facts beyond "from"
      whereEl.innerHTML = "";
      codeEl.innerHTML = noteHtml(sourceNote({ key: step.node, status: "missing" }) ?? "");
    } else {
      const view = model.sources.get(step.node), path = view?.path ?? fn.file;
      const note = sourceNote(view);
      let marks = { doc: false, sig: false };
      if (note !== null || !view?.lines) {
        whereEl.innerHTML = whereHtml(path, null);
        codeEl.innerHTML = noteHtml(note ?? "");
      } else {
        whereEl.innerHTML = whereHtml(path, whole(view.declarationLine, whole(view.firstLine, 0)) || null);
        marks = renderCode(view, view.lines);
      }
      facts.push(...callFacts(step));
      // doc and signature show as fact rows only when the code has no margin mark for them
      if (step.documentation && !marks.doc) facts.push(["doc", esc(step.documentation)]);
      if (step.signature && !marks.sig) facts.push(["signature", `<code>${esc(oneLineSignature(step.signature))}</code>`]);
    }
    stepEl.innerHTML = `<p class="cd-ann">${md(step.annotation)}</p><dl class="cd-facts">${facts.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>`;
    swap(prev);
  }

  // ============ events ============

  codeEl.addEventListener("scroll", () => ctx.kick(0), { signal });

  return {
    render,
    destroy() {
      for (const el of [head, stepEl, whereEl, codeEl]) el.replaceChildren();
      stepEl.classList.remove("swap"); codeEl.classList.remove("swap");
    },
  };
}

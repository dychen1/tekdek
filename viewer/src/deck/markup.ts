/* Deck markup. This module owns the small HTML builders that the deck modules share: icons, the pointer,
   inline code, identifier chips, ledger rows, and the two picker lists. Every builder escapes the data that it writes.
   # Sections
   helpers, ledger, pickers */
import type { ArtifactEntry, FlowEntry, StepLedger } from "../data/models.ts";
import { esc, plural } from "../layout/util.ts";

// ============ helpers ============

export const icon = (name: string) => `<svg class="ic" aria-hidden="true"><use href="#i-${name}"/></svg>`;
export const PTR = '<svg class="ic ptr" aria-hidden="true"><use href="#i-ptr"/></svg>';
/** Escapes text, then turns `x` into <code>x</code> and **x** into <strong>x</strong>. */
export const md = (text: string) => esc(text).replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");

// ============ ledger ============
// A mark is a backtick span `kind:name`. The chip carries the kind color and a kind mark; the name stays ink.

const FILE_ICON = '<svg width="8" height="10" viewBox="0 0 9 11" aria-hidden="true"><path d="M.6.6h5l2.8 2.8v7H.6z M5.6.6v2.8h2.8"/></svg>';
const VALUE_ICON = '<svg width="8" height="8" viewBox="0 0 8 8" aria-hidden="true"><path d="M4 .8 7.2 4 4 7.2 .8 4Z"/></svg>';
const KIND_MARK: Record<string, string> = { fn: "ƒ", type: "T", value: VALUE_ICON, file: FILE_ICON, field: "" };
const KIND_NAME: Record<string, string> = { fn: "function", type: "type", value: "value", file: "file", field: "field" };

/** Returns one identifier chip. */
export const chip = (kind: string, name: string) =>
  `<span class="id k-${kind}" title="${KIND_NAME[kind]}">${KIND_MARK[kind] ? `<i aria-hidden="true">${KIND_MARK[kind]}</i>` : ""}${esc(name)}</span>`;

/** Escapes one ledger line and turns each `kind:name` mark into a chip. A span without a known kind stays inline code. */
export function marks(text: string): string {
  return text.split("`").map((part, i) => {
    if (i % 2 === 0) return esc(part);
    const at = part.indexOf(":"), kind = part.slice(0, at);
    return at > 0 && Object.hasOwn(KIND_MARK, kind) ? chip(kind, part.slice(at + 1)) : `<code>${esc(part)}</code>`;
  }).join("");
}

/** Renders label rows. A row with one item reads inline; a row with more items is a bullet list. Empty rows drop out.
 *  `lead` goes before the first value. */
export function ledgerRows(rows: readonly (readonly [string, readonly string[] | null])[], lead = ""): string {
  return rows.filter(([, items]) => items?.length).map(([label, items], i) => {
    const start = i === 0 ? lead : "";
    const value = items!.length === 1 ? `${start}${marks(items![0])}` : `${start}<ul>${items!.map(item => `<li>${marks(item)}</li>`).join("")}</ul>`;
    return `<dt>${label}</dt><dd>${value}</dd>`;
  }).join("");
}

/** Renders a step ledger in slot order. */
export function ledgerHtml(ledger: StepLedger, lead = ""): string {
  const rows = ledgerRows([
    ["takes", ledger.takes], ["requires", ledger.requires], ["does", ledger.does],
    ["on fail", ledger.onFail], ["returns", ledger.returns], ["writes", ledger.writes],
  ], lead);
  return `<dl class="lg">${rows || `<dt></dt><dd>${lead}</dd>`}</dl>`;
}

// ============ pickers ============

/** The flow list of one codebase. */
export function flowPicker(flows: readonly FlowEntry[], current: string | null): string {
  const items = flows.map(flow => {
    const steps = flow.stepCount === undefined ? "" : `${plural(flow.stepCount, "step")} · `;
    return `<li><button class="pk-item" type="button" data-flow="${esc(flow.name)}" aria-current="${flow.name === current}">` +
      `<span class="pk-name">${esc(flow.name)}</span>${flow.stale ? '<span class="pk-flag">stale</span>' : ""}` +
      `<span class="pk-meta">${steps}${plural(flow.clusterCount ?? 0, "cluster")}</span></button></li>`;
  }).join("");
  return `<p class="pk-hd">flows</p><ul class="pk-list">${items}</ul>`;
}

/** The list of indexed codebases. `flowCount` counts the flows that the viewer shows. */
export function codebasePicker(artifacts: readonly ArtifactEntry[], current: string | null, flowCount: (entry: ArtifactEntry) => number): string {
  const items = artifacts.map(entry => `<li><button class="pk-item" type="button" data-artifact="${esc(entry.file)}" aria-current="${entry.file === current}">` +
    `<span class="pk-name">${esc(entry.label ?? entry.file)}</span><span class="pk-meta">${esc(entry.file)} · ${plural(flowCount(entry), "flow")}</span></button></li>`).join("");
  return `<p class="pk-hd">codebases</p><ul class="pk-list">${items}</ul>`;
}

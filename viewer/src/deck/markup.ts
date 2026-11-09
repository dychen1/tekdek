/* Deck markup. This module owns the small HTML builders that the deck modules share: icons, the pointer,
   inline code, and the two picker lists. Every builder escapes the data that it writes.
   # Sections
   helpers, pickers */
import type { ArtifactEntry, FlowEntry } from "../data/models.ts";
import { esc, plural } from "../layout/util.ts";

// ============ helpers ============

export const icon = (name: string) => `<svg class="ic" aria-hidden="true"><use href="#i-${name}"/></svg>`;
export const PTR = '<svg class="ic ptr" aria-hidden="true"><use href="#i-ptr"/></svg>';
/** Escapes text, then turns `x` into <code>x</code> and **x** into <strong>x</strong>. */
export const md = (text: string) => esc(text).replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");

/** Renders step text as blocks. Each run of `- ` lines is a list, each run of `1. ` lines is a numbered list, and
 *  each run of `|` rows with a `|---|` second row is a table. A list or table needs no blank line around it. Other
 *  lines form paragraphs: a blank line starts a new one, and a line feed is a line break. `lead` goes at the start
 *  of the first paragraph, or before the first block. */
export function mdBlock(text: string, lead = ""): string {
  const kind = (line: string) => /^[-*] /.test(line) ? "ul" : /^\d+[.)] /.test(line) ? "ol" : line.startsWith("|") ? "table" : line ? "p" : "";
  const cells = (row: string) => row.replace(/^\||\|$/g, "").split("|").map(cell => md(cell.trim()));
  const runs: { kind: string; lines: string[] }[] = [];
  for (const line of text.split("\n").map(line => line.trim())) {
    const k = kind(line), last = runs[runs.length - 1];
    if (!k) { if (last) runs.push({ kind: "", lines: [] }); continue; }
    if (last?.kind === k) last.lines.push(line); else runs.push({ kind: k, lines: [line] });
  }
  let first = true;
  const html = runs.filter(run => run.kind).map(({ kind: k, lines }) => {
    const start = first ? lead : ""; first = false;
    if (k === "ul") return `${start}<ul>${lines.map(line => `<li>${md(line.slice(2))}</li>`).join("")}</ul>`;
    if (k === "ol") return `${start}<ol>${lines.map(line => `<li>${md(line.replace(/^\d+[.)] /, ""))}</li>`).join("")}</ol>`;
    if (k === "table" && lines.length >= 2 && /^\|[\s:|-]+\|?$/.test(lines[1])) {
      const head = cells(lines[0]).map(cell => `<th>${cell}</th>`).join("");
      const body = lines.slice(2).map(row => `<tr>${cells(row).map(cell => `<td>${cell}</td>`).join("")}</tr>`).join("");
      return `${start}<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
    }
    return `<p>${start}${lines.map(md).join("<br>")}</p>`;
  }).join("");
  return html || (lead ? `<p>${lead}</p>` : "");
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

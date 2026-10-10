/* Deck markup. This module owns the small HTML builders that the deck modules share: icons, the pointer,
   inline code, and the two picker lists. Every builder escapes the data that it writes.
   # Sections
   helpers, pickers */
import type { ArtifactEntry, FlowEntry } from "../data/models.ts";
import { esc, plural } from "../layout/util.ts";

export { esc } from "../layout/util.ts";

// ============ helpers ============

export const icon = (name: string) => `<svg class="ic" aria-hidden="true"><use href="#i-${name}"/></svg>`;
export const PTR = '<svg class="ic ptr" aria-hidden="true"><use href="#i-ptr"/></svg>';
/** Escapes text, then turns `x` into <code>x</code>. */
export const md = (text: string) => esc(text).replace(/`([^`]+)`/g, "<code>$1</code>");

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

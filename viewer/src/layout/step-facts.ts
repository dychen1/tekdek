/* Step facts. This module owns the text of the code column's fact block: the transition from the previous step,
   the call lists, the other visits, and the note that replaces code when the source cannot be shown.
   It returns plain text and plain records; the code column escapes them. */
import type { DeckCall, DeckFunction, SourceView, Transition } from "../data/models.ts";

/** The sentence for the transition into the current step. The flow start and a gap also carry a tag. */
export function transitionFact(transition: Transition | null, prev: string | null, cur: string): { tag: "flow start" | "gap" | null; text: string } {
  const p = prev ?? "the previous step";
  switch (transition) {
    case "flow_start": return { tag: "flow start", text: "" };
    case "same_function": return { tag: null, text: `stays in ${cur}` };
    case "calls": return { tag: null, text: `${p} calls ${cur}` };
    case "returns_to_caller": return { tag: null, text: `returns to the caller ${cur}` };
    case "sibling_call": return { tag: null, text: `${p} and ${cur} share a caller` };
    case "calls_indirectly": return { tag: null, text: `${p} reaches ${cur} through other calls` };
    case "returns_to_ancestor": return { tag: null, text: `returns from ${p} to an earlier caller, ${cur}` };
    case "no_static_edge": return { tag: "gap", text: `no static call from ${p} to ${cur}` };
    default: return { tag: null, text: "" };
  }
}

/** One name in a call list. `step` is set only for a current cluster member that is not the current step. */
export interface CallRef { key: string; label: string; hint: string | null; step: number | null; inCluster: boolean }

/** Names as the graph gives them, never merged. Equal names get their file as a hint. */
export function callRefs(keys: readonly string[], fns: ReadonlyMap<string, DeckFunction>, members: ReadonlySet<string>,
  stepOf: (key: string) => number | null, current: number): CallRef[] {
  const named = keys.map(key => ({ key, fn: fns.get(key) })).filter((item): item is { key: string; fn: DeckFunction } => !!item.fn);
  const counts = new Map<string, number>();
  for (const { fn } of named) counts.set(fn.name, (counts.get(fn.name) ?? 0) + 1);
  return named.map(({ key, fn }) => {
    const inCluster = members.has(key), step = inCluster ? stepOf(key) : null;
    return { key, label: fn.name, hint: (counts.get(fn.name) ?? 0) > 1 ? fn.file : null, step: step !== null && step !== current ? step : null, inCluster };
  });
}

const distinct = (keys: string[]) => [...new Set(keys)];
/** The distinct callers of a function, in graph order. */
export const callersOf = (key: string, calls: readonly DeckCall[]) => distinct(calls.filter(c => c.target === key).map(c => c.source));
/** The distinct callees of a function, in graph order. */
export const calleesOf = (key: string, calls: readonly DeckCall[]) => distinct(calls.filter(c => c.source === key).map(c => c.target));
/** The visits of the function other than the current step. */
export const otherVisits = (visits: readonly number[], current: number) => visits.filter(i => i !== current);

/** The reason that the server sends for a function whose graph node has no source range. */
const NO_SOURCE_RANGE = "the graph has no source range for this function";

/** The note that replaces code, or null when the source can show. The text may hold `code` spans. */
export function sourceNote(view: SourceView | undefined): string | null {
  if (!view) return "This file cannot be read: the viewer server did not send it.";
  switch (view.status) {
    case "ok": return null;
    case "missing": return "This function is not in the current graph.";
    case "changed": return "This file changed after the last index. Run `tekdek index` to see this function.";
    default:
      // The file may be readable here, so the note names the function and not the file.
      if (view.reason === NO_SOURCE_RANGE) return "This function cannot be shown: the graph has no source range for it.";
      return `This file cannot be read: ${view.reason ?? "unknown reason"}.`;
  }
}

/** A signature on one line: no space after "(", no trailing comma before ")", one space between words. */
export const oneLineSignature = (signature: string) => signature.replace(/\(\s+/g, "(").replace(/,\s*\)/g, ")").replace(/\s+/g, " ");

// Tests for the step facts: transition sentences, call lists, visits, source notes, and code line numbers.

import assert from "node:assert/strict";
import test from "node:test";

import type { DeckFunction } from "../src/data/models.ts";
import { calleesOf, callersOf, callRefs, lineSpan, oneLineSignature, otherVisits, sourceNote, transitionFact, whole } from "../src/layout/step-facts.ts";

test("each transition reads as one sentence, with a tag at the flow start and at a gap", () => {
  assert.deepEqual(transitionFact("flow_start", null, "main"), { tag: "flow start", text: "" });
  assert.deepEqual(transitionFact("calls", "main", "run"), { tag: null, text: "main calls run" });
  assert.deepEqual(transitionFact("sibling_call", "parse", "execute"), { tag: null, text: "parse and execute share a caller" });
  assert.deepEqual(transitionFact("returns_to_caller", "digests", "verify_artifact"), { tag: null, text: "returns to the caller verify_artifact" });
  assert.deepEqual(transitionFact("same_function", "run", "run"), { tag: null, text: "stays in run" });
  assert.deepEqual(transitionFact("calls_indirectly", "a", "b"), { tag: null, text: "a reaches b through other calls" });
  assert.deepEqual(transitionFact("returns_to_ancestor", "a", "b"), { tag: null, text: "returns from a to an earlier caller, b" });
  assert.deepEqual(transitionFact("no_static_edge", "fact_layer", "freeze"), { tag: "gap", text: "no static call from fact_layer to freeze" });
  assert.deepEqual(transitionFact(null, "a", "b"), { tag: null, text: "" });
});

const fn = (id: string, name: string, file: string): DeckFunction => ({ id, name, file, cluster: null });
const fns = new Map([fn("a::run", "run", "src/cli/mod.rs"), fn("b::run", "run", "src/cli/flow.rs"), fn("c::emit", "emit", "src/cli/mod.rs")].map(f => [f.id, f]));

test("call lists keep every name, add the file only to equal names, and link current cluster members", () => {
  const refs = callRefs(["a::run", "b::run", "c::emit"], fns, new Set(["c::emit"]), key => (key === "c::emit" ? 25 : null), 3);
  assert.deepEqual(refs.map(r => [r.label, r.hint, r.inCluster, r.step]), [
    ["run", "src/cli/mod.rs", false, null],
    ["run", "src/cli/flow.rs", false, null],
    ["emit", null, true, 25],
  ]);
  assert.equal(callRefs(["c::emit"], fns, new Set(["c::emit"]), () => 3, 3)[0].step, null);
});

test("callers and callees are distinct keys in graph order", () => {
  const calls = [{ source: "a::run", target: "c::emit" }, { source: "b::run", target: "c::emit" }, { source: "c::emit", target: "b::run" }];
  assert.deepEqual(callersOf("c::emit", calls), ["a::run", "b::run"]);
  assert.deepEqual(calleesOf("c::emit", calls), ["b::run"]);
  assert.deepEqual(otherVisits([3, 7, 12], 7), [3, 12]);
});

test("a missing, changed, unreadable, or rangeless source becomes one note in place of code", () => {
  assert.equal(sourceNote({ key: "k", status: "ok", lines: ["fn a() {}"] }), null);
  assert.equal(sourceNote({ key: "k", status: "missing" }), "This function is not in the current graph.");
  assert.equal(sourceNote({ key: "k", status: "changed", path: "src/a.rs" }), "This file changed after the last index. Run `tekdek index` to see this function.");
  assert.equal(sourceNote({ key: "k", status: "unreadable", reason: "the file is missing" }), "This file cannot be read: the file is missing.");
  assert.equal(sourceNote({ key: "k", status: "unreadable" }), "This file cannot be read: unknown reason.");
  assert.equal(
    sourceNote({ key: "k", status: "no_range", path: "src/a.rs" }),
    "This function cannot be shown: the graph has no source range for it.",
  );
  assert.equal(sourceNote(undefined), "This file cannot be read: the viewer server did not send it.");
});

test("an ok source with no lines becomes the unreadable note", () => {
  assert.equal(sourceNote({ key: "k", status: "ok" }), "This file cannot be read: the viewer server sent no lines.");
  assert.equal(sourceNote({ key: "k", status: "ok", lines: [] }), "This file cannot be read: the viewer server sent no lines.");
});

test("a line number from the source answer is a whole number, else the fallback", () => {
  assert.equal(whole(12, 1), 12);
  assert.equal(whole(1.5, 1), 1);
  assert.equal(whole("12", 1), 1);
  assert.equal(whole(undefined, 0), 0);
});

test("a line range maps to the shown lines and stops at the last one", () => {
  assert.deepEqual(lineSpan([12, 14], 10, 20), { from: 2, to: 4 });
  assert.deepEqual(lineSpan([12, 40], 10, 5), { from: 2, to: 4 });
  assert.deepEqual(lineSpan([10, 10], 10, 5), { from: 0, to: 0 });
  assert.equal(lineSpan([8, 12], 10, 5), null, "a range that starts above the shown lines");
  assert.equal(lineSpan([16, 18], 10, 5), null, "a range that starts below the shown lines");
  assert.equal(lineSpan([14, 12], 10, 5), null, "a range that ends before it starts");
  assert.equal(lineSpan(null, 10, 5), null);
  assert.equal(lineSpan(undefined, 10, 5), null);
});

test("a signature reads on one line with no padding inside the parentheses", () => {
  assert.equal(oneLineSignature("fn run(\n    intent: &Intent,\n    store: &Store,\n) -> Result<()>"), "fn run(intent: &Intent, store: &Store) -> Result<()>");
  assert.equal(oneLineSignature("fn emit()  ->  u8"), "fn emit() -> u8");
  assert.equal(oneLineSignature("fn a(x: u8)"), "fn a(x: u8)");
});

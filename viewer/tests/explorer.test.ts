// Tests for the explorer rows: tree order, guides, and open state.

import assert from "node:assert/strict";
import test from "node:test";

import { fileTree } from "../src/data/adapt.ts";
import { anchorRow, buildExplorer, dirNote, guideKinds, isShown, openTo } from "../src/layout/explorer.ts";

import { must } from "./fixtures.ts";

const rows = buildExplorer(fileTree(["README.md", "src/main.rs", "src/cli/mod.rs", "src/cli/flow.rs", "src/store/graph.rs"]));
const row = (path: string) => must(rows.find(r => r.path === path), path);

test("the explorer lists files before subdirectories, with tree guides", () => {
  assert.deepEqual(rows.map(r => r.path), ["README.md", "src/", "src/main.rs", "src/cli/", "src/cli/flow.rs", "src/cli/mod.rs", "src/store/", "src/store/graph.rs"]);
  assert.deepEqual(guideKinds(row("README.md")), []);
  assert.deepEqual(guideKinds(row("src/cli/flow.rs")), ["pipe", "tee"]);
  assert.deepEqual(guideKinds(row("src/store/graph.rs")), ["gap", "end"]);
  assert.deepEqual(row("src/cli/").files, ["src/cli/flow.rs", "src/cli/mod.rs"]);
});

test("opening a path shows the row, and a closed directory anchors its files", () => {
  const open = new Set<string>();
  assert.equal(isShown(row("src/cli/flow.rs"), open), false);
  openTo(open, row("src/cli/flow.rs"));
  openTo(open, row("src/cli/flow.rs"));
  assert.deepEqual([...open].sort(), ["src/", "src/cli/"]);
  assert.equal(isShown(row("src/cli/flow.rs"), open), true);
  assert.equal(anchorRow(row("src/store/graph.rs"), open), row("src/store/"));
  assert.equal(dirNote(row("src/store/"), open), "+1 file");
  assert.equal(dirNote(row("src/cli/"), open), "");
});

import assert from "node:assert/strict";
import { test } from "node:test";

import { mdBlock } from "../src/deck/markup.ts";

test("step text renders paragraphs, line breaks, lists, and tables", () => {
  assert.equal(mdBlock("One line.\nNext `x`."), "<p>One line.<br>Next <code>x</code>.</p>");
  assert.equal(mdBlock("Lead.\n\n- a\n- **b**"), "<p>Lead.</p><ul><li>a</li><li><strong>b</strong></li></ul>");
  assert.equal(mdBlock("1. a\n2. b"), "<ol><li>a</li><li>b</li></ol>");
  assert.equal(mdBlock("Stops when:\n- a\n- b\nThen ends."), "<p>Stops when:</p><ul><li>a</li><li>b</li></ul><p>Then ends.</p>");
  assert.equal(mdBlock("| Case | Result |\n|---|---|\n| bad | stop |"),
    "<table><thead><tr><th>Case</th><th>Result</th></tr></thead><tbody><tr><td>bad</td><td>stop</td></tr></tbody></table>");
});

test("step text escapes markup and puts the lead in the first paragraph", () => {
  assert.equal(mdBlock("<b>x</b>", "<span>tag</span>"), "<p><span>tag</span>&lt;b&gt;x&lt;/b&gt;</p>");
  assert.equal(mdBlock("- a", "<i>t</i>"), "<i>t</i><ul><li>a</li></ul>");
  assert.equal(mdBlock("", "<i>t</i>"), "<p><i>t</i></p>");
});

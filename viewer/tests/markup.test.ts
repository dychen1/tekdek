import { test } from "node:test";
import assert from "node:assert/strict";

import { ledgerHtml, ledgerRows, marks } from "../src/deck/markup.ts";

const EMPTY = { takes: null, requires: null, does: null, onFail: null, returns: null, writes: null };

test("marks turn kind spans into chips and escape the rest", () => {
  assert.equal(marks("calls `fn:run` <now>"), 'calls <span class="id k-fn" title="function"><i aria-hidden="true">ƒ</i>run</span> &lt;now&gt;');
  assert.equal(marks("`field:texts.unreadable`"), '<span class="id k-field" title="field">texts.unreadable</span>');
  assert.equal(marks("`plain` and `constructor:x`"), "<code>plain</code> and <code>constructor:x</code>");
});

test("ledger rows read inline for one item and as bullets for more", () => {
  assert.equal(ledgerRows([["rule", ["One."]], ["none", null], ["rules", ["a", "b"]]]),
    "<dt>rule</dt><dd>One.</dd><dt>rules</dt><dd><ul><li>a</li><li>b</li></ul></dd>");
});

test("a step ledger keeps slot order and puts the lead before the first value", () => {
  const html = ledgerHtml({ ...EMPTY, writes: ["w"], takes: ["t"], onFail: ["f"] }, "<i>tag</i>");
  assert.equal(html, '<dl class="lg"><dt>takes</dt><dd><i>tag</i>t</dd><dt>on fail</dt><dd>f</dd><dt>writes</dt><dd>w</dd></dl>');
});

// Tests for the syntax colors of the code viewer.

import assert from "node:assert/strict";
import test from "node:test";

import { highlight } from "../src/layout/highlight.ts";

test("Rust keywords, function names, comments, attributes, and multi-line strings take their colors", () => {
  const [doc, attr, sig, open, close] = highlight(["/// Reads <it>.", "#[must_use]", "pub fn read(path: &str) -> String {", "    let s = \"a", "b\";"], "rust");
  assert.equal(doc, '<span class="t-com">/// Reads &lt;it&gt;.</span>');
  assert.equal(attr, '<span class="t-attr">#[must_use]</span>');
  assert.ok(sig.startsWith('<span class="t-kw">pub</span> <span class="t-kw">fn</span> <span class="t-fn">read</span>'));
  assert.ok(sig.includes('<span class="t-type">String</span>'));
  assert.ok(open.endsWith('<span class="t-str">&quot;a</span>'));
  assert.ok(close.startsWith('<span class="t-str">b&quot;</span>'));
});

test("Python decorators, definitions, comments, and triple-quoted strings take their colors", () => {
  const [deco, def, doc1, doc2, comment] = highlight(["@cache", "def load(path):", '    """Load', '    it."""', "    # done"], "python");
  assert.equal(deco, '<span class="t-attr">@cache</span>');
  assert.ok(def.startsWith('<span class="t-kw">def</span> <span class="t-fn">load</span>'));
  assert.ok(doc1.includes('<span class="t-str">&quot;&quot;&quot;Load</span>'));
  assert.equal(doc2, '<span class="t-str">    it.&quot;&quot;&quot;</span>');
  assert.ok(comment.endsWith('<span class="t-com"># done</span>'));
});

test("prefixed triple-quoted Python strings open, carry, and close like plain ones", () => {
  const [open, mid, close, code] = highlight(['    r"""Raw', "    more", '    done"""', "    x = 1"], "python");
  assert.ok(open.includes('<span class="t-str">r&quot;&quot;&quot;Raw</span>'));
  assert.equal(mid, '<span class="t-str">    more</span>');
  assert.equal(close, '<span class="t-str">    done&quot;&quot;&quot;</span>');
  assert.ok(!code.includes('class="t-str"'));
  assert.ok(code.includes('<span class="t-num">1</span>'));
  const [fOpen, fMid, fClose] = highlight(["    f'''Fmt", "    x", "    end'''"], "python");
  assert.ok(fOpen.includes('<span class="t-str">f&#39;&#39;&#39;Fmt</span>'));
  assert.equal(fMid, '<span class="t-str">    x</span>');
  assert.equal(fClose, '<span class="t-str">    end&#39;&#39;&#39;</span>');
});

test("another language shows escaped plain text", () => {
  assert.deepEqual(highlight(["a < b"], "typescript"), ["a &lt; b"]);
  assert.deepEqual(highlight(["a < b"], undefined), ["a &lt; b"]);
});

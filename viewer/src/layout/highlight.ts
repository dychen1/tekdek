/* Syntax colors. This module owns the token colors of the code viewer: Rust, Python, and TypeScript (also for
   JavaScript) by line, with strings and comments that carry across lines. Every other language shows escaped plain
   text. Output is one HTML string per line. */
import { esc } from "./util.ts";

const span = (cls: string, s: string): string => `<span class="${cls}">${esc(s)}</span>`;

/** Returns the colored HTML of each line. */
export function highlight(lines: readonly string[], language: string | undefined): string[] {
  if (language === "rust") return rustLines(lines);
  if (language === "python") return pythonLines(lines);
  if (language === "typescript") return typescriptLines(lines);
  return lines.map(line => esc(line));
}

// ============ Rust ============
// A string that stays open at a line end carries into the next line.

const RUST_KW = new Set("fn let mut pub use mod impl struct enum match if else return for in while loop as ref move self Self crate super where const static type trait dyn true false unsafe async await break continue".split(" "));

/** Returns the colored HTML of Rust lines, with strings that carry across lines. */
function rustLines(lines: readonly string[]): string[] {
  let inStr = false;
  return lines.map(src => {
    let head = "", rest = src;
    if (inStr) {
      const m = /^(?:[^"\\]|\\.)*"/.exec(src);
      if (!m) return span("t-str", src);
      head = span("t-str", m[0]); rest = src.slice(m[0].length); inStr = false;
    }
    const r = rustLine(rest);
    inStr = r.open;
    return head + r.html;
  });
}

/** Returns the colored HTML of one Rust line and whether a string stays open at its end. */
function rustLine(src: string): { html: string; open: boolean } {
  const re = /(\/\/.*$)|(b?"(?:[^"\\]|\\.)*")|(b?"(?:[^"\\]|\\.)*\\?$)|(b?'(?:[^'\\]|\\.)')|('[A-Za-z_]\w*)|(#!?\[[^\]]*\])|([A-Za-z_][A-Za-z0-9_]*)(!?)|(\d+)|(\s+)|([^\sA-Za-z0-9_"'#/]+)|([\s\S])/g;
  let out = "", m: RegExpExecArray | null, prev = "", open = false;
  while ((m = re.exec(src))) {
    if (m[1]) { out += span("t-com", m[1]); break; }
    if (m[2] || m[4]) { out += span("t-str", m[2] || m[4]); prev = ""; continue; }
    if (m[3]) { out += span("t-str", m[3]); open = true; break; }
    if (m[5]) { out += span("t-kw", m[5]); prev = ""; continue; }
    if (m[6]) { out += span("t-attr", m[6]); prev = ""; continue; }
    if (m[7]) {
      const w = m[7], bang = m[8], rest = src.slice(re.lastIndex);
      let cls = "";
      if (bang) cls = "t-fn";
      else if (RUST_KW.has(w)) cls = "t-kw";
      else if (prev === "fn") cls = "t-fn";
      else if (/^[A-Z][A-Z0-9_]+$/.test(w)) cls = "t-const";
      else if (/^[A-Z]/.test(w)) cls = "t-type";
      else if (/^\s*\(/.test(rest)) cls = "t-fn";
      out += cls ? span(cls, w + bang) : esc(w + bang);
      prev = w; continue;
    }
    if (m[9]) { out += span("t-num", m[9]); prev = ""; continue; }
    if (m[10]) { out += m[10]; continue; }
    out += span("t-p", m[11] || m[12]); prev = "";
  }
  return { html: out, open };
}

// ============ Python ============
// A triple-quoted string that stays open at a line end carries into the next line.

const PY_KW = new Set("def class return if elif else for while in not and or is None True False import from as with try except finally raise yield lambda pass break continue global nonlocal async await self".split(" "));

/** Returns the colored HTML of Python lines, with triple-quoted strings that carry across lines. */
function pythonLines(lines: readonly string[]): string[] {
  let open: string | null = null;
  return lines.map(src => {
    let head = "", rest = src;
    if (open) {
      const end = rest.indexOf(open);
      if (end < 0) return span("t-str", src);
      head = span("t-str", rest.slice(0, end + 3)); rest = rest.slice(end + 3); open = null;
    }
    const line = pythonLine(rest);
    open = line.open;
    return head + line.html;
  });
}

/** Returns the colored HTML of one Python line and the triple quote that stays open at its end. */
function pythonLine(src: string): { html: string; open: string | null } {
  const re = /(#.*$)|([rbfu]{0,2}(?:"""|'''))|([rbfu]{0,2}"(?:[^"\\]|\\.)*"|[rbfu]{0,2}'(?:[^'\\]|\\.)*')|(@[A-Za-z_][\w.]*)|([A-Za-z_]\w*)|(\d+(?:\.\d+)?)|(\s+)|([\s\S])/g;
  let out = "", m: RegExpExecArray | null, prev = "";
  while ((m = re.exec(src))) {
    if (m[1]) { out += span("t-com", m[1]); break; }
    if (m[2]) {
      const q = m[2].slice(-3), end = src.indexOf(q, re.lastIndex);
      if (end < 0) { out += span("t-str", src.slice(m.index)); return { html: out, open: q }; }
      out += span("t-str", src.slice(m.index, end + 3)); re.lastIndex = end + 3; prev = ""; continue;
    }
    if (m[3]) { out += span("t-str", m[3]); prev = ""; continue; }
    if (m[4]) { out += span("t-attr", m[4]); prev = ""; continue; }
    if (m[5]) {
      const w = m[5], rest = src.slice(re.lastIndex);
      const cls = PY_KW.has(w) ? "t-kw" : prev === "def" || prev === "class" ? (prev === "class" ? "t-type" : "t-fn")
        : /^[A-Z]/.test(w) ? "t-type" : /^\s*\(/.test(rest) ? "t-fn" : "";
      out += cls ? span(cls, w) : esc(w);
      prev = w; continue;
    }
    if (m[6]) { out += span("t-num", m[6]); prev = ""; continue; }
    if (m[7]) { out += m[7]; continue; }
    out += span("t-p", m[8]); prev = "";
  }
  return { html: out, open: null };
}

// ============ TypeScript ============
// A block comment or a template string that stays open at a line end carries into the next line.

const TS_KW = new Set(("abstract any as async await boolean break case catch class const constructor continue declare default " +
  "delete do else enum export extends false finally for from function get if implements import in instanceof interface keyof " +
  "let namespace never new null number of private protected public readonly return satisfies set static string super switch " +
  "this throw true try type typeof undefined unknown var void while yield").split(" "));
/** The words after which a name declares a type. */
const TS_TYPE_DECL = new Set(["class", "interface", "enum", "type", "extends", "implements", "new"]);

type Closer = "*/" | "`";

/** Returns the colored HTML of TypeScript lines, with block comments and template strings that carry across lines. */
function typescriptLines(lines: readonly string[]): string[] {
  let open: Closer | null = null;
  return lines.map(src => {
    let head = "", rest = src;
    if (open) {
      const cls = open === "*/" ? "t-com" : "t-str", end = closeAt(rest, open);
      if (end < 0) return span(cls, src);
      head = span(cls, rest.slice(0, end)); rest = rest.slice(end); open = null;
    }
    const line = typescriptLine(rest);
    open = line.open;
    return head + line.html;
  });
}

/** Returns the index right after the closer of an open block comment or template string, or -1. */
function closeAt(src: string, closer: Closer): number {
  if (closer === "*/") {
    const end = src.indexOf("*/");
    return end < 0 ? -1 : end + 2;
  }
  const m = /^(?:[^`\\]|\\.)*`/.exec(src);
  return m ? m[0].length : -1;
}

/** Returns the colored HTML of one TypeScript line and the closer that stays open at its end. */
function typescriptLine(src: string): { html: string; open: Closer | null } {
  const re = /(\/\/.*$)|(\/\*)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')|(`)|(@[A-Za-z_$][\w$.]*)|(#?[A-Za-z_$][\w$]*)|(\d+(?:\.\d+)?)|(\s+)|([\s\S])/g;
  let out = "", m: RegExpExecArray | null, prev = "";
  while ((m = re.exec(src))) {
    if (m[1]) { out += span("t-com", m[1]); break; }
    if (m[2] || m[4]) {
      const closer: Closer = m[2] ? "*/" : "`", cls = m[2] ? "t-com" : "t-str", end = closeAt(src.slice(re.lastIndex), closer);
      if (end < 0) { out += span(cls, src.slice(m.index)); return { html: out, open: closer }; }
      out += span(cls, src.slice(m.index, re.lastIndex + end)); re.lastIndex += end; prev = ""; continue;
    }
    if (m[3]) { out += span("t-str", m[3]); prev = ""; continue; }
    if (m[5]) { out += span("t-attr", m[5]); prev = ""; continue; }
    if (m[6]) {
      const w = m[6], rest = src.slice(re.lastIndex);
      const cls = TS_KW.has(w) && !(prev === "function" || TS_TYPE_DECL.has(prev)) ? "t-kw"
        : prev === "function" ? "t-fn" : TS_TYPE_DECL.has(prev) ? "t-type"
        : /^[A-Z][A-Z0-9_]+$/.test(w) ? "t-const" : /^[A-Z]/.test(w) ? "t-type" : /^\s*(?:<[^>]*>\s*)?\(/.test(rest) ? "t-fn" : "";
      out += cls ? span(cls, w) : esc(w);
      prev = w; continue;
    }
    if (m[7]) { out += span("t-num", m[7]); prev = ""; continue; }
    if (m[8]) { out += m[8]; continue; }
    out += span("t-p", m[9]); prev = "";
  }
  return { html: out, open: null };
}

/* Viewer page. This module owns page boot, the codebase and flow pickers, the loading and error states,
   the source load, and the URL state (`?artifact=&flow=&step=`). It creates one deck for the selected
   codebase and flow, and replaces it on a change.
   # Sections
   status, loading, pickers, URL state, boot */
import type { ArtifactEntry, ArtifactIndex, DeckFlow, DeckGraph, FlowEntry, SourceView } from "./data/models.ts";
import { deckFlow, deckGraph, pickFlow, frozenFlows } from "./data/adapt.ts";
import { loadFlow, loadGraph, loadIndex, loadSource, LoadError, type LoadReason } from "./data/load.ts";
import { createDeck, type DeckInstance } from "./deck/deck.ts";
import { codebasePicker, esc, flowPicker, icon } from "./deck/markup.ts";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

// ============ status ============

/** Shows one loading line with a cursor at the top of the code column, or hides it. The code viewer hides while it shows. */
function showLoading(line: string | null) {
  const status = $("status");
  status.hidden = line === null;
  if (line === null) { status.replaceChildren(); return; }
  showReader(false);
  status.innerHTML = `<div class="st-line"><span>${esc(line)}</span><i class="cursor" aria-hidden="true"></i></div>`;
}

/** Shows or hides the code header and the code viewer. They hide while no deck is loaded. */
function showReader(visible: boolean) {
  for (const id of ["codeHead", "code"]) $(id).style.display = visible ? "" : "none";
}

/** Fills the code column, the bar, and the flow band while no deck is loaded. The chrome stays visible and inert. */
function showShell(badge: string, color: string, desc: string, meta: string) {
  document.documentElement.style.setProperty("--cc", color);
  $("cdHead").innerHTML = `<span class="pbadge" style="--c:${esc(color)}"><span>${esc(badge)}</span></span>`;
  $("cdStep").innerHTML = `<p class="cd-ann">${desc}</p>` + (meta ? `<p class="cd-empty">${meta}</p>` : "");
  showReader(false);
  $("where").replaceChildren();
  $("code").replaceChildren();
  $("counter").textContent = "Step 0 / 0";
  $("chip").classList.add("none");
  $("chipText").textContent = "No flow";
  $("cMeta").textContent = "";
  $("cDesc").textContent = "";
  $("ticks").replaceChildren();
  $("flowPick").outerHTML = '<span id="flowPick"></span>';
  $("fhMeta").replaceChildren();
  setChrome(false);
}

/** Enables or disables every control that needs a deck. */
function setChrome(enabled: boolean) {
  for (const id of ["prevBtn", "nextBtn", "clearBtn", "compactBtn", "zoomOut", "zoomIn", "keyBtn", "fullBtn"]) {
    $<HTMLButtonElement>(id).disabled = !enabled;
  }
}

/** The recovery step for each cause of a load failure. */
const RECOVERY: Record<LoadReason, string> = {
  fetch: "Start the viewer server with <code>tekdek serve</code>, then reload this page.",
  digest: "Reindex the codebase with <code>tekdek index</code>, then reload this page.",
  graph: "Reindex the codebase with <code>tekdek index</code> to write a current graph.",
  flow: "Freeze the flow again with <code>tekdek flow import</code>, then reload this page.",
  empty: "Index a codebase with <code>tekdek index</code>, then reload this page.",
};

/** Shows a load failure in the code column, with one recovery step.
 *  A failure of a load that a newer load replaced is dropped: the newer load owns the page. */
function fail(error: unknown, mine = ticket) {
  if (mine !== ticket) return;
  deck?.destroy(); deck = null;
  showLoading(null);
  const reason = error instanceof LoadError ? error.reason : "fetch";
  const message = error instanceof Error ? error.message : String(error);
  showShell("error", "var(--danger)", esc(message), RECOVERY[reason]);
}

// ============ loading ============

let index: ArtifactIndex | null = null;
let entry: ArtifactEntry | null = null;
let graph: DeckGraph | null = null;
let flowEntry: FlowEntry | null = null;
let deck: DeckInstance | null = null;
let ticket = 0;

/** Loads one codebase, verifies its graph, and opens the requested flow. A failure shows unless a newer load started. */
async function openArtifact(file: string, flowName: string | null, step: number) {
  const mine = ++ticket;
  try {
    const next = index!.artifacts.find(item => item.file === file) ?? null;
    if (!next) throw new LoadError("fetch", `The artifact index has no codebase named ${file}.`);
    deck?.destroy(); deck = null;
    entry = next; graph = null;
    renderRepoBadge();
    const name = entry.label ?? entry.file;
    showShell("loading", "var(--accent)", `Reading and checking ${esc(name)}.`, "");
    showLoading(`loading ${name}`);
    const document = await loadGraph(entry);
    if (mine !== ticket) return;
    graph = deckGraph(document, entry.label ?? entry.file.replace(/\.json$/, ""));
    await openFlow(pickFlow(frozenFlows(entry.flows), flowName), step);
  } catch (error) {
    fail(error, mine);
  }
}

/** Loads one flow of the current codebase and the source of its step functions, then replaces the deck.
 *  A failed source load still plays the flow: every code viewer shows the unreadable note. */
async function openFlow(next: FlowEntry | null, step: number) {
  const mine = ++ticket;
  try {
    let flow: DeckFlow | null = null;
    let sources: ReadonlyMap<string, SourceView> = new Map();
    if (next) {
      showLoading(`loading flow ${next.name}`);
      const artifact = await loadFlow(next);
      if (mine !== ticket) return;
      sources = await loadSource(entry!.file, next.name).catch(() => new Map<string, SourceView>());
      if (mine !== ticket) return;
      try { flow = deckFlow(artifact, graph!, entry!.digest, next); }
      catch (error) {
        throw new LoadError("flow", `The flow file of ${next.name} is not a flow that this viewer reads. ${error instanceof Error ? error.message : ""}`.trim());
      }
    }
    flowEntry = next;
    deck?.destroy();
    showLoading(null);
    setChrome(true);
    showReader(true);
    deck = createDeck({
      graph: graph!, flow, sources, flowCount: frozenFlows(entry!.flows).length,
      onFlowPick: anchor => openPicker(anchor, "flow"),
      onStep: n => writeUrl(n),
    });
    (window as unknown as { __deck: unknown }).__deck = deck.inspect;
    writeUrl(0);
    if (step > 0) deck.go(step);
  } catch (error) {
    fail(error, mine);
  }
}

// ============ pickers ============

const pickPop = $("pickPop");
let pickAnchor: HTMLElement | null = null;

/** Opens the codebase or flow list under its badge. */
function openPicker(anchor: HTMLElement, kind: "flow" | "codebase") {
  if (pickAnchor === anchor) { closePicker(true); return; }
  closePicker(false);
  pickAnchor = anchor;
  pickPop.innerHTML = kind === "flow"
    ? flowPicker(frozenFlows(entry?.flows), flowEntry?.name ?? null)
    : codebasePicker(index?.artifacts ?? [], entry?.file ?? null, item => frozenFlows(item.flows).length);
  pickPop.setAttribute("aria-label", kind === "flow" ? "Choose a flow" : "Choose a codebase");
  anchor.setAttribute("aria-expanded", "true");
  const a = anchor.getBoundingClientRect();
  pickPop.classList.add("show");
  const w = pickPop.offsetWidth, h = pickPop.offsetHeight;
  const x = Math.max(8, Math.min(a.left, innerWidth - w - 8));
  const y = a.bottom + 6 + h > innerHeight - 8 ? Math.max(8, a.top - 6 - h) : a.bottom + 6;
  pickPop.style.transform = `translate(${Math.round(x)}px,${Math.round(y)}px)`;
  (pickPop.querySelector<HTMLElement>('[aria-current="true"]') ?? pickPop.querySelector<HTMLElement>(".pk-item"))?.focus();
}

function closePicker(returnFocus: boolean) {
  if (!pickAnchor) return;
  pickPop.classList.remove("show");
  pickAnchor.setAttribute("aria-expanded", "false");
  if (returnFocus && pickAnchor.isConnected) pickAnchor.focus();
  pickAnchor = null;
}

pickPop.addEventListener("click", e => {
  const item = (e.target as Element).closest<HTMLElement>(".pk-item");
  if (!item) return;
  closePicker(false);
  if (item.dataset.flow !== undefined) {
    const next = frozenFlows(entry?.flows).find(flow => flow.name === item.dataset.flow) ?? null;
    if (next && next.name !== flowEntry?.name) void openFlow(next, 0);
  } else if (item.dataset.artifact && item.dataset.artifact !== entry?.file) {
    void openArtifact(item.dataset.artifact, null, 0);
  }
});
// No key in the picker reaches the deck keys, so the deck never changes under an open list.
pickPop.addEventListener("keydown", e => {
  e.stopPropagation();
  const items = [...pickPop.querySelectorAll<HTMLElement>(".pk-item")];
  const i = items.indexOf(document.activeElement as HTMLElement);
  const j = e.key === "ArrowDown" ? i + 1 : e.key === "ArrowUp" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : null;
  if (j === null) return;
  e.preventDefault();
  items[Math.max(0, Math.min(items.length - 1, j))]?.focus();
});
document.addEventListener("pointerdown", e => {
  if (pickAnchor && !(e.target as Element).closest("#pickPop") && !pickAnchor.contains(e.target as Node)) closePicker(false);
});
// Escape closes an open picker before the deck sees it: the picker first, then the key, then the flow.
addEventListener("keydown", e => {
  if (pickAnchor && e.key === "Escape") { e.preventDefault(); e.stopImmediatePropagation(); closePicker(true); }
}, { capture: true });

/** The codebase badge: a picker when the index holds more than one codebase. */
function renderRepoBadge() {
  const head = $("sbHead");
  const name = esc(entry?.label ?? entry?.file ?? "codebase");
  if ((index?.artifacts.length ?? 0) > 1) {
    head.innerHTML = `<button class="sb-title" id="repoPick" type="button" aria-haspopup="dialog" aria-expanded="false" aria-label="Codebase ${name}. Choose another codebase">${name}${icon("chev")}</button>`;
    const button = $("repoPick");
    button.addEventListener("click", () => openPicker(button, "codebase"));
  } else head.innerHTML = `<span class="sb-title" id="repoPick">${name}</span>`;
}

// ============ URL state ============

function readUrl() {
  const params = new URLSearchParams(location.search);
  const step = Number(params.get("step"));
  return { artifact: params.get("artifact"), flow: params.get("flow"), step: Number.isSafeInteger(step) && step > 0 ? step : 0 };
}

function writeUrl(step: number) {
  const params = new URLSearchParams();
  if (entry) params.set("artifact", entry.file);
  if (flowEntry) params.set("flow", flowEntry.name);
  if (step > 0) params.set("step", String(step));
  history.replaceState(null, "", `${location.pathname}?${params}`);
}

// ============ boot ============

async function boot() {
  showShell("loading", "var(--accent)", "Reading the artifact index.", "");
  showLoading("loading the artifact index");
  index = await loadIndex();
  const want = readUrl();
  const file = index.artifacts.some(item => item.file === want.artifact) ? want.artifact!
    : index.artifacts.some(item => item.file === index!.defaultArtifact) ? index.defaultArtifact! : index.artifacts[0].file;
  await openArtifact(file, want.flow, want.step);
}

boot().catch(fail);

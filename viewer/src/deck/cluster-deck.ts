/* Deck cluster deck. This module owns the pinned 3D deck of the current cluster: a title band, a front face,
   a cast shadow, and one lifted box for each member in first-visit order. A cluster change replaces the deck in one frame.
   A box goes to its first step. */
import { boxWidth, deckHeight, deckTransform, DECK_Z, home } from "../layout/deck-geometry.ts";
import { f1 } from "../layout/util.ts";
import { esc, PTR } from "./markup.ts";
import type { ClusterView } from "../layout/model.ts";
import { $, type ClusterDeck, type DeckContext } from "./context.ts";

/** One lifted box and the elements that the deck reads. */
interface BoxView { key: string; name: string; file: string; steps: readonly number[]; el: HTMLElement; lid: HTMLElement; btn: HTMLButtonElement; badge: HTMLElement }

/** The deck of one cluster. */
interface DeckView { cam: HTMLElement; slab: HTMLElement; top: HTMLElement; boxes: BoxView[]; byKey: Map<string, BoxView> }

/** Puts a box at a deck-plane place and a height. */
const setBox = (el: HTMLElement, x: number, y: number, z: number) => { el.style.transform = `translate3d(${f1(x)}px,${f1(y)}px,${f1(z)}px)`; };

export function createClusterDeck(ctx: DeckContext): ClusterDeck {
  const { model, state } = ctx;
  const { CL, FN } = model;
  const cv = $("#cv");
  let deck: DeckView | null = null;

  /** Builds the lifted box of one member. */
  function makeBox(key: string, cluster: ClusterView): BoxView {
    const fn = FN.get(key)!, steps = model.plan.visits.get(key) ?? [];
    const el = document.createElement("div");
    el.className = "mbox"; el.dataset.key = key; el.style.setProperty("--c", cluster.cv);
    el.innerHTML = `<div class="face fshadow"></div><div class="riser"><div class="face front"></div><div class="face lid">${PTR}` +
      `<button class="tmain" type="button" tabindex="-1"${steps.length ? ` data-step="${steps[0]}"` : ""}><span class="lp">${esc(fn.file)}</span><span class="ln">${esc(fn.name)}</span></button>` +
      `<span class="sn">${steps.join(" ")}</span><span class="stepno">${steps[0] ?? ""}</span></div></div>`;
    return { key, name: fn.name, file: fn.file, steps, el, lid: el.querySelector<HTMLElement>(".lid")!, btn: el.querySelector<HTMLButtonElement>(".tmain")!, badge: el.querySelector<HTMLElement>(".stepno")! };
  }

  /** Builds the deck of a cluster, flat on the base plane. `pin` puts it; `rise` lifts it. */
  function createDeck(id: string): DeckView | null {
    const cluster = CL.get(id);
    if (!cluster) return null;
    const keys = model.members.get(id) ?? [];
    const cam = document.createElement("div");
    cam.className = "deckcam noanim"; cam.style.setProperty("--c", cluster.cv);
    cam.innerHTML = `<div class="slab" style="width:${model.deckSize.width}px;height:${deckHeight(keys.length)}px"><div class="shadow"></div><div class="deck"><div class="dfront"></div>` +
      `<div class="dtop"><div class="dlabel"><span class="dname">flow functions in this cluster</span></div></div></div></div>`;
    const boxes = keys.map((key, i) => {
      const box = makeBox(key, cluster);
      cam.appendChild(box.el);
      setBox(box.el, ...home(i), 0);
      return box;
    });
    cv.appendChild(cam);
    return { cam, slab: cam.querySelector<HTMLElement>(".slab")!, top: cam.querySelector<HTMLElement>(".dtop")!, boxes, byKey: new Map(boxes.map(box => [box.key, box])) };
  }

  /** Shows a placed deck already raised, its boxes at their final place, in one frame. */
  function rise(d: DeckView) {
    d.slab.classList.add("up");
    d.boxes.forEach((box, i) => { box.el.classList.add("up"); setBox(box.el, ...home(i), DECK_Z + 1); });
    void d.cam.offsetWidth;
    d.cam.classList.remove("noanim");
  }

  /** Removes the current deck at once. */
  function sinkDeck() {
    deck?.cam.remove();
    deck = null;
  }

  function show(cluster: string | null) {
    sinkDeck();
    if (!cluster) return;
    deck = createDeck(cluster);
    if (deck) rise(deck);
  }

  /** Marks the current box and shows a file path on the boxes that live in other files. */
  function mark() {
    const step = model.stepAt(state.cur);
    if (!deck || !step) return;
    const file = model.fnAt(state.cur)?.file ?? null;
    for (const box of deck.boxes) {
      const focus = box.key === step.node, away = box.file !== file;
      box.el.classList.toggle("focus", focus);
      box.el.classList.toggle("away", away);
      if (focus) box.badge.textContent = String(state.cur);
      box.el.style.setProperty("--lw", boxWidth(box.name, box.file, box.steps, away) + "px");
      box.btn.tabIndex = 0;
      box.btn.setAttribute("aria-label", `${box.name}${away ? " in " + box.file : ""}, step ${box.steps.join(", ")}${focus ? ", the current step" : ""}`);
    }
  }

  return {
    show, mark,
    pin(spot, anchor, k) { if (deck) deck.cam.style.transform = deckTransform(spot, anchor, k); },
    currentLid() {
      const step = model.stepAt(state.cur);
      return deck && step ? deck.byKey.get(step.node)?.lid ?? null : null;
    },
    deckRight: () => deck?.top.getBoundingClientRect().right ?? 0,
    destroy: sinkDeck,
  };
}

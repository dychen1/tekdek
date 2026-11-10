# Lift Deck

## Overview

- Purpose: describe the viewer that plays a frozen flow or a PR diff in the browser.
- Scope: the parts of the page, the keys, the PR diff view, the URL, and what `tekdek serve` reads.
- Boundaries: the file tree is the anchor. Nothing on the canvas moves between steps. Lines show static calls, not runtime counts.
- Outcome: a reader follows one behavior step by step and still knows where each function lives.

## Parts

The viewer has five parts.

- The explorer at left shows the files of the codebase. Directories start closed, and the flow start opens the path to each flow file.
- The current file's functions branch out of the explorer into a file box.
- The current cluster's functions sit on a pinned deck, and the current step's function is tethered to its slot in the file box.
- The code column at right shows the step title and its ledger: one labeled row for each used slot (takes, requires, does, on fail, returns, writes). Below the ledger are the transition from the step before, the callers and callees, and the whole function source. The header of the source has the line wrap switch and the full screen button.
- The bottom bar shows the step counter, the current cluster, the data path of the flow (the input and output models of the clusters, in order), the rules of the current cluster, and a strip of every step.

The band above the map has the map tools (compact, zoom from 50% to 150%, and the key), the flow picker, and the state of the flow: `fresh` or `stale`, and its generation.

## Keys

| Key | Action |
| --- | --- |
| K or Right | Next step |
| J or Left | Previous step |
| L | First step of the next section, where the cluster changes |
| H | First step of the previous section |
| F | Show the code full screen, or return |
| C | Fold or name the functions that are not in the flow |
| Esc | Close the key, then leave full screen, then clear the flow |
| ? | Show or hide the key |

The key popover also has the theme switch (GitHub Dark by default, light, and night) and a switch that turns off the one-character keys. The arrows and Esc stay on. The browser keeps the theme, the key switch, and the line wrap choice.

## PR Diff View

`tekdek serve --diff <name>` opens on an overview: the clusters of the Before flow and the After flow side by side, with the change counts and line counts of each cluster.

| Key or pointer | Action on the overview |
| --- | --- |
| J and K | Choose a cluster |
| Hover or focus | Show the note of a cluster |
| Space, or the chevron | Open the cluster to show its steps on both sides |
| Enter, or a click on a cluster or a step | Enter the deck |
| Esc | Close the open cluster |

The deck plays the After flow. The code column shows the diff of the current step in place of its source: the kind of the step, the path and line, the added and removed line counts, and the changed lines. A removed step has no After step: the deck stays at the entry step of its cluster, and the code column shows the removed step and its diff. In the deck, Esc returns to the overview.

See [PR Diffs](diffs.md) for how a diff is made.

## URL and Server

The URL keeps the graph, the flow or the diff, and the step: `?artifact=<file>&flow=<name>&step=<n>`, or `?artifact=<file>&diff=<name>`.

- `tekdek serve` binds to the loopback address, serves the viewer from the binary, and reads only the store files that its catalog lists.
- It also reads the source files of a flow's step functions, inside the project folder that holds the store. A function whose source changed after the last index shows a note in place of its source.
- Before it serves a `.tekdek` store, `tekdek serve` reindexes the project graph if the source changed after the last index, the same as `tekdek index` with no options. `--no-index` skips that step.
- `tekdek serve` sends each graph with the semantic edges in its cache, and starts no language server.

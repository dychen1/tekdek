---
name: flow-view
description: Open a frozen tekdek flow in the Lift Deck in the browser. Use when the user asks to view, open, play, or show a flow.
argument-hint: "[flow name]"
---

# View a Flow

Open one frozen flow in the Lift Deck. This skill changes no file.

1. Pick the flow from `tekdek flow list` (add `--store <directory>` when the user names a store): the flow that `$ARGUMENTS` names, the only flow, or ask. When the store has no flow, offer `/tekdek:flow`.
2. Run `tekdek serve --flow <name> --port 0 --open` as a background process, so that it keeps running after this turn. The first output line is the URL of the flow.
3. Give the user the URL, the keys (Right and Left step, F fits the map, ? shows the key, Esc clears the flow), and how to stop the server.

A `stale` flow was frozen against an older graph. It still plays, with a warning.

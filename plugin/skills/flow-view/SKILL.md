---
name: flow-view
description: Open a frozen tekdek flow in the Lift Deck in the browser. Use when the user asks to view, open, play, or show a flow.
argument-hint: "[flow name]"
---

# View a Flow

Open one frozen flow in the Lift Deck. This skill starts the local viewer server and gives the user the URL. It changes no file.

1. Find the flow. Run `tekdek flow list` in the analyzed project, or add `--store <directory>` when the user names a store. Use the flow that `$ARGUMENTS` names. Otherwise, use the only flow, or ask which flow to open. If the store has no flow, offer `/tekdek:flow` instead.
2. Start the server as a background process, so that it keeps running after this turn:

   ```sh
   tekdek serve --flow <name> --port 0 --open
   ```

   The first output line is the URL of the flow. Port 0 selects a free port. `--open` opens the default browser.
3. Give the user the URL and the keys: Right and Left step through the flow, F fits the map, ? shows the key, and Esc clears the flow.
4. Tell the user that the server runs until they stop it, and how to stop it.

If the list marks the flow `stale`, the graph changed after the freeze. The flow still plays, with a stale warning. Offer to index the project again and write a new flow.

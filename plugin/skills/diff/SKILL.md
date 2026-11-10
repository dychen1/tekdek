---
name: diff
description: Freeze what a pull request changed in one flow as a tekdek diff, and open it in the Lift Deck. Use only when the user asks for a PR diff, a diff of a flow, or what a branch changed in a behavior.
argument-hint: "[--base <commit or branch>] <behavior or flow name>"
allowed-tools: Bash(tekdek *)
---

# PR Diffs

Freeze what one pull request changed in one behavior: a Before flow on the base commit, an After flow on the head commit, and the step pairs between them. Code derives the change kind and the line counts of each step from git. The Lift Deck shows the two flows side by side. Agents without the plugin can read this file at `plugin/skills/diff/SKILL.md` in the tekdek repository.

## Inputs

- A behavior or an existing flow name, and optionally a base. The head is the current checkout: the user checks out the PR branch first. The default base is the merge-base of `HEAD` and the default branch.
- A diff name is a lowercase slug, such as `pr-42-request-auth`. Ask one short question only when the behavior or the base is ambiguous.

## Setup

- This skill needs tekdek 0.3.0 or later. Run `tekdek version`. If the command is not found, follow [install.md](../flow/install.md): run the plugin launcher by its full path, or give the user the install command and stop. If the version is older, stop and tell the user to run the installer again to upgrade.
- Follow [the flow skill](../flow/SKILL.md) for the store, the language servers, the flow format, and the flow rules. This skill adds only the diff steps.
- Commit the head first: import reads the head from `HEAD`, and it rejects a function whose file has uncommitted changes.

## Commands

```sh
tekdek diff prepare [--base <commit or branch>]         # check out and index the base, list the flows that fit it
tekdek diff agent-contract                               # JSON Schema of the diff document
tekdek diff import <name> --base <commit> --before <flow> --after <flow> --input <path> --check --json
tekdek diff import <name> --base <commit> --before <flow> --after <flow> --input <path> --author-name <agent> --json
tekdek diff verify <name>
tekdek diff show <name>
tekdek serve --diff <name> --port 0 --open
```

`prepare` prints the base store. Pass `--store <base store>` to `graph search`, `graph show`, `flow import`, and `flow show` to read or write the base.

## Make the Diff

1. Run `tekdek index`, then `tekdek diff prepare`. Keep the base commit and the base store from its output.
2. **Before flow.** When `prepare` lists a flow that fits the base and walks the behavior, use it. Otherwise write the Before flow on the base graph and import it in the base store: `tekdek flow import <flow> --store <base store> --graph <graph file> ...`.
3. **After flow.** Write the After flow on the head graph and import it in the project store under a new name, such as `<flow>-pr-42`. Walk the same behavior. Keep the titles and the cluster keys of the steps that did not change, so that the two sides read alike.
4. **Pair the steps.** Read both flows with `flow show` (add `--store <base store>` for the Before flow). Pair each Before step with the After step that does the same job, by step number. A step in no pair is new or removed. A renamed or moved function can still pair. Use `graph show` on both stores when the match is not clear.
5. **Write the notes.** For each cluster that holds a change, write one note on the side that shows it: one sentence that starts with `Now:` and says what the code does differently. Never name a cluster rename, merge, or move: clusters only split a long call stack.
6. **Freeze.** Write the document outside the repository:

   ```json
   {
     "contractVersion": 1,
     "pairs": [{ "before": 1, "after": 1 }, { "before": 2, "after": 3 }],
     "notes": [{ "side": "after", "clusterKey": "route-request", "text": "Now: ..." }]
   }
   ```

   Run the import with `--check` first. It lists every issue and writes nothing. Fix the issues, then import. The freeze removes the base worktree.

## Verify and Report

Run `tekdek diff verify <name>`. It must report `valid`. `unverifiable` means that git no longer has a commit of the diff.

Report the diff name, the base and head commits, the step counts of both sides, the counts per kind (new, removed, changed, moved, unchanged), and the added and removed lines. Open the diff with `tekdek serve --diff <name> --port 0 --open` as a background process. The first output line is the URL of the diff.

Always end the report with the browser URL as a link. Do not finish without the URL. If the server does not start, say so and give the `tekdek serve` command.

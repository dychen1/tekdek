# PR Diffs

## Overview

- Purpose: show what a pull request changed in one behavior.
- Scope: the base worktree, the Before and After flows, the pairs and notes, the freeze, verification, and the view.
- Boundaries: a flow import never makes a diff. The `diff` skill makes one only when a user asks. Clusters are not compared.
- Outcome: one frozen file that holds both flows and one row for each step, derived from git.

## Make a Diff

Check out the PR branch and commit the head first. Then ask the agent, for example `/tekdek:diff request-auth`. The [diff skill](../plugin/skills/diff/SKILL.md) runs these steps:

- `diff prepare` checks out the base commit in a worktree in the git folder and indexes it. The default base is the merge-base of `HEAD` and the default branch. It lists the flows whose every step function is unchanged on the base.
- The agent uses a flow that fits the base, or writes a Before flow in the base store (`--store <base store>`). It writes an After flow on the head graph. Then it writes one document: the pairs of Before and After steps by step number, and one note per changed cluster. `diff agent-contract` prints its JSON Schema.
- `diff import` checks the document and freezes `diffs/<name>.json`. The diff holds both flows, both commits, the pairs, the notes, and one row for each step. Code derives each row from git: the kind (`new`, `removed`, `changed`, `moved` to another file, or `unchanged`), the line span on each commit, and the line diff of the function. The counts come from that line diff. Clusters are not compared. The freeze removes the base worktree.

The After flow must be fresh, and `diff import` rejects a function whose file has uncommitted changes.

## Verify

`diff verify` reads the two commits from git and derives every row again. It reports `unverifiable` when git no longer has a commit.

A diff holds both of its flows, so it follows the schema rule of a flow: a build cannot read a diff whose flows have another schema version. Make the diff again with the current build. See [Frozen Flows](flows.md#stale-flows-and-schema-versions).

## View

`tekdek serve --diff <name>` opens the diff in the [Lift Deck](lift-deck.md#pr-diff-view): the Before and After flows side by side, then the deck on the After flow with the diff of each step.

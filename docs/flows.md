# Frozen Flows

## Overview

- Purpose: describe what a flow is, who writes each part, and how tekdek freezes and verifies it.
- Scope: the agent output, the derived layers, the import, revisions, verification, and schema versions.
- Boundaries: the agent writes meaning only. Code derives every ID, membership, color, transition, and digest.
- Outcome: the same agent output on the same graph always freezes the same bytes, except the provenance.

## What the Agent Writes

A flow is an ordered walk through the functions behind one behavior. The agent writes one output document. `flow agent-contract` prints its JSON Schema, and the [flow skill](../plugin/skills/flow/SKILL.md) tells an agent how to write it.

- The behavior and its boundaries, in plain text.
- 1 to 8 clusters. A cluster is a phase of the flow. Each cluster has a key, a label, the data models that enter it (`input`), the data models that leave it (`output`), and 1 to 3 rules that state what the cluster makes true.
- 1 to 400 steps. Each step names one function by its node key, one cluster, a title, and a ledger.
- A ledger has six slots: `takes`, `requires`, `does`, `onFail`, `returns`, and `writes`. A step uses at least one slot, and a used slot holds 1 to 6 short items. The ledgers tell one story about the data: what a step takes from the step before, and what it hands on.
- An identifier in a rule or a ledger item is a mark, a backtick span `kind:name`. The kinds are `fn`, `type`, `value`, `file`, and `field`. A `fn` mark must name a function of the graph.
- All steps of one function use the same cluster.

When the graph does not contain the behavior, the agent answers `no_flow` with a reason, and the import writes nothing.

## What Code Derives

Code derives the rest: function membership, cluster colors, visit runs, static call transitions, step IDs, and digests. A transition is a static relation between two steps, not a runtime observation.

## Import and Verify

- `flow import` validates the complete output and freezes one generation. On any error, it prints every issue and writes no flow. `--check` runs the complete import and writes no flow. Both can add the edges of the step nodes to the semantic cache (see [Semantic Edges](analysis.md#semantic-edges)).
- A new name freezes an `initial` generation. An existing name freezes a `revision` with a diff against its parent, on the graph of the parent.
- `flow verify` recomputes the derived layers and digests. An edited generation fails with `digest_mismatch`.
- `flow list` shows the current generation of each flow and its freshness. `flow show <name>` prints one flow.

## Stale Flows and Schema Versions

- A flow binds to the digest of its graph. When you index changed source again, the flow becomes stale. A stale flow still plays, with a warning. To follow the new graph, write the flow again under a new name. See [Project Store](store.md).
- A flow file states its schema version, and a build reads only the version that it writes. `flow list` and `tekdek serve` skip a flow of another version with a warning that names both versions. Write the flow again with the current build.

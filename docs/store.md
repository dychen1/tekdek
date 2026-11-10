# Project Store

## Overview

- Purpose: describe the `.tekdek` folder that holds the graphs, flows, and diffs of a project.
- Scope: where the store is, what each file is, which graph is the default, and what to commit.
- Boundaries: the store holds no source code. A generation never changes after its freeze.
- Outcome: a folder that a team commits to share flows.

## Location

Every command uses the nearest `.tekdek` folder from the working directory. The search stops at the top folder of the Git work tree, and it does not go up into your home folder. Pass `--store <directory>` to use another store.

`tekdek index` writes to the nearest `.tekdek` folder at or above the root. When the project has no store, `index` creates `<root>/.tekdek` after the analysis succeeds. Commit the folder to share flows.

- In a project that has a store, `tekdek index` without a root indexes the project root from any subfolder, as Git commands do. The project root is the folder that holds the store.
- A root argument inside that project needs `--name`, so a partial graph never replaces the project graph: `tekdek index services/worker --name worker` writes `worker.json`.

## Layout

```text
acme-api/
|-- src/
`-- .tekdek/
    |-- .gitignore                      # ignores the lock files, the daemon files, interrupted writes, and index.json
    |-- acme-api.json                   # graph of the project, from tekdek index
    |-- acme-api.semantic.json          # semantic edges of the nodes that commands read
    |-- flows/
    |   |-- request-auth/
    |   |   |-- current.json            # names the current generation
    |   |   `-- generations/
    |   |       |-- 1-3f9a0c1d2e4b.json # first freeze
    |   |       `-- 2-8b21d0e7aa13.json # revision of 1, now current
    |   `-- billing-webhook/
    |       |-- current.json
    |       `-- generations/
    |           `-- 1-c0d4e5f60718.json
    `-- diffs/
        `-- pr-42-request-auth.json     # one frozen PR diff
```

## Rules

- A flow is one folder. Each freeze adds one generation file, named by its ordinal and the first 12 characters of its digest. A generation never changes after the freeze. `current.json` names the current generation.
- A diff is one file in `diffs/`. See [PR Diffs](diffs.md).
- The graph belongs to the store, not to one flow. All flows of a project share its graph.
- The catalog of graphs and current flows is not a file in the store. `tekdek serve` builds it from the store for each request, so two branches that add flows merge without a conflict. An `index.json` file from an older tekdek has no effect; delete it.
- The default graph is the only graph, else the graph named after the project folder, else the first graph in name order. A named index does not change the default. The `graph` commands and the viewer use the default when you do not name a graph.
- A flow binds to the digest of its graph, so commit the graph with its flows. When you index changed source again, the digest changes and the flows become stale. A stale flow still plays, with a warning. To follow the new graph, write the flow again under a new name.
- Commit `<graph>.semantic.json` with its graph, so `flow verify` runs without a language server, for example in CI. `graph show` adds to it, so expect changes in this file.
- A project folder named `x.semantic` needs `tekdek index --name <name>`, because `x.semantic.json` is the name of a semantic cache.

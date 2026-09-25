# Rust Development

## Documentation

- Use strict Simplified Technical English in documentation, comments, and errors.
- Use short sentences, active voice, and one term for each concept.
- Give each module a short noun-phrase title. State its ownership and useful inputs or outputs.
- Use concise `# Sections` lists and section banners in large files.
- Do not add prose below a section banner unless it explains a non-obvious rule or invariant.
- Give each function docstring one short purpose sentence. State what the function does, not how it works.
- After the purpose, document only non-obvious errors, edge cases, rules, or invariants.
- Do not repeat the code or add implementation narratives to documentation.
- Do not document obvious constructors, accessors, conversions, or defaults.
- Document non-obvious order, matching, merging, fallback, schema, and error behavior.
- Add one short purpose comment before each arm in a match with four or more logical arms.

## Code

- Order declarations from the main model or entry point to specific details.
- Keep `unsafe` code out of the project.
- Propagate typed errors with `?`. Do not use `unwrap` in production code.
- Keep tests in local `#[cfg(test)]` modules. Name tests after observable behavior.
- Use a narrow lint exception only when its `reason` explains the need.

## Checks

Run `npm ci` once. Every Cargo build runs `build.rs`, which bundles the viewer with the npm dependencies.

Run these commands after each Rust change:

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features
cargo test --locked --all-targets
```

## Lift Deck rules

- Keep the Code explorer sidebar transparent at every zoom level and viewport width. Do not add a solid panel, blur, or opaque backing layer. Keep the faint map visibility under the text (the reading mask).
- Directories, files, and lifted boxes support explicit collapse, including when a flow opens them. A toggle keeps the row on the map and preserves its screen position.
- Directories and files start closed. The flow start opens the tree path to the flow's named rows once. No row moves between flow steps; only a user fold changes the flow layout.
- Draw every connector (tethers, call arcs, the stack arrow) in screen space from the on-screen rectangles of the two nodes that it joins. Each end must touch its node.
- Keep the flow controls at a fixed size in each state. Call lines show static relationships, not runtime counts.

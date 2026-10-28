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

- Keep the Code explorer sidebar transparent at every viewport width. Do not add a solid panel, blur, or opaque backing layer.
- Directories start closed. The flow start opens the path to each flow file once. Nothing on the canvas moves or animates between steps or clusters: the file box, the deck, and the connectors appear at their final place.
- The deck is pinned: one place and one width for every step and cluster.
- Draw every connector (the guide, the tether, the arrow) in screen space from the on-screen rectangles of the two elements that it joins. Each end must touch its element. Draw at most three, and never let one cross another, a box, or a panel.
- Show each piece of information once. Line numbers appear only in the code viewer. Call lists show static relationships, not runtime counts.

# Contributing

These rules apply to human and agent changes in this repository.

## Change discipline

- Keep a behavior change separate from a mechanical cleanup.
- Name each behavior change in the commit message and pull request description.
- Do not change defaults, fallback behavior, authorization, or policy during an unnamed cleanup.
- Preserve existing behavior during a refactor unless the task requires a change.
- If security requires a new default, add a focused test and describe the change.
- Keep unrelated generated files and formatting changes out of the commit.

## Design

- Put a shared rule at the narrowest common call path.
- Extract a helper when two sites must obey the same security or persistence rule.
- Do not create a generic layer only because two data shapes look similar.
- Remove pass-through functions unless they define a boundary or a test seam.
- Use an enum or discriminated union for protocol states and closed sets.
- Do not infer protocol state from a substring.
- Keep interfaces smaller than their implementations.
- Prefer the standard library and installed dependencies.
- Delete dead code before you add an abstraction.

## Errors and trust boundaries

- Validate external input before file, process, provider, or network access.
- Fail closed for unknown authorization and policy values.
- Keep secret files private and replace state files atomically.
- Rate-limit authenticated routes that can start processes or write files.
- Return stable error codes. Keep sensitive details out of responses and logs.

## Cross-surface changes

- Trace each payload field from its producer to every consumer.
- Preserve navigation targets through web, desktop, and mobile bridges.
- Add one end-to-end assertion when a field crosses a process or platform boundary.
- Keep wire formats backward compatible. Use explicit defaults for new optional fields.

## Tests

- Add the smallest test that fails without the change.
- Test the shared rule once at its common boundary.
- Add a regression test for each corrected security or data-loss fault.
- Do not copy a large fixture when a focused assertion proves the behavior.

Run the checks for each changed area:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
npm run typecheck
npm run test:server
```

Run the platform build when a change affects the desktop or mobile application.

## Pull request review

A review must separate these questions:

1. Does the change match its issue or specification?
2. Does the change preserve unrelated behavior?
3. Does one module own each policy and persistence rule?
4. Do tests cover the changed behavior and its failure path?
5. Does the pull request state any accepted limitation or manual acceptance step?

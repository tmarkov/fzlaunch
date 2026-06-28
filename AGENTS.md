# AGENTS.md

Development guidelines for future work on `fzlaunch`.

## Core Principles

1. Prefer minimal rule surface.
   Use one broad, consistent rule instead of several narrow rules. Exceptions need
   strong justification and explicit tests. The known explicit exception is
   initial `[tilde]`, which enters raw edit mode without copying the selected
   result.

2. Let behavior drive abstraction.
   Do not add traits, source contracts, modules, fields, or value kinds until
   tests or spec examples need them.

3. Let spec examples drive implementation.
   When behavior is unclear, clarify or add examples first. Code should
   implement clarified behavior, not guesses.

4. Keep state in the state machine.
   External callers should feed events or updated buffers. `InputState` should
   own selection, mode, current-value resolution, and ranking.

5. Prefer semantic APIs over UI mechanics.
   Use APIs such as `update_input(Value)` rather than low-level gestures such as
   `type_char`, because the TUI may support paste, deletion, cursor movement, and
   editing in the middle.

6. Keep domain types small.
   Start with the fields needed for current behavior. Add direct actions, source
   metadata, or richer value kinds only when tests force them.

7. Build the composition core first.
   Queueing, slot filling, escaping, current-value resolution, and execution
   planning should be testable without TUI or source plumbing.

## Testing Practices

1. Use red-green-refactor.
2. Add tests for behavior before implementation.
3. Add edge-case tests before fixing edge-case bugs.
4. Avoid redundant assertions.
5. Test boundary semantics explicitly, including exact match vs proper prefix,
   empty input vs no match, and search mode vs edit mode.
6. Name tests after behavior, not implementation.
7. Keep core tests deterministic and local. Avoid filesystem, shell, or terminal
   dependencies unless the behavior specifically requires them.
8. When semantics change, update old tests so their names and assertions describe
   the new contract.

## API Practices

1. Public methods should express domain operations, such as `feed`,
   `update_input`, `press_tilde`, `current`, `compose`, and `compile`.

2. Avoid test-only control APIs.
   If a test needs to fake internal state directly, the public model is probably
   wrong.

3. Keep boundaries narrow.
   Sources should eventually produce candidates. The UI should render state and
   send input changes. Neither should own core semantics.

4. Preserve invariants by default.
   Prefer private fields plus focused accessors unless direct mutation is clearly
   harmless.

5. Return domain results, not incidental details.
   For future Enter/Tab behavior, prefer an enum such as
   `ActionResult::{Queued, Execute(Value), None}` over leaking queue internals.

## Refactoring Practices

1. Refactor only after behavior is green, unless the refactor is required to make
   the test expressible.
2. Delete unused scaffolding quickly.
3. Keep commits small and behavioral.
4. Separate "add failing tests", "make green", and "refactor" when useful.
5. Review diffs before committing. If a diff includes unrelated cleanup, split
   it.

Run full verification after meaningful changes:

```text
nix develop -c cargo test
nix develop -c cargo fmt --check
nix develop -c cargo clippy --all-targets -- -D warnings
```

## Design Practices

1. Avoid heuristics for user intent.
   Prefer explicit mechanisms such as initial `[tilde]` over guessing whether
   text is a shell command.

2. Keep mode semantics sharp.
   Search mode reranks and selects. Edit mode edits a value and ignores results.

3. Escaping happens at insertion boundaries.
   `Escaped` values are quoted when inserted. Composed shell text becomes `Raw`.

4. UI should not define semantics.
   The TUI should adapt terminal events into state-machine operations. It should
   not decide what command will execute.

5. Prefer observable behavior over implementation language.
   Specs and tests should say what happens, not how the implementation must
   internally arrange it.

6. Re-check each new feature against the minimal-rule principle.
   If it needs an exception, first look for a simpler rule that covers both old
   and new behavior.

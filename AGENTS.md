# AGENTS.md

## Rule Priority

- Correctness, safety, and explicit project requirements take priority over stylistic preferences.
- Follow existing project conventions when they do not conflict with explicit instructions or correctness.
- Treat the rules in this file as defaults, not absolute requirements. Deviate when doing so clearly improves correctness, readability, or maintainability.
- When rules conflict, prefer the more specific rule over the more general one.
- Do not introduce unnecessary changes merely to comply with a stylistic rule.
- If an important requirement is ambiguous and cannot be resolved from the existing code or context, ask for clarification rather than guessing.

## General

- Prefer concise, direct, readable implementations.
- Prioritize readability over minimizing line count or following rules mechanically.
- Avoid unnecessary refactoring, indirection, and abstractions.
- Do not introduce complexity without a clear benefit.

## Naming

- Prefer concise, preferably one-word names.
- Omit redundant prefixes and suffixes when the context makes the meaning clear.
- Use qualifiers when needed to distinguish meanings or avoid ambiguity.
- Prefer verb-based function names. Use prefixes such as `to_` and `has_` when appropriate.
- Omit redundant function-name modifiers when the module, type, or context already makes the purpose clear.
- Add qualifiers when they clarify the operation, such as `resolve_latest`.

### Abbreviations

- Apply abbreviation conventions consistently to all identifiers, including variables, functions, fields, parameters, and types.
- Use common abbreviations when they are shorter and remain readable:
  - `location` → `loc`
  - `position` → `pos`
  - `column` → `col`
  - `source` → `src`
  - `destination` → `dst`
  - `start` → `st`
  - `end` → `ed`
  - `current` → `curr`
  - `previous` → `prev`
- Distinguish `location` and `position` by meaning:
  - `location` refers to where something is located or placed.
  - `position` refers to a coordinate or a position within an order or sequence.
- Use conventional plural forms. Abbreviated plurals such as `locs` and `cols` are acceptable when natural.
- Do not invent awkward plural abbreviations. Use `positions`, not `poss`.
- Prefer full words when no common abbreviation exists or abbreviation reduces readability.

## Variables and Scope

- Keep variables within the narrowest practical scope.
- Inline values used only once by default, unless naming them improves readability or avoids a complex expression.
- Construct values at the point of use when construction is effectively free and doing so keeps the code simple.
- Actively use shadowing when it improves clarity.
- Avoid unnecessary prefixes and suffixes when the context is sufficient.
- Use `path` for a generic path, `file` for a file, and `dir` for a directory. Add qualifiers only when necessary to distinguish their roles.
- Define variables used by control constructs such as `while` immediately before the construct when practical.
- If such a variable is also used after the construct, separate its declaration from the construct with a blank line.

## Functions and Closures

- Prefer concise functions with a clear responsibility.
- Do not extract trivial expressions, simple method chains, or thin wrappers into functions without a clear benefit.
- A function with a single call site is acceptable when extraction improves readability.
- Split long operations into smaller functions when doing so clarifies the logic; otherwise, keep them together.
- Use closures for small operations local to a function when appropriate, especially when the operation is reused within that function.
- Avoid creating functions solely to reduce line count or divide code mechanically.

## Error Handling

- Prefer `?` for propagating errors.
- Use `unwrap()` and `expect()` when success is guaranteed by design or when appropriate in tests.
- Propagate errors properly and preserve their causes where practical.
- Do not silently ignore errors without justification.
- Avoid excessive error wrapping and unnecessary error abstractions.

## Ownership and Cloning

- Prefer ownership moves and borrowing over unnecessary cloning.
- Copy or clone small values when that makes the code simpler than introducing complicated borrowing.
- Do not avoid cloning at the expense of readability without a meaningful benefit.
- Do not introduce unnecessary abstractions or complicated data structures merely to work around ownership or lifetime constraints.

## Abstraction and Design

- Introduce abstractions when they provide clear benefits, such as reuse, type safety, or separation of responsibilities.
- Do not introduce speculative abstractions or generalizations solely for possible future extensibility.
- Use traits, generics, and wrapper types when they meaningfully improve the design.
- Do not extract shared code solely to eliminate duplication when the result is less clear.
- Prefer direct implementations and avoid unnecessary indirection.

## Visibility

- Use the minimum visibility necessary.
- Use `pub` only when an item must be exposed outside its module.
- Prefer narrower visibility such as `pub(crate)` and `pub(super)` when appropriate.
- Keep implementation details private by default.

## Control Flow

- Use `match` when handling multiple patterns or branches.
- Use `if let` when handling a specific pattern.
- Use `let else` for early exits when a pattern does not match.
- Choose the construct that expresses the intent most clearly and concisely.

## Formatting

- Separate independent blocks and control constructs with blank lines.
- Do not force blank lines inside nested blocks or within expressions such as `if` expressions.
- Keep related statements together.
- Avoid packing independent control constructs or statements tightly together.

## Comments

- Do not write comments or documentation comments.
- This includes `//`, `/* ... */`, `///`, and `//!`.

## Verification

- Do not run Clippy. The user runs Clippy separately.
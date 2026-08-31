# ADR-0001: Separate PF2e parsing from PF2e rule authority

## Status

Accepted.

## Context

An earlier single service (`infernal-rules-extractor-pf2e`) both parsed
Pathfinder 2e source text into structured rules *and* persisted the
result as if it were authoritative domain data, in one process and one
database. That mixed two different kinds of responsibility behind one
kernel identity and one schema:

- turning source text into structured fields is a **transformation**:
  deterministic-where-possible, stateless-where-practical, and expected
  to change independently as parsing heuristics improve;
- deciding what counts as *the* rule -- its stable identity, its
  versions, its relationships, whether a proposed change actually
  replaces or extends something that already exists -- is a **domain
  authority** decision, the same kind of responsibility Librarian already
  holds for documents.

Collapsing both into one service meant a parsing change and a rule-
authority change could never be deployed, scaled, or reasoned about
independently, and meant the "authoritative" store could never
distinguish "this is what a parser proposed" from "this is what the
domain decided to trust."

## Decision

PF2e source parsing and authoritative PF2e rule storage are separate
services:

- `infernal-pf2e-parser-simple` -- a transformation service. It converts
  PF2e source text into structured rule *candidates* and hands each one
  to the Rules Service through a governed Infernal Law Request
  (`pf2e.rules.admit`). It owns no authoritative rule data, no stable
  rule IDs, no rule lifecycle, and no rule relationships. It may retain
  minimal local state to make retrying a claimed `pf2e.parse` route
  safe, but that state is a retry cache, never the rules database.
- `infernal-pf2e-rules-simple` -- the domain authority for structured
  PF2e rules. It accepts candidates from the Parser (or, eventually, any
  other PF2e parser implementation) via `pf2e.rules.admit`, validates
  them, decides rule identity and versioning, and owns the only
  authoritative PF2e rule store, search, and relationships. It never
  parses source text itself.

Both remain domain services on top of the `infernal-law` kernel, per
[`minimum-viable-kernel.md`](https://github.com/BenjaminGrandstaff/infernal-law/blob/main/docs/architecture/minimum-viable-kernel.md):
the kernel owns identity, communication admission, authorization,
durable Requests/routes, claims/leases/fencing, and audit; neither PF2e
service gained any of that, and the kernel gained no PF2e-specific
types, parsers, or relationships from this split.

## Reason

Parsing is a transformation responsibility. Rule storage,
canonicalization, versioning, relationships, and search are domain-
authority responsibilities. Keeping them separate lets a parser
implementation evolve, be replaced, or be run independently without ever
touching -- or being trusted by default by -- the authoritative rules
database.

## Consequences

- Multiple parser implementations may eventually feed the same Rules
  Service, each producing candidates in the same wire shape.
- The Rules Service must validate parser output rather than blindly
  trusting it -- see `infernal-pf2e-rules-simple`'s own `dispatch.rs`
  and `domain.rs` for what "validate, don't trust" means concretely
  (strict `rule_type` parsing, confidence-range and `parser_version`
  checks, and admission idempotency that treats a repeated
  `candidate_id` as already handled rather than as a demand to store it
  again).
- Cross-service result transfer remains governed by Infernal Law: the
  Parser submits `pf2e.rules.admit` as an ordinary signed Request under
  its own identity, never a direct call, shared database, or shared
  filesystem state with the Rules Service.
- The kernel's 200-character `scope` ceiling (ILK-006 not yet built)
  now constrains *two* governed hops instead of one -- see
  `infernal-pf2e-parser-simple`'s README, "Kernel payload limitations",
  for the accounting of what a `pf2e.rules.admit` Request can and cannot
  carry today.

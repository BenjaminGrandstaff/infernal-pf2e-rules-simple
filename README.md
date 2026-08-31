# infernal-pf2e-rules-simple

> The PF2e Rules Service owns authoritative structured Pathfinder 2e
> rule data, versions, relationships, and search. It does not parse
> source documents.

> All communication with other services is mediated through Infernal
> Law.

This is a domain service running on top of the
[infernal-law](https://github.com/BenjaminGrandstaff/infernal-law)
kernel. It is one half of a split originally implemented as a single
service, `infernal-rules-extractor-pf2e`; see
[ADR-0001](docs/architecture/decisions/0001-separate-pf2e-parsing-from-pf2e-rule-authority.md)
for why they were split, and
[`minimum-viable-kernel.md`](https://github.com/BenjaminGrandstaff/infernal-law/blob/main/docs/architecture/minimum-viable-kernel.md)
for the architectural authority both services were built against.

## Architecture

```text
PF2e Parser
    |
    | pf2e.rules.admit
    v
Infernal Law
    |
    v
PF2e Rules Service
    |
    v
PF2e rule database / indexes
```

This service owns:

- authoritative PF2e rule records
- stable rule IDs
- rule versions
- rule lifecycle
- validation of rule candidates
- deduplication
- canonicalization
- provenance links to source material
- PF2e-specific relationships (deferred to a later milestone -- see
  "What this service must not become")
- rule search (deferred; see below)

This service does **not** own:

- raw document storage (Librarian's)
- generic document parsing
- parser implementation (`infernal-pf2e-parser-simple`'s)
- OCR
- generic extraction frameworks
- Infernal Law routing or authorization

Librarian remains authoritative for source documents. This service
remains authoritative for normalized PF2e rule data. It never invokes
parser code in-process, reads the Parser's database, or reads
Librarian's database directly.

## This service validates, it does not trust

The Parser proposes; this service decides what becomes authoritative.
An admitted candidate is not trusted merely because it arrived through a
governed Request from a known parser identity. Before admitting a
candidate, this service validates (`domain.rs`, `dispatch.rs`):

- **valid source provenance** -- a well-formed `document_id`, a 32-byte
  `content_digest`, and a non-empty `location`, enforced at the wire-
  parsing layer (`dispatch::parse_scope`);
- **supported rule type** -- `rule_type` must be one of the six known
  variants; an unrecognized value is rejected as a malformed scope, not
  silently folded into `unclassified` the way the Parser's own text
  parser gracefully degrades ambiguous *prose*. A malformed wire value
  is a protocol problem, not an interpretation problem;
- **structurally valid parsed fields** -- `confidence` must be within
  `[0.0, 1.0]` (`AdmissionError::InvalidConfidence`); deeper structural
  validation of a candidate's normalized fields (`trigger`, `effect`,
  etc.) is not yet possible, because the governed `pf2e.rules.admit`
  Request does not carry them at all -- see "Kernel payload
  limitations" below;
- **content digest/provenance consistency** -- provenance is carried
  through unchanged from the Parser's own `SourceReference` and stored
  as received; this service does not re-derive or second-guess it, only
  requires it to be present and well-formed;
- **deterministic duplicate detection** -- a `candidate_id` admitted
  once is recognized on every subsequent admission attempt, globally
  enforced by a unique database constraint, not just application logic;
- **parser version recorded** -- `AdmissionError::MissingParserVersion`
  rejects a candidate with an empty `parser_version`, since this service
  will not admit a candidate whose producer cannot be identified for
  provenance.

## Kernel payload limitations

Inherited from `infernal-pf2e-parser-simple`'s own accounting (see that
repository's README): the `pf2e.rules.admit` Request's `scope` (bounded
to 200 characters) carries a candidate's identity, classification,
confidence, and full source provenance, but not its normalized parsed
fields. Concretely, this means this service's own "structurally valid
parsed fields" validation is currently limited to what actually arrives
through the governed channel (`rule_type`, `confidence`) -- it cannot
yet validate a candidate's `trigger`, `effect`, `requirements`,
`prerequisites`, or `references`, because those never reach this
service today. This is a real, current limitation of the MVP kernel
(ILK-006 artifact/content mediation, not yet built), documented here
rather than worked around with a side channel to the Parser.

## Versioning

`(document_id, name)` identifies "the same logical rule" across
versions -- there is no real PF2e knowledge graph here (see "What this
service must not become"). A second, distinct candidate admitted for a
document that already produced a rule under the same `name` becomes a
new *version* of that rule (`rule_id` unchanged, `version` incremented),
never a silent overwrite and never a wholly separate rule. A candidate
with no `name` (for example an `unclassified` candidate) always creates
a new `rule_id` at version 1 -- there is no reliable identity to chain
it onto. See `domain.rs`'s own module documentation and
`tests/domain_repository.rs`.

## Held candidates and Reserved Material

Admission is not strictly binary. A structurally valid candidate whose
`name` opens with a possessive proper-noun prefix (ORC's own named
example: "Bimbol's Bursting Bunion") is held for human review instead
of admitted or rejected outright -- see `dispatch.rs`'s own module
documentation and [ORC-NOTICE.md](ORC-NOTICE.md). A held candidate
never becomes a rule on its own; a human resolves it later, either into
an admitted rule or a permanent discard.

`cargo run --bin review` is the terminal interface to that queue. It
connects straight to `PF2E_RULES_DATABASE_URL` -- the same trust
boundary as already having database or cluster access, no new network
listener or auth surface:

```sh
cargo run --bin review -- list
cargo run --bin review -- show <held_id>
cargo run --bin review -- admit <held_id>
cargo run --bin review -- discard <held_id>
```

Both `admit` and `discard` are idempotent: resolving the same
`held_id` to the same resolution twice recognizes the prior resolution
rather than double-admitting; resolving it to a *different* resolution
than what already happened is a reported conflict, never a silent
overwrite of what a human already decided.

This one check is deliberately narrow, not comprehensive Reserved
Material filtering -- see ORC-NOTICE.md's "Known limitation" section
for what still passes through undetected.

## Cross-service failure: admission not confirmed

This is the same distributed-transaction boundary
`infernal-librarian-simple` documented first, applied to a second hop:
admission and its local commit happen inside one database transaction,
*before* this service's own kernel claim is completed, and the claim is
only completed if admission succeeds. If the Parser retries after an
unconfirmed admission, the same `candidate_id` (which the Parser's own
idempotency keeps stable across its own retries) is recognized here as
already admitted -- `was_already_processed: true`, no duplicate rule.
Proven directly:
`readmitting_the_same_candidate_id_never_creates_a_duplicate_rule` in
`tests/domain_repository.rs`, and
`a_repository_failure_never_completes_the_kernel_claim` in
`tests/kernel_adapter.rs`.

### Everything else tested

- **Rules Service crash before rule commit / after rule commit before
  kernel completion** -- admission and its commit happen inside one
  database transaction; the claim is only completed afterward. See
  above.
- **Duplicate rule candidate** --
  `readmitting_the_same_candidate_id_never_creates_a_duplicate_rule`.
- **Stale/fenced worker** --
  `reports_fencing_loss_before_completion_without_erroring`.
- **Missing source provenance / malformed candidate** --
  `a_scope_missing_the_pipe_separator_is_rejected`,
  `a_malformed_scope_fails_the_pass_without_completing_the_claim`.
- **Unsupported rule type** --
  `an_unrecognized_rule_type_is_rejected_as_a_malformed_scope_not_folded_to_unclassified`.
- **Database unavailable** --
  `a_repository_failure_never_completes_the_kernel_claim`.
- **Kernel unavailable** -- `work_once` returns `Err`, logged and
  retried on the next poll tick.

## Infernal Law integration

Using [`infernal-client-rs`](https://github.com/BenjaminGrandstaff/infernal-client-rs),
this service enrolls as its own service principal, renews its own
instance lease proactively (`POST /v1/instances/renew`, included from
the start), maintains an active inclusive subscription for
`pf2e.rules.admit`, polls `GET /v1/routes/eligible`, claims eligible
work under its own identity, reads the routed Request, admits the
candidate, and completes its own claim. Unlike
`infernal-pf2e-parser-simple`, this service never submits a governed
Request of its own -- it only ever consumes `pf2e.rules.admit` work,
matching every other consumer-only reference service in this ecosystem.

### Worker ownership and fencing

This service claims and completes its own work directly. A claim fenced
before completion is reported as `LostBeforeCompletion`, never a false
`Completed`.

## What this service must not become

- not a generic document parser
- not a universal rules engine
- not an Infernal Law kernel module
- not a Pathfinder character builder
- not a Pathfinder rules database (beyond the small, versioned store
  this milestone actually needs)
- not a search engine
- not a direct Librarian client
- not an AI agent orchestration platform

Consistent with this, rule search and PF2e-specific relationship storage
(`feat requires feat`, `condition modifies check`, and similar) are
explicitly deferred past this first milestone -- adding them now, before
a second real candidate producer exists to justify the shape, would be
exactly the "generalized... complete Pathfinder rules engine" this
service must not become.

## Configuration

- `KERNEL_AUTHORITY` (required)
- `KERNEL_CA_CERT_PATH` (optional)
- `RULES_SERVICE_ID` (required)
- `CLAIM_LEASE_SECONDS` (default `300`)
- `POLL_INTERVAL_SECONDS` (default `5`)
- `ENROLLMENT_CHALLENGE` (optional, with `SERVICE_ENDPOINT`/`POD_UID`/
  `WORKLOAD_TOKEN_PATH` as in every other reference service)
- `PF2E_RULES_DATABASE_URL` (required) -- this service's own
  authoritative store, entirely separate from infernal-law's own
  database and from `infernal-pf2e-parser-simple`'s own database.
- `HEALTH_ADDRESS` (default `0.0.0.0:8090`)

## Status

Verified live 2026-08-31 against a real kind-deployed kernel, evaluator,
and both PF2e services' own isolated PostgreSQL instances: this service
claimed a real `pf2e.rules.admit` Request submitted by a live
`infernal-pf2e-parser-simple` Deployment (itself triggered by a separate
Requester identity's signed `pf2e.parse` submission) and admitted it as
a new authoritative rule, version 1. The `candidate_id` recorded here
matched exactly the Parser's own retry-cache record for the same
candidate, and `infernal-law`'s own database contained zero
PF2e-specific tables afterward. See `infernal-pf2e-parser-simple`'s
`tests/live_requester_submission.rs` for the driving test.

## Development

```sh
cargo build
cargo test
```

## Tests

- **Unit tests** (`cargo test --lib`) -- wire-format parsing, signature
  construction, and `dispatch.rs`'s strict scope validation.
- **Domain tests** (`tests/domain_repository.rs`, live PostgreSQL,
  `#[ignore]`d) -- admission idempotency, versioning-by-name, and
  restart durability:
  ```sh
  export PF2E_RULES_DATABASE_URL='postgres://...'
  cargo test --test domain_repository -- --ignored --test-threads=1
  ```
- **Kernel adapter tests** (`tests/kernel_adapter.rs`) -- `work_once`'s
  orchestration against fakes, including the critical failure-semantics
  and fencing tests.

## Podman

```sh
podman build -t localhost/infernal-pf2e-rules-simple:latest .
```

## Kubernetes

Before this service can do anything, an operator must provision, out of
band:

1. an `identities` row for `RULES_SERVICE_ID`;
2. an enrollment binding for this service's Kubernetes ServiceAccount,
   enabled;
3. `service_communication_admission` enabled for that identity;
4. an ILK-002 authority grant for `subscription.create` under this
   service's own identity (this service never submits
   `pf2e.rules.admit` itself, so it needs no grant for that action);
5. separately, an ILK-002 authority grant for `pf2e.rules.admit` under
   the Parser's own identity (provisioned in `infernal-pf2e-parser-
   simple`'s own deployment, not here);
6. a real ADR-0008 enrollment challenge, set as `ENROLLMENT_CHALLENGE`.

## Scope discipline

Before proposing a change to `infernal-law` on this project's behalf,
stop and ask whether it protects authority, communication, or
correctness. If not, it belongs in this service or in
`infernal-pf2e-parser-simple`. Nothing in this repository's development
required a kernel change. The kernel payload gap this service inherits
(see above) is documented, not routed around with a direct call to the
Parser or a shared database.

## Success criterion

The PF2e Rules Service can accept structured rule candidates without
containing parsing logic, and PF2e parsing can be replaced, upgraded, or
run independently without changing the authoritative PF2e rules
database.

## License

MIT. See [LICENSE](LICENSE). This applies to the software only --
Pathfinder Second Edition rule content this service admits and stores
is Licensed Material under the Open RPG Creative License; see
[ORC-NOTICE.md](ORC-NOTICE.md).

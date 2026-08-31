# ORC Notice

This project stores and processes Pathfinder Second Edition rule
*mechanics* -- rule types, triggers, requirements, effects, and related
structured fields -- as Licensed Material under the Open RPG Creative
License ("ORC License").

This product is licensed under the ORC License located at the Library
of Congress at TX 9-307-067 and available online at
<https://paizo.com/orclicense> and other locations. All warranties are
disclaimed as set forth therein.

## Attribution

This product is based on the following Licensed Material: Pathfinder
Second Edition rules content, including but not limited to the
*Pathfinder Second Edition Core Rulebook*, *GM Core*, and *Monster
Core*, copyright Paizo Inc., published by Paizo Inc. under the Open
RPG Creative License.

## Reserved Material

Reserved Material elements are excluded from what this service treats
as admissible Licensed Material, including but not limited to:
Paizo's and its licensors' trademarks and trade dress; proper nouns
and setting/world lore (including Golarion-specific names, places, and
narrative content); distinctive character names, personalities, and
backstories; and visual art, maps, and music. This project does not
store or admit Reserved Material as rule data.

## Expressly designated Licensed Material

None. This project does not designate any additional Reserved
Material as Licensed Material.

## Known limitation: Reserved Material filtering is narrow, not complete

The commitment above is only partly enforced. `dispatch.rs` holds a
candidate for human review (`domain.rs`'s `HeldCandidate`/
`hold_candidate`/`resolve_held`) instead of admitting it when `name`
opens with a possessive proper-noun prefix -- ORC's own named example
("Bimbol's Bursting Bunion", which a licensee is told to rewrite as
"Bursting Bunion"). That one pattern is held, never auto-admitted and
never auto-stripped: rewriting a candidate's content on the book's
behalf without a human confirming the result would be its own kind of
guess, so a held candidate waits for `resolve_held` either way.

That is the *only* pattern detected. A proper noun with no possessive
marker at all -- a monster literally named after a setting NPC, a
place name, a trademarked term embedded mid-`effect` rather than at
the start of `name` -- passes through this check undetected and is
admitted normally. `infernal-pf2e-parser-simple`'s own extraction
(`parser.rs`, `book_adapter.rs`) does no Reserved Material filtering
either; it only reformats book text into `parser.rs`'s grammar.

This narrows the gap; it does not close it. Before any real Paizo
source text is fed through the pipeline at volume, the undetected
cases above still need a real answer -- a curated denylist of known
Reserved terms, broader heuristics, or a mandatory review step for
every candidate above some confidence -- not just this one check.

## Scope of this notice

The MIT license in [`LICENSE`](LICENSE) covers this repository's
*software* only. It does not extend any rights to Licensed Material or
Reserved Material under the ORC License, which are governed solely by
the ORC License itself. Any Licensed Material admitted, stored, or
served by this service remains available to downstream recipients
under the same ORC License terms -- this project does not impose
additional or different restrictions on it.

See also [`infernal-pf2e-parser-simple`](https://github.com/BenjaminGrandstaff/infernal-pf2e-parser-simple)'s
own `ORC-NOTICE.md`, which governs the upstream parsing hop that
produces the candidates this service admits.

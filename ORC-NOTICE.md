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

## Known limitation: Reserved Material is not yet filtered

The commitment above is a policy, not yet an enforced one. Neither
this service's admission validation (`dispatch.rs`, `domain.rs`) nor
`infernal-pf2e-parser-simple`'s extraction (`parser.rs`) currently
detects or strips proper nouns and other Reserved Material from a
candidate's `name`, `trigger`, or `effect` text -- those fields are
validated for structure (rule type, confidence range, provenance) and
otherwise stored verbatim. ORC's own guidance for a possessive-proper-
noun mechanic name (e.g. "Bimbol's Bursting Bunion") is to delete the
proper noun and keep the generic name ("Bursting Bunion"); no such
stripping exists yet on either hop.

This is a real, current gap, not a silent risk: only synthetic test
fixtures have been admitted so far (see this repository's README,
"Status"), so no actual Reserved Material has passed through this
pipeline to date. It must be addressed -- in the Parser's extraction,
this service's admission validation, or both -- before any real Paizo
source text is fed through the pipeline, not worked around after the
fact.

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

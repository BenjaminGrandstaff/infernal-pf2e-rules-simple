//! Goal: interpret a routed `pf2e.rules.admit` Request and decide whether
//! its candidate becomes an authoritative rule -- the boundary between
//! "the kernel handed us governed work" and "this service's own domain
//! decides what to trust." Nothing below this module knows infernal-law
//! exists; nothing above it knows what a PF2e rule is.
//!
//! ## Wire shape (matches `infernal-pf2e-parser-simple`'s own `dispatch.rs`)
//!
//! ```text
//! <candidate_id>@<parser_version>#<rule_type>!<confidence>|<document_id>~<document_version>~<digest_b64>~<location>~<name>
//! ```
//!
//! This module's parsing is deliberately strict about `rule_type`: an
//! unrecognized value is rejected as a malformed scope, not silently
//! folded into `unclassified` the way the Parser's own text parser
//! degrades ambiguous *prose*. A malformed wire value is a protocol
//! problem, not an interpretation problem, and this service does not
//! extend the Parser the benefit of the doubt just because the Request
//! came from a known parser identity -- see this repository's README,
//! "This service validates, it does not trust".
//!
//! Confidence range and `parser_version` presence are validated one
//! layer down, in `domain::AdmissionRepository::admit_candidate` --
//! those are domain admission rules, not wire-parsing rules.
//!
//! ## Reserved Material: one narrow, honest check
//!
//! `possessive_reserved_material_reason` catches exactly the pattern
//! ORC's own guidance calls out by name: a possessive proper-noun
//! prefix on `name` (its example: "Bimbol's Bursting Bunion", which a
//! licensee is told to rewrite as "Bursting Bunion"). A match holds the
//! candidate for human review (`AdmissionRepository::hold_candidate`)
//! rather than auto-stripping and admitting it -- rewriting a
//! candidate's content on the book's behalf without a human confirming
//! the result is exactly the "guess dressed up as a fact"
//! `infernal-pf2e-parser-simple`'s own `parser.rs` disclaims for
//! parsing, and this service extends the same standard to admission.
//!
//! This is **not** comprehensive Reserved Material detection. A proper
//! noun with no possessive marker at all -- a monster literally named
//! after a setting NPC, a place name, a trademarked term -- passes this
//! check and is admitted normally. See this repository's
//! `ORC-NOTICE.md`, "Known limitation: Reserved Material is not yet
//! filtered", which this one check narrows but does not close.

use uuid::Uuid;

use crate::domain::{AdmissionRepository, AdmittedCandidate, RuleType, SourceProvenance};
use crate::error::RulesError;

pub const ADMIT_ACTION: &str = "pf2e.rules.admit";
pub const ACTIONS: [&str; 1] = [ADMIT_ACTION];

#[derive(Debug)]
pub enum DispatchOutcome {
    Admitted {
        rule_id: Uuid,
        version: i64,
        was_already_processed: bool,
    },
    Held {
        held_id: Uuid,
        reason: String,
        was_already_processed: bool,
    },
}

pub fn dispatch(
    action: &str,
    scope: &str,
    repository: &dyn AdmissionRepository,
) -> Result<DispatchOutcome, RulesError> {
    match action {
        ADMIT_ACTION => {
            let candidate = parse_scope(scope)?;
            match possessive_reserved_material_reason(&candidate) {
                Some(reason) => {
                    let outcome = repository.hold_candidate(candidate, reason.clone())?;
                    Ok(DispatchOutcome::Held {
                        held_id: outcome.held_id,
                        reason,
                        was_already_processed: outcome.was_already_processed,
                    })
                }
                None => {
                    let outcome = repository.admit_candidate(candidate)?;
                    Ok(DispatchOutcome::Admitted {
                        rule_id: outcome.rule_id,
                        version: outcome.version,
                        was_already_processed: outcome.was_already_processed,
                    })
                }
            }
        }
        other => Err(RulesError::UnknownAction(other.to_owned())),
    }
}

/// If `candidate.name` opens with a possessive proper-noun prefix (a
/// capitalized word immediately followed by `'s`/`’s`), returns the
/// reason to hold it. Widens what gets held, never narrows it: `None`
/// means this one check found nothing, not that the name is clean --
/// see this module's own documentation.
fn possessive_reserved_material_reason(candidate: &AdmittedCandidate) -> Option<String> {
    let name = candidate.name.as_deref()?;
    let first_word = name.split_whitespace().next()?;
    let base = strip_possessive(first_word)?;
    is_capitalized_word(base).then(|| {
        format!(
            "name begins with a possessive proper-noun prefix ({first_word:?}) -- \
             ORC Reserved Material; the proper noun must be stripped before this \
             becomes an authoritative rule"
        )
    })
}

fn strip_possessive(word: &str) -> Option<&str> {
    word.strip_suffix("'s")
        .or_else(|| word.strip_suffix("\u{2019}s"))
}

fn is_capitalized_word(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(char::is_uppercase)
        && chars.all(|c| !c.is_alphabetic() || c.is_lowercase())
}

fn parse_scope(scope: &str) -> Result<AdmittedCandidate, RulesError> {
    let (header, tail) = scope
        .split_once('|')
        .ok_or(RulesError::MalformedScope("scope must contain '|'"))?;
    let (id_version_type, confidence) = header
        .split_once('!')
        .ok_or(RulesError::MalformedScope("scope header must contain '!'"))?;
    let (id_version, rule_type) = id_version_type
        .split_once('#')
        .ok_or(RulesError::MalformedScope("scope header must contain '#'"))?;
    let (candidate_id, parser_version) = id_version
        .split_once('@')
        .ok_or(RulesError::MalformedScope("scope header must contain '@'"))?;

    let fields: Vec<&str> = tail.splitn(5, '~').collect();
    let [document_id, document_version, digest_b64, location, name] = fields[..] else {
        return Err(RulesError::MalformedScope(
            "scope tail must contain exactly 5 '~'-separated fields",
        ));
    };

    let candidate_id: Uuid = candidate_id
        .parse()
        .map_err(|_| RulesError::MalformedScope("candidate_id must be a UUID"))?;
    if parser_version.is_empty() {
        return Err(RulesError::MalformedScope(
            "parser_version must not be empty",
        ));
    }
    let rule_type = RuleType::try_parse(rule_type).ok_or(RulesError::MalformedScope(
        "rule_type is not a recognized value",
    ))?;
    let confidence: f64 = confidence
        .parse()
        .map_err(|_| RulesError::MalformedScope("confidence must be a number"))?;
    let document_id: Uuid = document_id
        .parse()
        .map_err(|_| RulesError::MalformedScope("document_id must be a UUID"))?;
    let document_version: i64 = document_version
        .parse()
        .map_err(|_| RulesError::MalformedScope("document_version must be an integer"))?;
    if location.is_empty() {
        return Err(RulesError::MalformedScope("location must not be empty"));
    }
    let content_digest = decode_digest(digest_b64)?;
    let name = if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    };

    Ok(AdmittedCandidate {
        candidate_id,
        parser_version: parser_version.to_owned(),
        rule_type,
        confidence,
        name,
        source: SourceProvenance {
            document_id,
            document_version,
            content_digest,
            location: location.to_owned(),
        },
    })
}

fn decode_digest(value: &str) -> Result<[u8; 32], RulesError> {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RulesError::MalformedScope("content digest must be base64url"))?;
    bytes
        .try_into()
        .map_err(|_| RulesError::MalformedScope("content digest must be exactly 32 bytes"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use crate::domain::{AdmissionError, AdmissionOutcome, HoldOutcome};

    use super::*;

    #[derive(Default)]
    struct FakeRepository {
        calls: Mutex<Vec<AdmittedCandidate>>,
        result: Option<Result<AdmissionOutcome, AdmissionError>>,
        held_calls: Mutex<Vec<(AdmittedCandidate, String)>>,
        hold_result: Option<Result<HoldOutcome, AdmissionError>>,
    }

    impl AdmissionRepository for FakeRepository {
        fn admit_candidate(
            &self,
            candidate: AdmittedCandidate,
        ) -> Result<AdmissionOutcome, AdmissionError> {
            self.calls.lock().unwrap().push(candidate);
            match &self.result {
                Some(Ok(outcome)) => Ok(AdmissionOutcome {
                    rule_id: outcome.rule_id,
                    version: outcome.version,
                    was_already_processed: outcome.was_already_processed,
                }),
                Some(Err(error)) => Err(*error),
                None => Ok(AdmissionOutcome {
                    rule_id: Uuid::new_v4(),
                    version: 1,
                    was_already_processed: false,
                }),
            }
        }

        fn get(
            &self,
            _rule_id: Uuid,
            _version: Option<i64>,
        ) -> Result<crate::domain::Rule, AdmissionError> {
            unimplemented!("not exercised by dispatch tests")
        }

        fn hold_candidate(
            &self,
            candidate: AdmittedCandidate,
            reason: String,
        ) -> Result<HoldOutcome, AdmissionError> {
            self.held_calls.lock().unwrap().push((candidate, reason));
            match &self.hold_result {
                Some(Ok(outcome)) => Ok(HoldOutcome {
                    held_id: outcome.held_id,
                    was_already_processed: outcome.was_already_processed,
                }),
                Some(Err(error)) => Err(*error),
                None => Ok(HoldOutcome {
                    held_id: Uuid::new_v4(),
                    was_already_processed: false,
                }),
            }
        }

        fn resolve_held(
            &self,
            _held_id: Uuid,
            _resolution: crate::domain::HoldResolution,
        ) -> Result<crate::domain::ResolutionOutcome, AdmissionError> {
            unimplemented!("not exercised by dispatch tests -- dispatch only ever holds or admits")
        }

        fn get_held(&self, _held_id: Uuid) -> Result<crate::domain::HeldCandidate, AdmissionError> {
            unimplemented!("not exercised by dispatch tests -- dispatch only ever holds or admits")
        }

        fn list_pending_held(&self) -> Result<Vec<crate::domain::HeldCandidate>, AdmissionError> {
            unimplemented!("not exercised by dispatch tests -- dispatch only ever holds or admits")
        }
    }

    struct ScopeFixture {
        candidate_id: Uuid,
        parser_version: &'static str,
        rule_type: &'static str,
        confidence: f64,
        document_id: Uuid,
        version: i64,
        digest: [u8; 32],
        location: &'static str,
        name: &'static str,
    }

    impl Default for ScopeFixture {
        fn default() -> Self {
            Self {
                candidate_id: Uuid::new_v4(),
                parser_version: "pf2e-parser-0.1.0",
                rule_type: "action",
                confidence: 0.9,
                document_id: Uuid::new_v4(),
                version: 1,
                digest: [0_u8; 32],
                location: "p1",
                name: "X",
            }
        }
    }

    fn scope_for(fixture: &ScopeFixture) -> String {
        format!(
            "{}@{}#{}!{:.2}|{}~{}~{}~{}~{}",
            fixture.candidate_id,
            fixture.parser_version,
            fixture.rule_type,
            fixture.confidence,
            fixture.document_id,
            fixture.version,
            URL_SAFE_NO_PAD.encode(fixture.digest),
            fixture.location,
            fixture.name,
        )
    }

    #[test]
    fn a_well_formed_scope_is_parsed_and_forwarded_to_the_repository() {
        let repository = FakeRepository::default();
        let fixture = ScopeFixture {
            confidence: 0.9,
            version: 3,
            digest: [7_u8; 32],
            location: "p12",
            name: "Stride",
            ..ScopeFixture::default()
        };
        let candidate_id = fixture.candidate_id;
        let document_id = fixture.document_id;
        let scope = scope_for(&fixture);

        let outcome = dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(matches!(
            outcome,
            DispatchOutcome::Admitted {
                was_already_processed: false,
                ..
            }
        ));
        let calls = repository.calls.lock().unwrap();
        assert_eq!(calls[0].candidate_id, candidate_id);
        assert_eq!(calls[0].rule_type, RuleType::Action);
        assert_eq!(calls[0].name.as_deref(), Some("Stride"));
        assert_eq!(calls[0].source.document_id, document_id);
        assert_eq!(calls[0].source.location, "p12");
    }

    #[test]
    fn an_empty_name_is_parsed_as_none_not_an_empty_string() {
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            rule_type: "unclassified",
            confidence: 0.2,
            name: "",
            ..ScopeFixture::default()
        });

        dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(repository.calls.lock().unwrap()[0].name.is_none());
    }

    #[test]
    fn an_unrecognized_rule_type_is_rejected_as_a_malformed_scope_not_folded_to_unclassified() {
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            rule_type: "not-a-real-rule-type",
            confidence: 0.5,
            ..ScopeFixture::default()
        });

        let result = dispatch(ADMIT_ACTION, &scope, &repository);

        assert!(matches!(result, Err(RulesError::MalformedScope(_))));
        assert!(repository.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_scope_missing_the_pipe_separator_is_rejected() {
        let repository = FakeRepository::default();

        let result = dispatch(ADMIT_ACTION, "not-a-valid-scope", &repository);

        assert!(matches!(result, Err(RulesError::MalformedScope(_))));
    }

    #[test]
    fn unknown_actions_are_rejected_without_touching_the_repository() {
        let repository = FakeRepository::default();

        let result = dispatch("pf2e.rules.query", "irrelevant", &repository);

        match result {
            Err(RulesError::UnknownAction(action)) => assert_eq!(action, "pf2e.rules.query"),
            other => panic!("expected UnknownAction, got {other:?}"),
        }
    }

    #[test]
    fn a_domain_validation_error_propagates() {
        let repository = FakeRepository {
            result: Some(Err(AdmissionError::InvalidConfidence)),
            ..FakeRepository::default()
        };
        let scope = scope_for(&ScopeFixture::default());

        let result = dispatch(ADMIT_ACTION, &scope, &repository);

        assert!(matches!(
            result,
            Err(RulesError::Admission(AdmissionError::InvalidConfidence))
        ));
    }

    #[test]
    fn a_possessive_proper_noun_prefix_is_held_for_review_instead_of_admitted() {
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            name: "Bimbol's Bursting Bunion",
            ..ScopeFixture::default()
        });

        let outcome = dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(matches!(
            outcome,
            DispatchOutcome::Held {
                was_already_processed: false,
                ..
            }
        ));
        assert!(repository.calls.lock().unwrap().is_empty());
        let held_calls = repository.held_calls.lock().unwrap();
        assert_eq!(
            held_calls[0].0.name.as_deref(),
            Some("Bimbol's Bursting Bunion")
        );
        assert!(held_calls[0].1.contains("Bimbol's"));
    }

    #[test]
    fn the_book_s_own_curly_apostrophe_is_recognized_too() {
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            name: "Kyra\u{2019}s Radiant Blast",
            ..ScopeFixture::default()
        });

        let outcome = dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(matches!(outcome, DispatchOutcome::Held { .. }));
    }

    #[test]
    fn a_generic_name_with_no_possessive_prefix_is_admitted_normally() {
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            name: "Bursting Bunion",
            ..ScopeFixture::default()
        });

        let outcome = dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(matches!(outcome, DispatchOutcome::Admitted { .. }));
        assert!(repository.held_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_lowercase_possessive_word_is_not_mistaken_for_a_proper_noun() {
        // "someone's" is a possessive, but not a *proper* noun -- the
        // book's own generic phrasing must not be held on that basis
        // alone.
        let repository = FakeRepository::default();
        let scope = scope_for(&ScopeFixture {
            name: "someone's Item",
            ..ScopeFixture::default()
        });

        let outcome = dispatch(ADMIT_ACTION, &scope, &repository).unwrap();

        assert!(matches!(outcome, DispatchOutcome::Admitted { .. }));
    }
}

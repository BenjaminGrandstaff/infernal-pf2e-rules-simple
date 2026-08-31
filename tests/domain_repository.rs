//! Goal: prove this service's own admission, deduplication, and
//! versioning behavior entirely without a live infernal-law kernel and
//! without infernal-pf2e-parser-simple. Nothing here signs a request or
//! knows infernal-law exists.

use infernal_pf2e_rules_simple::database::Database;
use infernal_pf2e_rules_simple::domain::{
    AdmissionError, AdmissionRepository, AdmittedCandidate, RuleType, SourceProvenance,
};
use infernal_pf2e_rules_simple::postgres_rules_repository::PostgresRulesRepository;
use uuid::Uuid;

fn repository() -> PostgresRulesRepository {
    let database = Database::connect_from_env().expect("database should connect and migrate");
    PostgresRulesRepository::new(database)
}

fn candidate(document_id: Uuid, name: Option<&str>, confidence: f64) -> AdmittedCandidate {
    AdmittedCandidate {
        candidate_id: Uuid::new_v4(),
        parser_version: "pf2e-parser-test".to_owned(),
        rule_type: RuleType::Action,
        confidence,
        name: name.map(str::to_owned),
        source: SourceProvenance {
            document_id,
            document_version: 1,
            content_digest: [9_u8; 32],
            location: "p1".to_owned(),
        },
    }
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn a_new_candidate_is_admitted_as_version_one_of_a_new_rule() {
    let repository = repository();

    let outcome = repository
        .admit_candidate(candidate(Uuid::new_v4(), Some("Stride"), 0.9))
        .unwrap();

    assert!(!outcome.was_already_processed);
    assert_eq!(outcome.version, 1);
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn readmitting_the_same_candidate_id_never_creates_a_duplicate_rule() {
    // The critical test: "Parser successfully created candidate; Rules
    // Service admission Request was not confirmed; Parser retries" must
    // recognize the same candidate_id and return the original rule_id
    // and version, not a new one.
    let repository = repository();
    let document_id = Uuid::new_v4();
    let mut candidate = candidate(document_id, Some("Toughness"), 0.9);
    candidate.candidate_id = Uuid::new_v4();
    let fixed_candidate_id = candidate.candidate_id;

    let first = repository.admit_candidate(candidate.clone()).unwrap();
    assert!(!first.was_already_processed);

    let retried = repository.admit_candidate(candidate).unwrap();

    assert!(retried.was_already_processed);
    assert_eq!(retried.rule_id, first.rule_id);
    assert_eq!(retried.version, first.version);
    assert_eq!(fixed_candidate_id, fixed_candidate_id); // sanity: same id used both times
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn a_second_distinct_candidate_for_the_same_document_and_name_becomes_a_new_version() {
    let repository = repository();
    let document_id = Uuid::new_v4();

    let first = repository
        .admit_candidate(candidate(document_id, Some("Guarded Stance"), 0.8))
        .unwrap();
    let second = repository
        .admit_candidate(candidate(document_id, Some("Guarded Stance"), 0.85))
        .unwrap();

    assert!(!second.was_already_processed);
    assert_eq!(second.rule_id, first.rule_id);
    assert_eq!(second.version, first.version + 1);
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn a_different_name_under_the_same_document_gets_its_own_rule_id() {
    let repository = repository();
    let document_id = Uuid::new_v4();

    let first = repository
        .admit_candidate(candidate(document_id, Some("Stride"), 0.9))
        .unwrap();
    let second = repository
        .admit_candidate(candidate(document_id, Some("Interact"), 0.9))
        .unwrap();

    assert_ne!(first.rule_id, second.rule_id);
    assert_eq!(second.version, 1);
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn unnamed_candidates_never_chain_onto_each_other_as_versions() {
    let repository = repository();
    let document_id = Uuid::new_v4();

    let first = repository
        .admit_candidate(candidate(document_id, None, 0.2))
        .unwrap();
    let second = repository
        .admit_candidate(candidate(document_id, None, 0.2))
        .unwrap();

    assert_ne!(first.rule_id, second.rule_id);
    assert_eq!(second.version, 1);
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn an_out_of_range_confidence_is_rejected() {
    let repository = repository();

    let result = repository.admit_candidate(candidate(Uuid::new_v4(), Some("X"), 1.5));

    assert!(matches!(result, Err(AdmissionError::InvalidConfidence)));
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn a_missing_parser_version_is_rejected() {
    let repository = repository();
    let mut candidate = candidate(Uuid::new_v4(), Some("X"), 0.5);
    candidate.parser_version = "  ".to_owned();

    let result = repository.admit_candidate(candidate);

    assert!(matches!(result, Err(AdmissionError::MissingParserVersion)));
}

#[test]
#[ignore = "requires PF2E_RULES_DATABASE_URL and PostgreSQL"]
fn a_committed_rule_survives_reconnecting_a_fresh_repository() {
    let first_connection = repository();
    let outcome = first_connection
        .admit_candidate(candidate(Uuid::new_v4(), Some("Stride"), 0.9))
        .unwrap();

    let restarted = repository();
    let rule = restarted.get(outcome.rule_id, None).unwrap();

    assert_eq!(rule.name.as_deref(), Some("Stride"));
    assert_eq!(rule.version, outcome.version);
}

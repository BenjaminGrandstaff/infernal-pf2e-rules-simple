//! Goal: persist authoritative PF2e rules, enforcing admission
//! idempotency (a globally unique `candidate_id`) and the
//! `(document_id, name)` versioning identity directly in SQL where
//! practical, not just in application code.
//!
//! IDs are stored and queried as plain `text`, matching the convention
//! established in `infernal-rules-extractor-pf2e` and
//! `infernal-pf2e-parser-simple`.
//!
//! ## Resolving a held candidate stays atomic
//!
//! `admit_within` holds the core insert-or-recognize logic
//! `admit_candidate` and `resolve_held`'s `Admit` path both need. Both
//! callers run it inside their own already-open transaction and commit
//! once, so "the held row is marked admitted" and "the rule row exists"
//! can never split apart across a crash -- either both happen, or
//! neither does and the resolution is safe to retry.

use r2d2_postgres::postgres::{Row, Transaction};
use uuid::Uuid;

use crate::database::Database;
use crate::domain::{
    AdmissionError, AdmissionOutcome, AdmissionRepository, AdmittedCandidate, HeldCandidate,
    HoldOutcome, HoldResolution, ResolutionOutcome, Rule, RuleType, SourceProvenance,
};

const FIND_BY_CANDIDATE_ID_SQL: &str = "
    SELECT rule_id, version FROM rules WHERE candidate_id = $1
";

const FIND_LATEST_BY_NAME_SQL: &str = "
    SELECT rule_id, version FROM rules
    WHERE document_id = $1 AND name = $2
    ORDER BY version DESC
    LIMIT 1
";

const INSERT_RULE_SQL: &str = "
    INSERT INTO rules
        (rule_id, version, rule_type, name, confidence, document_id, document_version,
         content_digest, location, candidate_id, parser_version)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
    ON CONFLICT (candidate_id) DO NOTHING
";

const SELECT_RULE_VERSION_SQL: &str = "
    SELECT rule_id, version, rule_type, name, confidence, document_id, document_version,
           content_digest, location, candidate_id, parser_version
    FROM rules
    WHERE rule_id = $1 AND version = $2
";

const SELECT_LATEST_RULE_SQL: &str = "
    SELECT rule_id, version, rule_type, name, confidence, document_id, document_version,
           content_digest, location, candidate_id, parser_version
    FROM rules
    WHERE rule_id = $1
    ORDER BY version DESC
    LIMIT 1
";

const FIND_HELD_BY_CANDIDATE_ID_SQL: &str = "
    SELECT held_id FROM held_candidates WHERE candidate_id = $1
";

const INSERT_HELD_SQL: &str = "
    INSERT INTO held_candidates
        (held_id, candidate_id, rule_type, name, confidence, document_id, document_version,
         content_digest, location, parser_version, reason)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
    ON CONFLICT (candidate_id) DO NOTHING
";

const SELECT_HELD_SQL: &str = "
    SELECT held_id, candidate_id, rule_type, name, confidence, document_id, document_version,
           content_digest, location, parser_version, reason, resolution
    FROM held_candidates
    WHERE held_id = $1
";

/// Same projection as `SELECT_HELD_SQL`, but locks the row for the rest
/// of the transaction -- `resolve_held` reads its current resolution
/// and, if still unresolved, writes a new one, and must not let a
/// concurrent `resolve_held` for the same `held_id` interleave between
/// those two steps.
const SELECT_HELD_FOR_UPDATE_SQL: &str = "
    SELECT held_id, candidate_id, rule_type, name, confidence, document_id, document_version,
           content_digest, location, parser_version, reason, resolution
    FROM held_candidates
    WHERE held_id = $1
    FOR UPDATE
";

const SELECT_PENDING_HELD_SQL: &str = "
    SELECT held_id, candidate_id, rule_type, name, confidence, document_id, document_version,
           content_digest, location, parser_version, reason, resolution
    FROM held_candidates
    WHERE resolution IS NULL
    ORDER BY held_at
";

const RESOLVE_HELD_SQL: &str = "
    UPDATE held_candidates SET resolution = $1, resolved_at = now()
    WHERE held_id = $2
";

pub struct PostgresRulesRepository {
    database: Database,
}

impl PostgresRulesRepository {
    pub const fn new(database: Database) -> Self {
        Self { database }
    }

    pub const fn database(&self) -> &Database {
        &self.database
    }
}

fn repository_error<E: std::fmt::Display>(error: E) -> AdmissionError {
    eprintln!("rules repository error: {error}");
    AdmissionError::Repository
}

fn row_to_rule(row: &Row) -> Result<Rule, AdmissionError> {
    let rule_id: String = row.get(0);
    let rule_type: String = row.get(2);
    let document_id: String = row.get(5);
    let candidate_id: String = row.get(9);
    let digest: Vec<u8> = row.get(7);
    let content_digest: [u8; 32] = digest.try_into().map_err(|_| AdmissionError::Repository)?;
    Ok(Rule {
        rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
        version: row.get(1),
        rule_type: RuleType::try_parse(&rule_type).ok_or(AdmissionError::Repository)?,
        name: row.get(3),
        confidence: row.get(4),
        source: SourceProvenance {
            document_id: document_id
                .parse()
                .map_err(|_| AdmissionError::Repository)?,
            document_version: row.get(6),
            content_digest,
            location: row.get(8),
        },
        candidate_id: candidate_id
            .parse()
            .map_err(|_| AdmissionError::Repository)?,
        parser_version: row.get(10),
    })
}

/// Column order matches `SELECT_HELD_SQL`/`SELECT_HELD_FOR_UPDATE_SQL`/
/// `SELECT_PENDING_HELD_SQL`.
fn row_to_held_candidate(row: &Row) -> Result<HeldCandidate, AdmissionError> {
    let held_id: String = row.get(0);
    let candidate_id: String = row.get(1);
    let rule_type: String = row.get(2);
    let document_id: String = row.get(5);
    let digest: Vec<u8> = row.get(7);
    let content_digest: [u8; 32] = digest.try_into().map_err(|_| AdmissionError::Repository)?;
    Ok(HeldCandidate {
        held_id: held_id.parse().map_err(|_| AdmissionError::Repository)?,
        reason: row.get(10),
        candidate: AdmittedCandidate {
            candidate_id: candidate_id
                .parse()
                .map_err(|_| AdmissionError::Repository)?,
            parser_version: row.get(9),
            rule_type: RuleType::try_parse(&rule_type).ok_or(AdmissionError::Repository)?,
            confidence: row.get(4),
            name: row.get(3),
            source: SourceProvenance {
                document_id: document_id
                    .parse()
                    .map_err(|_| AdmissionError::Repository)?,
                document_version: row.get(6),
                content_digest,
                location: row.get(8),
            },
        },
    })
}

fn parse_resolution(value: &str) -> Result<HoldResolution, AdmissionError> {
    match value {
        "admitted" => Ok(HoldResolution::Admit),
        "discarded" => Ok(HoldResolution::Discard),
        _ => Err(AdmissionError::Repository),
    }
}

fn find_latest_rule_by_name(
    transaction: &mut Transaction<'_>,
    document_id: &str,
    name: &str,
) -> Result<Option<(String, i64)>, AdmissionError> {
    Ok(transaction
        .query_opt(FIND_LATEST_BY_NAME_SQL, &[&document_id, &name])
        .map_err(repository_error)?
        .map(|row| (row.get(0), row.get(1))))
}

/// The core of admission: validate, recognize a prior admission of the
/// same `candidate_id`, or version-and-insert a new rule. Shared by
/// `admit_candidate` and `resolve_held`'s `Admit` path -- see this
/// module's own documentation on why that sharing is what keeps
/// resolving a hold atomic.
fn admit_within(
    transaction: &mut Transaction<'_>,
    candidate: &AdmittedCandidate,
) -> Result<AdmissionOutcome, AdmissionError> {
    if !(0.0..=1.0).contains(&candidate.confidence) {
        return Err(AdmissionError::InvalidConfidence);
    }
    if candidate.parser_version.trim().is_empty() {
        return Err(AdmissionError::MissingParserVersion);
    }

    let candidate_id_text = candidate.candidate_id.to_string();

    if let Some(row) = transaction
        .query_opt(FIND_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
        .map_err(repository_error)?
    {
        let rule_id: String = row.get(0);
        let version: i64 = row.get(1);
        return Ok(AdmissionOutcome {
            rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
            version,
            was_already_processed: true,
        });
    }

    let document_id = candidate.source.document_id.to_string();
    let (rule_id, version) = match &candidate.name {
        Some(name) => match find_latest_rule_by_name(transaction, &document_id, name)? {
            Some((existing_rule_id, latest_version)) => (existing_rule_id, latest_version + 1),
            None => (Uuid::new_v4().to_string(), 1),
        },
        // No reliable identity to version against -- always a new
        // rule, never chained onto an unrelated anonymous one.
        None => (Uuid::new_v4().to_string(), 1),
    };

    let digest = candidate.source.content_digest.to_vec();
    let inserted = transaction
        .execute(
            INSERT_RULE_SQL,
            &[
                &rule_id,
                &version,
                &candidate.rule_type.as_str(),
                &candidate.name,
                &candidate.confidence,
                &document_id,
                &candidate.source.document_version,
                &digest,
                &candidate.source.location,
                &candidate_id_text,
                &candidate.parser_version,
            ],
        )
        .map_err(repository_error)?;

    if inserted == 0 {
        // Lost a race against a concurrent identical admission.
        let row = transaction
            .query_one(FIND_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
            .map_err(repository_error)?;
        let winning_rule_id: String = row.get(0);
        let winning_version: i64 = row.get(1);
        return Ok(AdmissionOutcome {
            rule_id: winning_rule_id
                .parse()
                .map_err(|_| AdmissionError::Repository)?,
            version: winning_version,
            was_already_processed: true,
        });
    }

    Ok(AdmissionOutcome {
        rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
        version,
        was_already_processed: false,
    })
}

impl AdmissionRepository for PostgresRulesRepository {
    fn admit_candidate(
        &self,
        candidate: AdmittedCandidate,
    ) -> Result<AdmissionOutcome, AdmissionError> {
        let mut connection = self.database.connection().map_err(repository_error)?;
        let mut transaction = connection.transaction().map_err(repository_error)?;
        let outcome = admit_within(&mut transaction, &candidate)?;
        transaction.commit().map_err(repository_error)?;
        Ok(outcome)
    }

    fn get(&self, rule_id: Uuid, version: Option<i64>) -> Result<Rule, AdmissionError> {
        let rule_id_text = rule_id.to_string();
        let mut connection = self.database.connection().map_err(repository_error)?;
        let row = match version {
            Some(version) => {
                connection.query_opt(SELECT_RULE_VERSION_SQL, &[&rule_id_text, &version])
            }
            None => connection.query_opt(SELECT_LATEST_RULE_SQL, &[&rule_id_text]),
        }
        .map_err(repository_error)?
        .ok_or(AdmissionError::NotFound)?;
        row_to_rule(&row)
    }

    fn hold_candidate(
        &self,
        candidate: AdmittedCandidate,
        reason: String,
    ) -> Result<HoldOutcome, AdmissionError> {
        if !(0.0..=1.0).contains(&candidate.confidence) {
            return Err(AdmissionError::InvalidConfidence);
        }
        if candidate.parser_version.trim().is_empty() {
            return Err(AdmissionError::MissingParserVersion);
        }

        let candidate_id_text = candidate.candidate_id.to_string();
        let mut connection = self.database.connection().map_err(repository_error)?;
        let mut transaction = connection.transaction().map_err(repository_error)?;

        if let Some(row) = transaction
            .query_opt(FIND_HELD_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
            .map_err(repository_error)?
        {
            let held_id: String = row.get(0);
            transaction.commit().map_err(repository_error)?;
            return Ok(HoldOutcome {
                held_id: held_id.parse().map_err(|_| AdmissionError::Repository)?,
                was_already_processed: true,
            });
        }

        let held_id = Uuid::new_v4();
        let held_id_text = held_id.to_string();
        let document_id = candidate.source.document_id.to_string();
        let digest = candidate.source.content_digest.to_vec();

        let inserted = transaction
            .execute(
                INSERT_HELD_SQL,
                &[
                    &held_id_text,
                    &candidate_id_text,
                    &candidate.rule_type.as_str(),
                    &candidate.name,
                    &candidate.confidence,
                    &document_id,
                    &candidate.source.document_version,
                    &digest,
                    &candidate.source.location,
                    &candidate.parser_version,
                    &reason,
                ],
            )
            .map_err(repository_error)?;

        if inserted == 0 {
            // Lost a race against a concurrent identical hold.
            let row = transaction
                .query_one(FIND_HELD_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
                .map_err(repository_error)?;
            let winning_held_id: String = row.get(0);
            transaction.commit().map_err(repository_error)?;
            return Ok(HoldOutcome {
                held_id: winning_held_id
                    .parse()
                    .map_err(|_| AdmissionError::Repository)?,
                was_already_processed: true,
            });
        }

        transaction.commit().map_err(repository_error)?;
        Ok(HoldOutcome {
            held_id,
            was_already_processed: false,
        })
    }

    fn resolve_held(
        &self,
        held_id: Uuid,
        resolution: HoldResolution,
    ) -> Result<ResolutionOutcome, AdmissionError> {
        let held_id_text = held_id.to_string();
        let mut connection = self.database.connection().map_err(repository_error)?;
        let mut transaction = connection.transaction().map_err(repository_error)?;

        let row = transaction
            .query_opt(SELECT_HELD_FOR_UPDATE_SQL, &[&held_id_text])
            .map_err(repository_error)?
            .ok_or(AdmissionError::NotFound)?;

        let existing_resolution: Option<String> = row.get(11);
        if let Some(existing) = existing_resolution {
            let existing = parse_resolution(&existing)?;
            if existing != resolution {
                return Err(AdmissionError::ConflictingResolution);
            }
            let outcome = match existing {
                HoldResolution::Discard => ResolutionOutcome::Discarded,
                HoldResolution::Admit => {
                    let candidate_id_text: String = row.get(1);
                    let existing_rule = transaction
                        .query_one(FIND_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
                        .map_err(repository_error)?;
                    let rule_id: String = existing_rule.get(0);
                    ResolutionOutcome::Admitted(AdmissionOutcome {
                        rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
                        version: existing_rule.get(1),
                        was_already_processed: true,
                    })
                }
            };
            transaction.commit().map_err(repository_error)?;
            return Ok(outcome);
        }

        let outcome = match resolution {
            HoldResolution::Discard => {
                transaction
                    .execute(RESOLVE_HELD_SQL, &[&"discarded", &held_id_text])
                    .map_err(repository_error)?;
                ResolutionOutcome::Discarded
            }
            HoldResolution::Admit => {
                let candidate = row_to_held_candidate(&row)?.candidate;
                let admitted = admit_within(&mut transaction, &candidate)?;
                transaction
                    .execute(RESOLVE_HELD_SQL, &[&"admitted", &held_id_text])
                    .map_err(repository_error)?;
                ResolutionOutcome::Admitted(admitted)
            }
        };
        transaction.commit().map_err(repository_error)?;
        Ok(outcome)
    }

    fn get_held(&self, held_id: Uuid) -> Result<HeldCandidate, AdmissionError> {
        let held_id_text = held_id.to_string();
        let mut connection = self.database.connection().map_err(repository_error)?;
        let row = connection
            .query_opt(SELECT_HELD_SQL, &[&held_id_text])
            .map_err(repository_error)?
            .ok_or(AdmissionError::NotFound)?;
        row_to_held_candidate(&row)
    }

    fn list_pending_held(&self) -> Result<Vec<HeldCandidate>, AdmissionError> {
        let mut connection = self.database.connection().map_err(repository_error)?;
        let rows = connection
            .query(SELECT_PENDING_HELD_SQL, &[])
            .map_err(repository_error)?;
        rows.iter().map(row_to_held_candidate).collect()
    }
}

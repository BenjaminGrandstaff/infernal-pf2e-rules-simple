//! Goal: persist authoritative PF2e rules, enforcing admission
//! idempotency (a globally unique `candidate_id`) and the
//! `(document_id, name)` versioning identity directly in SQL where
//! practical, not just in application code.
//!
//! IDs are stored and queried as plain `text`, matching the convention
//! established in `infernal-rules-extractor-pf2e` and
//! `infernal-pf2e-parser-simple`.

use r2d2_postgres::postgres::Transaction;
use uuid::Uuid;

use crate::database::Database;
use crate::domain::{
    AdmissionError, AdmissionOutcome, AdmissionRepository, AdmittedCandidate, Rule, RuleType,
    SourceProvenance,
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

fn row_to_rule(row: &r2d2_postgres::postgres::Row) -> Result<Rule, AdmissionError> {
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

impl AdmissionRepository for PostgresRulesRepository {
    fn admit_candidate(
        &self,
        candidate: AdmittedCandidate,
    ) -> Result<AdmissionOutcome, AdmissionError> {
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
            .query_opt(FIND_BY_CANDIDATE_ID_SQL, &[&candidate_id_text])
            .map_err(repository_error)?
        {
            let rule_id: String = row.get(0);
            let version: i64 = row.get(1);
            transaction.commit().map_err(repository_error)?;
            return Ok(AdmissionOutcome {
                rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
                version,
                was_already_processed: true,
            });
        }

        let document_id = candidate.source.document_id.to_string();
        let (rule_id, version) = match &candidate.name {
            Some(name) => match find_latest_rule_by_name(&mut transaction, &document_id, name)? {
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
            transaction.commit().map_err(repository_error)?;
            return Ok(AdmissionOutcome {
                rule_id: winning_rule_id
                    .parse()
                    .map_err(|_| AdmissionError::Repository)?,
                version: winning_version,
                was_already_processed: true,
            });
        }

        transaction.commit().map_err(repository_error)?;

        Ok(AdmissionOutcome {
            rule_id: rule_id.parse().map_err(|_| AdmissionError::Repository)?,
            version,
            was_already_processed: false,
        })
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
}

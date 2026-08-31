//! Goal: own everything about deciding what becomes an authoritative PF2e
//! rule -- validation, deduplication, and versioning -- from a candidate
//! `infernal-pf2e-parser-simple` proposes. This service never parses
//! source text itself and never trusts a candidate merely because it
//! arrived through a governed Request from a known parser identity. See
//! `docs/architecture/decisions/0001-separate-pf2e-parsing-from-pf2e-
//! rule-authority.md` for the full reasoning behind this split.
//!
//! ## Two independent idempotency guarantees
//!
//! - **Admission idempotency** (this module): the same `candidate_id`
//!   admitted repeatedly must never create a second rule -- this is what
//!   makes "Parser successfully created a candidate, but the admission
//!   Request was not confirmed, so Parser retries" safe. See
//!   `AdmissionRepository::admit_candidate`.
//! - **Parser idempotency** (`infernal-pf2e-parser-simple`'s own
//!   concern): the same source parsed twice under the same
//!   `parser_version` reuses the same `candidate_id` in the first place,
//!   which is what makes admission idempotency actually effective across
//!   a retry rather than merely in theory.
//!
//! ## Rule identity and versioning
//!
//! There is no real PF2e knowledge graph here (deliberately -- see this
//! repository's README, "What this service must not become"). "The same
//! logical rule" is identified by `(document_id, name)`: a second,
//! distinct candidate admitted for a document that already produced a
//! rule under the same `name` becomes a new *version* of that rule
//! (`rule_id` unchanged, `version` incremented), never a silent
//! overwrite and never a wholly separate rule. A new `name` (or a first
//! candidate for a `document_id`) creates a new `rule_id` at version 1.

use std::fmt::{self, Display, Formatter};

use uuid::Uuid;

pub const SYSTEM: &str = "pf2e";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleType {
    Action,
    Reaction,
    FreeAction,
    Feat,
    Condition,
    Unclassified,
}

impl RuleType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Reaction => "reaction",
            Self::FreeAction => "free-action",
            Self::Feat => "feat",
            Self::Condition => "condition",
            Self::Unclassified => "unclassified",
        }
    }

    /// Strict, unlike a parser's own graceful degradation: an
    /// unrecognized value is a protocol-level validation failure here,
    /// not a silent fallback -- see this module's own documentation on
    /// why the Rules Service does not trust the parser's output blindly.
    pub fn try_parse(value: &str) -> Option<Self> {
        match value {
            "action" => Some(Self::Action),
            "reaction" => Some(Self::Reaction),
            "free-action" => Some(Self::FreeAction),
            "feat" => Some(Self::Feat),
            "condition" => Some(Self::Condition),
            "unclassified" => Some(Self::Unclassified),
            _ => None,
        }
    }
}

impl Display for RuleType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Provenance to the exact source that produced a candidate -- carried
/// through unchanged from `infernal-pf2e-parser-simple`'s own
/// `SourceReference`, never re-derived or guessed at by this service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceProvenance {
    pub document_id: Uuid,
    pub document_version: i64,
    pub content_digest: [u8; 32],
    pub location: String,
}

/// A candidate as received from the Parser -- not yet trusted, not yet
/// authoritative. `rule_type` is already validated to a known variant by
/// the time this reaches `AdmissionRepository` (see `dispatch.rs`); this
/// service still separately validates `confidence`'s range and
/// `parser_version`'s presence before admitting.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmittedCandidate {
    pub candidate_id: Uuid,
    pub parser_version: String,
    pub rule_type: RuleType,
    pub confidence: f64,
    pub name: Option<String>,
    pub source: SourceProvenance,
}

/// One authoritative, versioned PF2e rule record.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub rule_id: Uuid,
    pub version: i64,
    pub rule_type: RuleType,
    pub name: Option<String>,
    pub confidence: f64,
    pub source: SourceProvenance,
    pub candidate_id: Uuid,
    pub parser_version: String,
}

#[derive(Clone, Debug)]
pub struct AdmissionOutcome {
    pub rule_id: Uuid,
    pub version: i64,
    pub was_already_processed: bool,
}

/// A candidate held for human review rather than admitted or rejected
/// outright -- the third admission outcome this module previously had
/// no room for (see this repository's `ORC-NOTICE.md`, "Known
/// limitation: Reserved Material is not yet filtered": something has
/// to receive a candidate that might carry Reserved Material without
/// either trusting it as a rule or discarding it unreviewably). A held
/// candidate never becomes a `Rule` on its own; only
/// `AdmissionRepository::resolve_held` can turn it into one, or
/// discard it permanently.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldCandidate {
    pub held_id: Uuid,
    pub candidate: AdmittedCandidate,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct HoldOutcome {
    pub held_id: Uuid,
    pub was_already_processed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoldResolution {
    Admit,
    Discard,
}

#[derive(Clone, Debug)]
pub enum ResolutionOutcome {
    Admitted(AdmissionOutcome),
    Discarded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    /// `confidence` was outside `[0.0, 1.0]` -- a structurally invalid
    /// candidate, never admitted as-is.
    InvalidConfidence,
    /// `parser_version` was empty -- this service will not admit a
    /// candidate whose producer cannot be identified for provenance.
    MissingParserVersion,
    NotFound,
    /// `resolve_held` was called for a `held_id` that was already
    /// resolved, with a *different* `HoldResolution` than the one it
    /// was actually resolved with. Resolving it again with the *same*
    /// resolution is not an error -- see `resolve_held`'s own
    /// documentation on why that has to stay idempotent too.
    ConflictingResolution,
    Repository,
}

impl Display for AdmissionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfidence => {
                formatter.write_str("candidate confidence must be between 0.0 and 1.0")
            }
            Self::MissingParserVersion => {
                formatter.write_str("candidate must record a non-empty parser_version")
            }
            Self::NotFound => formatter.write_str("rule was not found"),
            Self::ConflictingResolution => formatter
                .write_str("held candidate was already resolved with a different resolution"),
            Self::Repository => formatter.write_str("admission repository operation failed"),
        }
    }
}

impl std::error::Error for AdmissionError {}

pub trait AdmissionRepository {
    /// Validates and admits `candidate`, or recognizes a prior admission
    /// of the same `candidate_id` -- see this module's own documentation
    /// for both idempotency guarantees this method's contract depends
    /// on.
    fn admit_candidate(
        &self,
        candidate: AdmittedCandidate,
    ) -> Result<AdmissionOutcome, AdmissionError>;

    fn get(&self, rule_id: Uuid, version: Option<i64>) -> Result<Rule, AdmissionError>;

    /// Validates and holds `candidate` for human review under `reason`,
    /// or recognizes a prior hold of the same `candidate_id` -- the
    /// same idempotency guarantee `admit_candidate` gives, so a retried
    /// hold never creates a second entry in the review queue. Structural
    /// validation (`confidence` range, non-empty `parser_version`) still
    /// applies: a held candidate is uncertain in *content*, not
    /// malformed.
    fn hold_candidate(
        &self,
        candidate: AdmittedCandidate,
        reason: String,
    ) -> Result<HoldOutcome, AdmissionError>;

    /// Resolves a previously held candidate into either an admitted
    /// rule or a permanent discard. Idempotent: resolving the same
    /// `held_id` to the same `resolution` again recognizes the prior
    /// resolution rather than erroring or double-admitting; resolving
    /// it to a *different* resolution than what already happened is
    /// `AdmissionError::ConflictingResolution`, never a silent
    /// overwrite of what a human already decided.
    fn resolve_held(
        &self,
        held_id: Uuid,
        resolution: HoldResolution,
    ) -> Result<ResolutionOutcome, AdmissionError>;

    fn get_held(&self, held_id: Uuid) -> Result<HeldCandidate, AdmissionError>;

    /// Lists every held candidate still awaiting resolution -- the
    /// review queue a human (or a future, stricter automated pass)
    /// works from.
    fn list_pending_held(&self) -> Result<Vec<HeldCandidate>, AdmissionError>;
}

-- Candidates held for human review rather than admitted or rejected
-- outright -- see domain.rs's own module documentation on
-- `HeldCandidate`/`AdmissionRepository::hold_candidate`. `candidate_id`
-- is globally unique for the same reason it is on `rules`: a repeated
-- hold of the same candidate is recognized, not re-inserted.
CREATE TABLE IF NOT EXISTS held_candidates (
    held_id text PRIMARY KEY,
    candidate_id text NOT NULL UNIQUE,
    rule_type text NOT NULL,
    name text,
    confidence double precision NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    document_id text NOT NULL,
    document_version bigint NOT NULL,
    content_digest bytea NOT NULL CHECK (octet_length(content_digest) = 32),
    location text NOT NULL,
    parser_version text NOT NULL CHECK (char_length(parser_version) > 0),
    reason text NOT NULL,
    held_at timestamptz NOT NULL DEFAULT now(),
    -- NULL until `resolve_held` decides the candidate's fate; a row is
    -- never deleted on discard, so the review queue's own history stays
    -- auditable.
    resolution text CHECK (resolution IN ('admitted', 'discarded')),
    resolved_at timestamptz
);

-- Backs `list_pending_held`: candidates still awaiting a decision.
CREATE INDEX IF NOT EXISTS held_candidates_pending_idx
    ON held_candidates (held_at)
    WHERE resolution IS NULL;

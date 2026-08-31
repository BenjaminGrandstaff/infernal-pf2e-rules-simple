-- The authoritative PF2e rules store. `candidate_id` is globally unique
-- so a repeated admission of the same candidate is rejected at the
-- database level, not just in application code -- see domain.rs's own
-- module documentation on admission idempotency.
CREATE TABLE IF NOT EXISTS rules (
    rule_id text NOT NULL,
    version bigint NOT NULL CHECK (version >= 1),
    rule_type text NOT NULL,
    name text,
    confidence double precision NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    document_id text NOT NULL,
    document_version bigint NOT NULL,
    content_digest bytea NOT NULL CHECK (octet_length(content_digest) = 32),
    location text NOT NULL,
    candidate_id text NOT NULL UNIQUE,
    parser_version text NOT NULL CHECK (char_length(parser_version) > 0),
    admitted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (rule_id, version)
);

-- Backs "the same logical rule" lookup: (document_id, name) identifies a
-- rule across versions -- see domain.rs's own module documentation for
-- why a NULL name never participates in this lookup.
CREATE INDEX IF NOT EXISTS rules_document_name_idx ON rules (document_id, name);

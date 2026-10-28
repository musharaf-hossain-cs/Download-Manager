CREATE TABLE request_contexts (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    referer TEXT,
    origin TEXT,
    user_agent TEXT,
    header_profile TEXT,
    credential_service TEXT,
    credential_account TEXT,
    cookie_service TEXT,
    cookie_account TEXT,
    expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms >= 0),
    CHECK((credential_service IS NULL) = (credential_account IS NULL)),
    CHECK((cookie_service IS NULL) = (cookie_account IS NULL))
) STRICT;

CREATE INDEX request_contexts_expiry_idx ON request_contexts(expires_at_ms);

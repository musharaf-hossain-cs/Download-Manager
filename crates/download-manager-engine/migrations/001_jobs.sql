CREATE TABLE jobs (
    id TEXT PRIMARY KEY NOT NULL,
    source_url TEXT NOT NULL,
    final_url TEXT,
    display_url TEXT NOT NULL,
    filename TEXT NOT NULL CHECK(length(filename) > 0),
    destination TEXT NOT NULL CHECK(length(destination) > 0),
    state TEXT NOT NULL CHECK(state IN (
        'created', 'probing', 'queued', 'connecting', 'downloading', 'pausing',
        'paused', 'verifying', 'assembling', 'completed', 'retry_waiting',
        'needs_authentication', 'needs_refresh', 'blocked', 'failed', 'cancelled',
        'missing_local_file'
    )),
    kind TEXT NOT NULL CHECK(kind IN ('file', 'media')),
    total_bytes INTEGER CHECK(total_bytes IS NULL OR total_bytes >= 0),
    downloaded_bytes INTEGER NOT NULL CHECK(downloaded_bytes >= 0),
    category TEXT,
    queue_name TEXT,
    priority TEXT NOT NULL CHECK(priority IN ('low', 'normal', 'high')),
    source_browser TEXT,
    source_profile TEXT,
    retry_count INTEGER NOT NULL CHECK(retry_count >= 0),
    error TEXT,
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= created_at_ms),
    completed_at_ms INTEGER CHECK(completed_at_ms IS NULL OR completed_at_ms >= created_at_ms),
    CHECK(total_bytes IS NULL OR downloaded_bytes <= total_bytes)
) STRICT;

CREATE TABLE remote_identities (
    job_id TEXT PRIMARY KEY NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    etag TEXT,
    last_modified TEXT,
    content_length INTEGER CHECK(content_length IS NULL OR content_length >= 0),
    mime_type TEXT,
    content_fingerprint TEXT,
    range_capability TEXT NOT NULL CHECK(range_capability IN ('unknown', 'supported', 'unsupported'))
) STRICT;

CREATE INDEX jobs_state_updated_idx ON jobs(state, updated_at_ms);

CREATE TABLE segments (
    id TEXT PRIMARY KEY NOT NULL,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    range_start INTEGER NOT NULL CHECK(range_start >= 0),
    range_end INTEGER NOT NULL CHECK(range_end >= range_start),
    durable_offset INTEGER NOT NULL CHECK(
        durable_offset >= range_start AND durable_offset <= range_end + 1
    ),
    worker_state TEXT NOT NULL CHECK(length(worker_state) > 0),
    checksum TEXT,
    checkpoint_at_ms INTEGER NOT NULL CHECK(checkpoint_at_ms >= 0),
    UNIQUE(job_id, range_start, range_end)
) STRICT;

CREATE INDEX segments_job_range_idx ON segments(job_id, range_start);

CREATE TABLE event_log (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    timestamp_ms INTEGER NOT NULL CHECK(timestamp_ms >= 0),
    component TEXT NOT NULL CHECK(length(component) > 0),
    severity TEXT NOT NULL CHECK(length(severity) > 0),
    event_type TEXT NOT NULL CHECK(length(event_type) > 0),
    correlation_id TEXT,
    redacted_detail TEXT NOT NULL
) STRICT;

CREATE INDEX event_log_job_sequence_idx ON event_log(job_id, sequence);
CREATE INDEX event_log_timestamp_idx ON event_log(timestamp_ms);

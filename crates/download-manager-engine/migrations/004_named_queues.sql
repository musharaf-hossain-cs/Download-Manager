CREATE TABLE download_queues (
    name TEXT PRIMARY KEY NOT NULL CHECK(length(name) BETWEEN 1 AND 256),
    position INTEGER NOT NULL UNIQUE CHECK(position >= 0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0)
) STRICT;

INSERT INTO download_queues (name, position, created_at_ms)
SELECT
    queue_name,
    ROW_NUMBER() OVER (ORDER BY MIN(created_at_ms), queue_name) - 1,
    MIN(created_at_ms)
FROM jobs
WHERE queue_name IS NOT NULL
GROUP BY queue_name;

CREATE INDEX download_queues_position_idx ON download_queues(position);

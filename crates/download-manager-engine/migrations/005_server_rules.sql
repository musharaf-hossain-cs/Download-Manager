CREATE TABLE server_rules (
    host TEXT PRIMARY KEY NOT NULL CHECK(length(host) BETWEEN 1 AND 253),
    connection_cap INTEGER CHECK(connection_cap BETWEEN 1 AND 32),
    user_agent TEXT CHECK(user_agent IS NULL OR length(user_agent) BETWEEN 1 AND 512),
    headers_json TEXT NOT NULL,
    disable_segmentation INTEGER NOT NULL CHECK(disable_segmentation IN (0, 1)),
    proxy_json TEXT
) STRICT;

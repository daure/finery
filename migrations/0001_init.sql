CREATE TABLE IF NOT EXISTS change_sets (
    public_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS ticket_changes (
    change_set_id TEXT NOT NULL REFERENCES change_sets(public_id) ON DELETE CASCADE,
    ticket_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (change_set_id, ticket_id)
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

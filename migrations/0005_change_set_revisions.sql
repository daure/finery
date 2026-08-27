ALTER TABLE change_sets ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;

CREATE TABLE IF NOT EXISTS change_set_catalog (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision BIGINT NOT NULL DEFAULT 1
);

INSERT INTO change_set_catalog (id, revision) VALUES (1, 1) ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS recent_tickets (
    ticket_key TEXT PRIMARY KEY,
    opened_order BIGINT NOT NULL
);

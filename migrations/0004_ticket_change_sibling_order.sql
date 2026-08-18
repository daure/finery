ALTER TABLE ticket_changes ADD COLUMN sibling_order INTEGER NOT NULL DEFAULT 0;

UPDATE ticket_changes AS target
SET sibling_order = (
    SELECT COUNT(*)
    FROM ticket_changes AS earlier
    WHERE earlier.change_set_id = target.change_set_id
      AND earlier.ticket_id < target.ticket_id
);

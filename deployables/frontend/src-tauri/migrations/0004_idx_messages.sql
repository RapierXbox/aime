CREATE INDEX idx_messages_pending_backfill
ON messages (account_id)
WHERE internal_date IS NULL;

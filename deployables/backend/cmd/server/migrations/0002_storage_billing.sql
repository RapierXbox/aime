-- +goose Up
-- storage is billed per stored byte over time, not per upload; this marks up to when an account is paid
ALTER TABLE accounts ADD COLUMN storage_billed_at TIMESTAMPTZ;

-- usage rows are no longer only tokens
ALTER TABLE usage_events RENAME COLUMN input_tokens TO quantity;
ALTER TABLE usage_events ADD COLUMN unit TEXT NOT NULL DEFAULT 'tokens'; -- tokens | mb_day

-- delta backups: a version may be a diff against an earlier one. deleting a base with
-- dependants is refused (default NO ACTION), deleting the account still cascades everything
ALTER TABLE backups ADD COLUMN kind TEXT NOT NULL DEFAULT 'full'; -- full | delta
ALTER TABLE backups ADD COLUMN parent_id BIGINT REFERENCES backups(id);
CREATE INDEX ON backups (parent_id) WHERE parent_id IS NOT NULL;

-- +goose Down
DROP INDEX IF EXISTS backups_parent_id_idx;
ALTER TABLE backups DROP COLUMN parent_id;
ALTER TABLE backups DROP COLUMN kind;
ALTER TABLE usage_events DROP COLUMN unit;
ALTER TABLE usage_events RENAME COLUMN quantity TO input_tokens;
ALTER TABLE accounts DROP COLUMN storage_billed_at;

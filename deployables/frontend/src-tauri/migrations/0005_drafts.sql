ALTER TABLE messages ADD COLUMN compose_metadata TEXT;
CREATE TABLE drafts (
    id TEXT PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES email_accounts(id) ON DELETE CASCADE,
    gmail_draft_id TEXT,
    data TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (account_id, gmail_draft_id)
);
CREATE INDEX drafts_account ON drafts(account_id, updated_at DESC);
CREATE TABLE draft_attempts (
    id TEXT PRIMARY KEY,
    draft_id TEXT NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('create','update','delete','send')),
    data TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('pending','unknown','confirmed','rejected','resolved'))
);
CREATE UNIQUE INDEX one_pending_draft_attempt ON draft_attempts(draft_id)
    WHERE outcome IN ('pending','unknown');

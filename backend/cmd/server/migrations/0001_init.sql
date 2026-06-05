-- +goose Up
CREATE TABLE accounts (
    id BIGSERIAL PRIMARY KEY,
    email TEXT UNIQUE NOT NULL,
    balance_microcredits BIGINT NOT NULL DEFAULT 0,
    stripe_customer_id TEXT, -- for later?! prob not
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE devices (
    id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    pg_alg TEXT NOT NULL DEFAULT 'ml-dsa-65',
    public_key BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ
);

CREATE TABLE auth_challanges (
    id BIGSERIAL PRIMARY KEY,
    device_id BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    nonce BYTEA NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    used BOOLEAN NOT NULL DEFAULT false
);

CREATE TABLE sessions (
    id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id BIGINT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE usage_events (
    id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind TEXT NOT NULL, -- embedding vs rerank
    model TEXT NOT NULL,
    backend TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    cost_microcredits BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE backup (
    id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    storage_path TEXT NOT NULL, -- file on the backups volume
    size_bytes BIGINT NOT NULL,
    checksum TEXT NOT NULL, -- client supplied opaque
    meta JSONB NOT NULL DEFAULT '{}',  -- opaque crypto header
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- +goose Down
DROP TABLE backups; DROP TABLE usage_events; DROP TABLE sessions; DROP TABLE auth_challanges; DROP TABLE devices; DROP TABLE accounts
-- completly freestyld of the dome.. num of changes made: its 7 now
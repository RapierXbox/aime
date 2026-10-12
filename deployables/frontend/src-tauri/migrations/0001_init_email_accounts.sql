CREATE TABLE IF NOT EXISTS email_accounts (
    id INTEGER PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    account_name TEXT NOT NULL,
    account_config TEXT NOT NULL --JSON string
);

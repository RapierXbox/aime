CREATE TABLE messages (
    account_id INTEGER NOT NULL,
    provider_msg_id TEXT NOT NULL,
    thread_id TEXT, -- nullable for forward compatability, since imap doesnt provide a thread_id natively
    sync_cursor TEXT, -- e.g. gmail history_id

    internal_date INTEGER,
    size_estimate INTEGER,
    date_header TEXT,

    from_addr TEXT,
    to_addrs TEXT, -- CSV
    cc_addrs TEXT, -- CSV

    -- for IMAP threading
    in_reply_to TEXT,
    msg_references TEXT,

    subject TEXT,
    snippet TEXT,

    -- body, attachments etc
    -- are stored separately in message_contents

    PRIMARY KEY (account_id, provider_msg_id),
    FOREIGN KEY (account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
);

CREATE TABLE message_contents (
    account_id INTEGER NOT NULL,
    provider_msg_id TEXT NOT NULL,
    mime_type TEXT CHECK (mime_type IN ('text/plain', 'text/html')) NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (account_id, provider_msg_id, mime_type),
    FOREIGN KEY (account_id, provider_msg_id) REFERENCES messages(account_id, provider_msg_id) ON DELETE CASCADE
);

CREATE TABLE message_has_label (
    account_id INTEGER NOT NULL,
    provider_msg_id TEXT NOT NULL,
    label_id TEXT NOT NULL,
    PRIMARY KEY (account_id, provider_msg_id, label_id),
    FOREIGN KEY (account_id, provider_msg_id) REFERENCES messages(account_id, provider_msg_id) ON DELETE CASCADE,
    FOREIGN KEY (label_id) REFERENCES labels(id)
);

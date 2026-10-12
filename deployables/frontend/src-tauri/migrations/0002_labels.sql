
-- limited fields because gmail's users.labels.list doesn't return all fields
CREATE TABLE labels (
    account_id INTEGER NOT NULL,
    -- id, name, messageListVisibility, labelListVisibility, and type
    id TEXT,
    name TEXT NOT NULL,

    -- wether the label is shown in the message list
    message_list_visibility TEXT
        CHECK (message_list_visibility IN ('show', 'hide')),

    -- wether the label is shown in the label list
    label_list_visibility TEXT
        CHECK (label_list_visibility IN ('labelShow', 'labelShowIfUnread', 'labelHide')),

    type TEXT
        CHECK (type IN ('system', 'user'))
        NOT NULL,

    PRIMARY KEY (account_id, id),
    FOREIGN KEY (account_id) REFERENCES email_accounts(id) ON DELETE CASCADE
);

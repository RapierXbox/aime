/*
Message {
    classification_label_values: None,
    history_id: None,
    id: Some(
        "19cc369aaedbcc50",
    ),
    internal_date: None,
    label_ids: None,
    payload: None,
    raw: None,
    size_estimate: None,
    snippet: None,
    thread_id: Some(
        "19cc369aaedbcc50",
    ),
}
*/

CREATE TABLE gmail_messages (
    id INTEGER PRIMARY KEY,
    google_message_id TEXT NOT NULL UNIQUE,
    thread_id TEXT NOT NULL,
    history_id TEXT,
    label_ids TEXT, -- JSON array
    snippet TEXT,
    internal_date INTEGER
);

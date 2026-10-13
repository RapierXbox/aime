use crate::{email::repo::Message, AppError};
use mail_builder::{headers::address::Address, MessageBuilder};
use mailparse::{addrparse_header, MailAddr};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct DraftFields {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body_text: String,
    /// The user's own text as HTML; always set, even for plain-text drafts.
    pub body_html: String,
    /// The quoted original (reply/forward), kept verbatim and never edited.
    pub quote_html: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub enum DraftStatus {
    Local,
    Synced,
    Conflict,
    Missing,
    Unsupported,
    Sending,
    Unknown,
    Sent,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub enum ComposeMode {
    New,
    Reply,
    ReplyAll,
    Forward,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub enum ConflictChoice {
    UseGmail,
    KeepLocalAsCopy,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub enum SendResolution {
    ConfirmSent,
    AllowManualRetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RemoteDraft {
    pub id: String,
    pub message_id: String,
    pub fields: DraftFields,
    pub issues: Vec<String>,
    pub thread_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub operation_id: Option<String>,
    pub from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct Draft {
    pub id: String,
    pub account_id: String,
    pub fields: DraftFields,
    pub revision: u32,
    pub acknowledged_revision: u32,
    pub status: DraftStatus,
    pub remote: Option<RemoteDraft>,
    pub conflict: Option<RemoteDraft>,
    pub issues: Vec<String>,
    pub thread_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub attempt_id: Option<String>,
    pub attempt_kind: Option<String>,
    pub sent_message_id: Option<String>,
}

impl Draft {
    pub fn new(account_id: i64, fields: DraftFields) -> Self {
        Self {
            id: random_id(),
            account_id: account_id.to_string(),
            fields,
            revision: 1,
            acknowledged_revision: 0,
            status: DraftStatus::Local,
            remote: None,
            conflict: None,
            issues: vec![],
            thread_id: None,
            in_reply_to: None,
            references: None,
            attempt_id: None,
            attempt_kind: None,
            sent_message_id: None,
        }
    }

    /// Whether the user may still edit this draft in its current status.
    pub fn editable(&self) -> bool {
        matches!(
            self.status,
            DraftStatus::Local | DraftStatus::Synced | DraftStatus::Conflict | DraftStatus::Missing
        )
    }

    /// Never uploaded and no text typed yet (empty, or only a quote): opening
    /// compose/reply alone must not create a Gmail draft. Uploaded drafts that get
    /// cleared still sync, so Gmail doesn't keep the old text.
    pub fn is_untouched(&self) -> bool {
        self.remote.is_none() && self.fields.body_text.trim().is_empty()
    }

    /// Replaces local state with `remote` and marks it acknowledged.
    pub fn adopt(&mut self, remote: RemoteDraft) {
        self.fields = remote.fields.clone();
        self.issues = remote.issues.clone();
        self.thread_id = remote.thread_id.clone();
        self.in_reply_to = remote.in_reply_to.clone();
        self.references = remote.references.clone();
        self.revision += 1;
        self.acknowledged_revision = self.revision;
        self.status = if remote.issues.is_empty() {
            DraftStatus::Synced
        } else {
            DraftStatus::Unsupported
        };
        self.remote = Some(remote);
        self.conflict = None;
    }
}

/// 32 random alphanumeric chars, used for draft and operation ids.
pub fn random_id() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 32)
}

/// Parses an address list into `(display name, address)` pairs; rejects header injection.
/// Address grammar is separate from Gmail's already-parsed MIME tree.
pub fn parse_addresses(input: &str) -> Result<Vec<(String, String)>, AppError> {
    if input.contains(['\r', '\n', '\0']) {
        return Err(AppError::InvalidRecipients);
    }

    if input.trim().is_empty() {
        return Ok(vec![]);
    }

    let raw = format!("To: {input}\r\n\r\n");
    let (headers, _) =
        mailparse::parse_headers(raw.as_bytes()).map_err(|_| AppError::InvalidRecipients)?;
    let list = addrparse_header(&headers[0]).map_err(|_| AppError::InvalidRecipients)?;
    let mut result = Vec::new();
    for address in list.iter() {
        let singles = match address {
            MailAddr::Single(a) => vec![a],
            MailAddr::Group(g) => g.addrs.iter().collect(),
        };
        for a in singles {
            let (local, domain) = a.addr.rsplit_once('@').ok_or(AppError::InvalidRecipients)?;
            if local.is_empty()
                || domain.is_empty()
                || !email_address::EmailAddress::is_valid(&a.addr)
                || a.addr.contains(['\r', '\n', '\0'])
            {
                return Err(AppError::InvalidRecipients);
            }
            let name = a.display_name.clone().unwrap_or_default();
            if name.contains(['\r', '\n', '\0']) {
                return Err(AppError::InvalidRecipients);
            }
            result.push((name, a.addr.clone()));
        }
    }
    if result.is_empty() && !list.iter().any(|a| matches!(a, MailAddr::Group(_))) {
        return Err(AppError::InvalidRecipients);
    }
    Ok(result)
}
/// Renders `(name, address)` pairs back into a header value.
fn format_addresses(input: Vec<(String, String)>) -> String {
    input
        .into_iter()
        .map(|(name, addr)| {
            if name.is_empty() {
                addr
            } else {
                format!(
                    "\"{}\" <{}>",
                    name.replace('\\', "\\\\").replace('"', "\\\""),
                    addr
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
/// Parses space-separated `<id@host>` tokens into bare message ids.
fn parse_message_ids(value: &str) -> Result<Vec<String>, AppError> {
    let mut result = Vec::new();
    for token in value.split_whitespace() {
        if !token.starts_with('<')
            || !token.ends_with('>')
            || !token.contains('@')
            || token.contains(['\r', '\n', '\0'])
        {
            return Err(AppError::InvalidReplyHeaders);
        }
        let inner = &token[1..token.len() - 1];
        if inner.contains(['<', '>']) {
            return Err(AppError::InvalidReplyHeaders);
        }
        result.push(inner.to_string());
    }
    Ok(result)
}
/// Builds the RFC 822 message for `draft`; HTML part is body + quote, text part is the body only.
pub fn build_mime_message(
    draft: &Draft,
    sender: &str,
    operation: &str,
    sending: bool,
) -> Result<Vec<u8>, AppError> {
    if draft.fields.subject.contains(['\r', '\n', '\0']) {
        return Err(AppError::InvalidRecipients);
    }
    let to = parse_addresses(&draft.fields.to)?;
    let cc = parse_addresses(&draft.fields.cc)?;
    let bcc = parse_addresses(&draft.fields.bcc)?;
    if sending && to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(AppError::InvalidRecipients);
    }
    let mut builder = MessageBuilder::new()
        .from(sender.to_string())
        .subject(draft.fields.subject.clone())
        // ponytail: text part omits the quote; add when text-only recipients matter
        .html_body(format!(
            "{}{}",
            draft.fields.body_html, draft.fields.quote_html
        ))
        .text_body(draft.fields.body_text.clone())
        .message_id(format!("{operation}@aime.local"))
        .header(
            "X-Aime-Operation",
            mail_builder::headers::text::Text::new(operation.to_string()),
        );
    let list = |items: Vec<(String, String)>| {
        Address::new_list(
            items
                .into_iter()
                .map(|(name, addr)| {
                    Address::new_address(if name.is_empty() { None } else { Some(name) }, addr)
                })
                .collect(),
        )
    };
    if !to.is_empty() {
        builder = builder.to(list(to));
    }
    if !cc.is_empty() {
        builder = builder.cc(list(cc));
    }
    if !bcc.is_empty() {
        builder = builder.bcc(list(bcc));
    }
    if let Some(value) = &draft.in_reply_to {
        builder = builder.in_reply_to(parse_message_ids(value)?);
    }
    if let Some(value) = &draft.references {
        builder = builder.references(parse_message_ids(value)?);
    }
    Ok(builder.write_to_vec()?)
}
type Mailbox = (String, String);

impl Draft {
    /// Builds the draft for `mode`: empty for `New`, a quoted reply or forward of `source` otherwise.
    pub fn try_compose(
        account: i64,
        sender: &str,
        mode: ComposeMode,
        source: Option<Message>,
    ) -> Result<Self, AppError> {
        let mut d = Draft::new(account, DraftFields::default());
        match (mode, source) {
            (ComposeMode::New, _) => {}
            (ComposeMode::Reply | ComposeMode::ReplyAll, Some(source)) => {
                let (to, cc) = reply_recipients(sender, mode, &source)?;
                let parent = source
                    .message_id
                    .clone()
                    .ok_or(AppError::InvalidReplyHeaders)?;
                if parse_message_ids(&parent)?.len() != 1 {
                    return Err(AppError::InvalidReplyHeaders);
                }
                d.fields.to = format_addresses(to);
                d.fields.cc = format_addresses(cc);
                d.fields.subject = prefixed_subject("Re:", source.subject.as_deref().unwrap_or(""));
                d.references = Some(
                    format!("{} {}", source.references.as_deref().unwrap_or(""), parent)
                        .trim()
                        .to_string(),
                );
                d.in_reply_to = Some(parent);
                d.thread_id = source.thread_id.clone();
                let header = escape_html(&format!(
                    "On {}, {} wrote:",
                    source.date_header.as_deref().unwrap_or(""),
                    source.from_addr.as_deref().unwrap_or("")
                ));
                d.fields.quote_html =
                    gmail_quote_html(&header, &quoted_original_html(&source), mode);
            }
            (ComposeMode::Forward, Some(source)) => {
                let subject = source.subject.as_deref().unwrap_or("");
                d.fields.subject = prefixed_subject("Fwd:", subject);
                let header = [
                    "Start of the forwarded message:".to_string(),
                    format!(
                        "From: {}",
                        escape_html(source.from_addr.as_deref().unwrap_or(""))
                    ),
                    format!(
                        "Date: {}",
                        escape_html(source.date_header.as_deref().unwrap_or(""))
                    ),
                    format!("Subject: {}", escape_html(subject)),
                    format!(
                        "To: {}",
                        escape_html(source.to_addrs.as_deref().unwrap_or(""))
                    ),
                ]
                .join("<br>");
                d.fields.quote_html =
                    gmail_quote_html(&header, &quoted_original_html(&source), mode);
            }
            (_, None) => return Err(AppError::DraftNotFound),
        }
        Ok(d)
    }
}

/// Addresses from `list` not yet in `seen`, which they are then added to.
fn unseen(seen: &mut HashSet<String>, list: &str) -> Result<Vec<Mailbox>, AppError> {
    Ok(parse_addresses(list)?
        .into_iter()
        .filter(|(_, a)| seen.insert(a.to_ascii_lowercase()))
        .collect())
}

/// `(to, cc)` for a reply: Reply-To or From minus ourselves; reply-all adds To and Cc, deduplicated.
fn reply_recipients(
    sender: &str,
    mode: ComposeMode,
    source: &Message,
) -> Result<(Vec<Mailbox>, Vec<Mailbox>), AppError> {
    let from = source.from_addr.as_deref().unwrap_or("");
    let visible_to = source.to_addrs.as_deref().unwrap_or("");
    let cc = source.cc_addrs.as_deref().unwrap_or("");
    let mut seen = HashSet::from([sender.to_ascii_lowercase()]);
    let mut to = unseen(&mut seen, source.reply_to.as_deref().unwrap_or(from))?;
    // Replying to our own Sent message targets its visible recipients, never its Bcc list.
    let own = parse_addresses(from)?
        .iter()
        .any(|(_, a)| a.eq_ignore_ascii_case(sender));
    if to.is_empty() && own {
        to = unseen(&mut seen, visible_to)?;
    }
    match mode {
        ComposeMode::ReplyAll => {
            to.extend(unseen(&mut seen, visible_to)?);
            Ok((to, unseen(&mut seen, cc)?))
        }
        _ => Ok((to, vec![])),
    }
}

/// `subject` with `prefix` (e.g. "Re:") unless it already starts with it, ignoring case.
fn prefixed_subject(prefix: &str, subject: &str) -> String {
    if subject
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        subject.to_string()
    } else {
        format!("{prefix} {subject}")
    }
}

/// The original's HTML body content, else its escaped text, else nothing.
fn quoted_original_html(source: &Message) -> String {
    let part = |mime: &str| {
        source
            .contents
            .iter()
            .find(|c| c.mime_type == mime)
            .map(|c| c.body.as_str())
    };
    match (part("text/html"), part("text/plain")) {
        // ponytail: drops template class-based styling; scope selectors under .gmail_quote if templates degrade
        (Some(html), _) => strip_style_blocks(html_body_content(html)),
        (None, Some(text)) => text_to_html(text),
        _ => String::new(),
    }
}

/// Wraps the original in Gmail's quote markup: a blockquote for replies, a plain block for forwards.
fn gmail_quote_html(header_html: &str, original_html: &str, mode: ComposeMode) -> String {
    match mode {
        ComposeMode::Forward => format!(
            r#"<div class="gmail_quote"><div class="gmail_attr">{header_html}<br></div><br>{original_html}</div>"#
        ),
        _ => format!(
            r#"<div class="gmail_quote"><div class="gmail_attr">{header_html}<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px #ccc solid;padding-left:1ex">{original_html}</blockquote></div>"#
        ),
    }
}

/// Escapes `& < > "` for HTML text and attribute values.
fn escape_html(text: &str) -> String {
    text.chars().fold(String::new(), |mut out, c| {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
        out
    })
}

/// Plain text as HTML: escaped, with `<br>` between lines.
pub fn text_to_html(text: &str) -> String {
    text.lines()
        .map(escape_html)
        .collect::<Vec<_>>()
        .join("<br>")
}

/// What lies between `<body…>` and `</body>` (case-insensitive), or the whole string if absent.
// ponytail: string scan, not a parser
fn html_body_content(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<body")
        .and_then(|i| lower[i..].find('>').map(|j| i + j + 1));
    match (start, lower.rfind("</body")) {
        (Some(s), Some(e)) if s <= e => &html[s..e],
        (Some(s), None) => &html[s..],
        _ => html,
    }
}

/// `html` without its `<style>…</style>` blocks, which would restyle the user's own text.
fn strip_style_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let block = lower.find("<style").and_then(|s| {
        lower[s..]
            .find("</style>")
            .map(|e| (s, s + e + "</style>".len()))
    });
    match block {
        Some((s, e)) => format!("{}{}", &html[..s], strip_style_blocks(&html[e..])),
        None => html.to_string(),
    }
}

/// Splits an uploaded body into `(user text, quote)` at the first Gmail quote block.
// ponytail: marker match; Gmail and our own output both emit it at top level
pub fn split_gmail_quote(html: &str) -> (&str, &str) {
    html.find(r#"<div class="gmail_quote"#)
        .map_or((html, ""), |i| html.split_at(i))
}

/// Whether the remote draft carries the same threading headers as the local one.
pub fn same_threading_headers(a: &Draft, b: &RemoteDraft) -> bool {
    let normalized = |s: &Option<String>| {
        s.as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    normalized(&a.in_reply_to) == normalized(&b.in_reply_to)
        && normalized(&a.references) == normalized(&b.references)
        && a.thread_id
            .as_ref()
            .is_none_or(|id| b.thread_id.as_ref() == Some(id))
}

/// Whether both reply headers parse as message ids.
pub fn valid_reply_headers(parent: &Option<String>, references: &Option<String>) -> bool {
    parent
        .as_deref()
        .is_none_or(|s| parse_message_ids(s).is_ok())
        && references
            .as_deref()
            .is_none_or(|s| parse_message_ids(s).is_ok())
}

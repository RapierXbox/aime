import { z } from "zod";
import {
  commands,
  type Draft,
  type DraftFields,
  type ComposeMode,
} from "@/bindings";

export function emailError(error: unknown): string {
  const name =
    typeof error === "string"
      ? error
      : error && typeof error === "object"
        ? Object.keys(error)[0]
        : "";
  const messages: Record<string, string> = {
    DraftRevisionConflict:
      "The saved draft changed. Review both versions before continuing.",
    DraftLocked: "This draft is locked while its Gmail status is resolved.",
    InvalidRecipients:
      "Check recipient addresses and subject. At least one recipient is required to send.",
    InvalidReplyHeaders:
      "The source email has missing or invalid reply headers.",
    EmailAuthorizationRequired:
      "Reconnect this Gmail account in Settings to authorize email operations.",
    EmailProviderRejected:
      "Gmail rejected the operation. Your draft is retained.",
    EmailNetworkError: "Cannot connect to Gmail. Sending has not been queued.",
    EmailOutcomeUnknown:
      "Gmail status is unknown. Check Gmail before retrying.",
  };
  return (
    messages[name ?? ""] ??
    "Could not complete the email operation. Your local edits are retained."
  );
}
export async function unwrap<T>(
  promise: Promise<
    { status: "ok"; data: T } | { status: "error"; error: unknown }
  >,
): Promise<T> {
  const result = await promise;
  if (result.status === "error") throw result.error;
  return result.data;
}
export const draftFieldsSchema = z.object({
  to: z.string(),
  cc: z.string(),
  bcc: z.string(),
  subject: z.string(),
  body_text: z.string(),
  body_html: z.string(),
  quote_html: z.string(),
});
// These are local callable tools, not a network endpoint or a model runtime.
// Sending and send-attempt resolution are intentionally absent.
export const draftTools = {
  list: {
    description: "List locally saved email drafts for one account",
    schema: z.object({ account_id: z.string() }),
    execute: ({ account_id }: { account_id: string }) =>
      unwrap(commands.listDrafts(account_id)),
  },
  read: {
    description: "Read an email draft and its current revision",
    schema: z.object({ draft_id: z.string() }),
    execute: ({ draft_id }: { draft_id: string }) =>
      unwrap(commands.getDraft(draft_id)),
  },
  create: {
    description: "Create an unsent draft; never sends email",
    schema: z.object({
      account_id: z.string(),
      mode: z.enum(["New", "Reply", "ReplyAll", "Forward"]),
      source_message_id: z.string().nullable(),
    }),
    execute: ({
      account_id,
      mode,
      source_message_id,
    }: {
      account_id: string;
      mode: ComposeMode;
      source_message_id: string | null;
    }) => unwrap(commands.createDraft(account_id, mode, source_message_id)),
  },
  edit: {
    description:
      "Save draft fields using the expected revision; never sends email",
    schema: z.object({
      draft_id: z.string(),
      expected_revision: z.number().int().positive(),
      fields: draftFieldsSchema,
    }),
    execute: ({
      draft_id,
      expected_revision,
      fields,
    }: {
      draft_id: string;
      expected_revision: number;
      fields: DraftFields;
    }) => unwrap(commands.saveDraft(draft_id, expected_revision, fields)),
  },
};
export const editable = (draft: Draft) =>
  ["Local", "Synced", "Conflict", "Missing"].includes(draft.status);

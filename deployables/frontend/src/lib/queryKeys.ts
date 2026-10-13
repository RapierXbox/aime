// tanstack query key factory
export const qk = {
  drafts: (accountId: string) => ["drafts", accountId] as const,
  draft: (accountId: string, id: string) => ["draft", accountId, id] as const,
  accounts: ["accounts"] as const,
  inbox: (inboxId: string, accountId: string) =>
    ["inbox", accountId, inboxId] as const,
  message: (accountId: string, msgId: string) =>
    ["message", accountId, msgId] as const,
} as const;

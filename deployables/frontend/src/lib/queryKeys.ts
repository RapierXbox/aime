// tanstack query key factory
export const qk = {
  accounts: ["accounts"] as const,
  inbox: (inboxId: string, accountId: string) =>
    ["inbox", accountId, inboxId] as const,
  message: (accountId: string, msgId: string) =>
    ["message", accountId, msgId] as const,
} as const;

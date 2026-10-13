import type { InvalidateEvent } from "@/bindings";
import { qk } from "./queryKeys";
export function invalidationKeys(
  event: InvalidateEvent,
): readonly (readonly string[])[] {
  if (event === "Accounts") return [qk.accounts];
  if (event.GetMessage) return [["message", event.GetMessage.account_id]];
  if (event.ListMessages) return [["inbox", event.ListMessages.account_id]];
  if (event.DraftsChanged)
    return [
      ["drafts", event.DraftsChanged.account_id],
      ["draft", event.DraftsChanged.account_id],
    ];
  const exhaustive: never = event;
  throw new Error(`Unhandled invalidation: ${String(exhaustive)}`);
}

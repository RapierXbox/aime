import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { commands } from "@/bindings";
import { unwrap } from "@/lib/drafts";
import { qk } from "@/lib/queryKeys";
import { syncDraftAccount } from "@/lib/DraftStore";

export default function DraftSync() {
  const accounts = useQuery({
    queryKey: qk.accounts,
    queryFn: () => unwrap(commands.emailListAccounts()),
  });

  useEffect(() => {
    const sync = () => {
      if (document.visibilityState === "hidden" || !navigator.onLine) return;
      for (const a of accounts.data ?? [])
        void syncDraftAccount(a.id).catch(() => {});
    };
    
    sync();

    const timer = setInterval(sync, 60_000);
    window.addEventListener("focus", sync);
    window.addEventListener("online", sync);
    document.addEventListener("visibilitychange", sync);

    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", sync);
      window.removeEventListener("online", sync);
      document.removeEventListener("visibilitychange", sync);
    };
  }, [accounts.data]);

  return null;
}

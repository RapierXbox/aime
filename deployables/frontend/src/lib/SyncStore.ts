import { create } from "zustand";
import { Channel } from "@tauri-apps/api/core";
import { commands, type Progress } from "@/bindings";

// TODO: rename to TaskStore for all asynchronous global tasks

type AccountSyncState = {
  status: "idle" | "syncing" | "ok" | "error";
  progress: number | null;
  statusMessage: string | null;
};

const idleState: AccountSyncState = {
  status: "idle",
  progress: null,
  statusMessage: null,
};

type SyncStore = {
  byAccount: Record<string, AccountSyncState>;
  sync: (accountId: string, full?: boolean) => void;
};

export const useSyncStore = create<SyncStore>()((set, get) => ({
  byAccount: {},

  sync: (accountId, full = false) => {
    const set_ = (patch: Partial<AccountSyncState>) =>
      set((s) => ({
        byAccount: {
          ...s.byAccount,
          [accountId]: { ...(s.byAccount[accountId] ?? idleState), ...patch },
        },
      }));

    if (get().byAccount[accountId]?.status === "syncing") return;

    const chan = new Channel<Progress>();
    chan.onmessage = (p) => {
      if (p.Update) {
        set_({ progress: p.Update.completed / p.Update.out_of });
      } else if (p.Message) {
        set_({ statusMessage: p.Message });
      }
    };

    set_({ status: "syncing", progress: null, statusMessage: null });

    const req = full
      // forcing a full sync is meant for dev only
      ? commands.devEmailFullSync(accountId, chan)
      : commands.emailSync(accountId, chan);

    req.then((res) => {
      set_({
        status: res.status === "ok" ? "ok" : "error",
      });
      setTimeout(() => set_({ status: "idle", progress: null }), 5000);
    });
  },
}));

export const accountSyncState = (
  state: SyncStore,
  accountId: string,
): AccountSyncState => state.byAccount[accountId] ?? idleState;

import type { SettingsSection } from "@/components/page/Settings";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { commands, type MailBox, type Message, type ComposeMode } from "@/bindings";
import { flushAllDrafts, useDraftStore } from "./DraftStore";
import { unwrap } from "./drafts";

type NavigationStoreState = {
  inbox: { accountId: string; inboxId: MailBox } | null;
  // independent of inbox, so switching inbox keeps the panel open
  // ponytail: holds the Message itself, not persisted and not refreshed on invalidation.
  // switch to an id + get_message command once that exists
  rightPanel: { view: "email"; accountId: string; message: Message } | { view: "draft"; accountId: string; draftId: string } | null;
  // open settings section, null when the settings dialog is closed
  settings: SettingsSection | null;
};

type NavigationStoreActions = {
  openInbox: (accountId: string, inboxId: MailBox) => void;
  selectEmail: (accountId: string, message: Message) => Promise<void>;
  selectDraft: (accountId: string, draftId: string) => Promise<void>;
  closePanel: () => Promise<void>;
  compose: (accountId: string, mode: ComposeMode, sourceId: string | null) => Promise<void>;
  openSettings: (section: SettingsSection) => void;
  closeSettings: () => void;
};

type NavigationStore = NavigationStoreState & NavigationStoreActions;

/// stores which inbox is open and whether the settings dialog is open
export const useNavigationStore = create<NavigationStore>()(
  persist(
    (set) => ({
      inbox: null,
      rightPanel: null,
      settings: null,

      openInbox: (accountId, inboxId) =>
        set({ inbox: { accountId, inboxId } }),

      selectEmail: async (accountId, message) => { await flushAllDrafts(); set({ rightPanel: { view: "email", accountId, message } }); },
      selectDraft: async (accountId, draftId) => { await flushAllDrafts(); set({ rightPanel: { view: "draft", accountId, draftId } }); },
      closePanel: async () => { await flushAllDrafts(); set({ rightPanel: null }); },
      compose: async (accountId, mode, sourceId) => {
        await flushAllDrafts();
        const draft = await unwrap(commands.createDraft(accountId, mode, sourceId));
        useDraftStore.getState().adopt(draft);
        set({ rightPanel: { view: "draft", accountId, draftId: draft.id } });
      },

      openSettings: (section) => set({ settings: section }),
      closeSettings: () => set({ settings: null }),
    }),
    {
      name: "nav-storage",
      // settings dialog should not reopen after a restart
      partialize: (state) => ({ inbox: state.inbox }),
    },
  ),
);

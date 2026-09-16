import type { SettingsSection } from "@/components/page/Settings";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { MailBox, Message } from "@/bindings";

type NavigationStoreState = {
  inbox: { accountId: string; inboxId: MailBox } | null;
  // independent of inbox, so switching inbox keeps the panel open
  // ponytail: holds the Message itself, not persisted and not refreshed on invalidation.
  // switch to an id + get_message command once that exists
  rightPanel: { view: "email"; accountId: string; message: Message } | null;
  // open settings section, null when the settings dialog is closed
  settings: SettingsSection | null;
};

type NavigationStoreActions = {
  openInbox: (accountId: string, inboxId: MailBox) => void;
  selectEmail: (accountId: string, message: Message) => void;
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

      selectEmail: (accountId, message) =>
        set({ rightPanel: { view: "email", accountId, message } }),

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

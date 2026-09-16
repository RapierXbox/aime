import type { SettingsSection } from "@/components/page/Settings";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { MailBox } from "@/bindings";

type NavigationStoreState = {
  inbox: { accountId: string; inboxId: MailBox } | null;
  // open settings section, null when the settings dialog is closed
  settings: SettingsSection | null;
};

type NavigationStoreActions = {
  openInbox: (accountId: string, inboxId: MailBox) => void;
  openSettings: (section: SettingsSection) => void;
  closeSettings: () => void;
};

type NavigationStore = NavigationStoreState & NavigationStoreActions;

/// stores which inbox is open and whether the settings dialog is open
export const useNavigationStore = create<NavigationStore>()(
  persist(
    (set) => ({
      inbox: null,
      settings: null,

      openInbox: (accountId, inboxId) =>
        set({ inbox: { accountId, inboxId } }),

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

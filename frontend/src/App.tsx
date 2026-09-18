/* eslint-disable @typescript-eslint/no-unused-vars */
import { Activity, useEffect, useState } from "react";
import "./App.css";

import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { AppSidebar } from "./components/ui/AppSidebar";
import { useNavigationStore } from "./lib/NavigationStore";
import Settings from "./components/page/Settings";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "./components/ui/dialog";
import InboxPage from "./components/page/InboxPage";
import { Separator } from "./components/ui/separator";
import { MessageView } from "./components/page/inbox/MessageView";

import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { commands, events, type MailBox } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { accountSyncState, useSyncStore } from "@/lib/SyncStore";
import { cn } from "@/lib/utils";

function App() {
  // der tanstack query client
  // because the server actively invalidates the cache, we can set a longeer (theoretically infinite)
  // stale time to avoid unnecessary refetches
  const [queryClient, _] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: 10000,
          },
        },
      }),
  );

  // receive invalidate messages from the frontend
  useEffect(() => {
    events.invalidateEvent.listen(ev => {
      if (ev.payload === "Accounts") {
        const getMsgQk = qk.accounts;
        queryClient.invalidateQueries({
          queryKey: [getMsgQk],
        });
      } else if (ev.payload.GetMessage) {
        const [qks, acc_id] = qk.message("", ev.payload.GetMessage.account_id);
        const queryKey = [qks, acc_id];
        queryClient.invalidateQueries({
          queryKey,
        });
      } else if (ev.payload.ListMessages) {
        const [qks, acc_id] = qk.inbox("", ev.payload.ListMessages.account_id);
        const queryKey = [qks, acc_id];

        queryClient.invalidateQueries({
          queryKey,
        });
      } else {
        // force a ts error when this block is non-exhaustive
        const _exhaustiveCheck: never = ev.payload;
        throw new Error(`Unhandled invalidate event: ${_exhaustiveCheck}`);
      }
    })
  });

  // für automatischen dark/light mode
  // TODO: change in settings
  useEffect(() => {
    const appWindow = getCurrentWindow();

    const applyTheme = (theme: string | null) => {
      const isDark =
        theme === "dark" ||
        (theme == null &&
          window.matchMedia("(prefers-color-scheme: dark)").matches);
      document.documentElement.classList.toggle("dark", isDark);
    };

    appWindow.theme().then(applyTheme);
    appWindow.onThemeChanged(({ payload }) => applyTheme(payload));

    // die tanstack query client invalidierung
    // wird im backend in der repo layer durch events ausgelöst
    // listen("invalidate", (ev) => {})
  }, []);

  const inbox = useNavigationStore((it) => it.inbox);
  const settings = useNavigationStore((it) => it.settings);
  const closeSettings = useNavigationStore((it) => it.closeSettings);
  const rightPanel = useNavigationStore((it) => it.rightPanel);

  return (
    <QueryClientProvider client={queryClient}>
      <SidebarProvider className="h-svh">
        <AppSidebar side="left" className="" />
        <SidebarInset className="min-h-0 overflow-hidden">
          <header
            className="flex h-8 shrink-0 items-center gap-2 transition-[width,height]
            ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12
            outline outline-border bg-muted
            "
          >
            <SidebarTrigger
              className="hover:bg-border ml-1"
              color="var(--muted-foreground)"
            />
            {inbox && (
              <InboxHeaderTools
                accountId={inbox.accountId}
                inboxId={inbox.inboxId}
              />
            )}
          </header>
          <div className="flex flex-1 min-h-0">
            {/* persists component state while improving performance, hides the component while settings are open */}
            <Activity mode={settings ? "hidden" : "visible"}>
              {inbox && (
                // key forces a remount when switching inbox, so no state leaks across them
                <InboxPage
                  key={`${inbox.accountId}:${inbox.inboxId}`}
                  accountId={inbox.accountId}
                  inboxId={inbox.inboxId}
                />
              )}
              <Separator orientation="vertical" />
              <div className="flex-1 min-w-0 overflow-y-auto">
                {rightPanel ? (
                  <MessageView
                    accountId={rightPanel.accountId}
                    message={rightPanel.message}
                  />
                ) : (
                  <div className="p-4 text-sm text-muted-foreground">
                    Select an email to view it
                  </div>
                )}
              </div>
            </Activity>
          </div>
        </SidebarInset>
      </SidebarProvider>
      <Dialog open={!!settings} onOpenChange={(open) => !open && closeSettings()}>
        <DialogContent className="flex h-[80vh] p-0 gap-0 rounded-none sm:max-w-3xl">
          <DialogTitle className="sr-only">Settings</DialogTitle>
          {/* Settings throws without a section; content stays mounted during the close animation */}
          {settings && <Settings />}
        </DialogContent>
      </Dialog>
    </QueryClientProvider>
  );
}

// account name + inbox label + sync button, top right of the app header.
// Reuses the qk.accounts query the sidebar already populated, so this is cache-only.
function InboxHeaderTools({
  accountId,
  inboxId,
}: {
  accountId: string;
  inboxId: MailBox;
}) {
  const accounts = useQuery({
    queryKey: qk.accounts,
    queryFn: () =>
      commands.emailListAccounts().then((it) => {
        if (it.status === "error") throw it.error;
        return it.data;
      }),
  });
  const accountName = accounts.data?.find((it) => it.id === accountId)?.name;

  const { status } = useSyncStore((s) => accountSyncState(s, accountId));
  const sync = useSyncStore((s) => s.sync);

  return (
    <div className="ml-auto flex items-center gap-2 mx-2 select-none">
      <span className="font-heading text-muted-foreground">
        {accountName ? `${accountName} — ${inboxId}` : inboxId}
      </span>
      <button
        type="button"
        title={`Sync ${accountName ?? ""}`}
        disabled={status === "syncing"}
        onClick={() => sync(accountId)}
        className="hover:bg-border p-1 cursor-pointer disabled:cursor-default"
      >
        <RefreshCw
          className={cn("size-4 text-muted-foreground", {
            "animate-spin": status === "syncing",
            "text-destructive": status === "error",
          })}
        />
      </button>
    </div>
  );
}

export default App;

/* eslint-disable @typescript-eslint/no-unused-vars */
import { Activity, Suspense, lazy, useEffect, useState } from "react";
import "./App.css";

import { getCurrentWindow } from "@tauri-apps/api/window";
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
import DraftsPage from "./components/page/drafts/DraftsPage";
import DraftSync from "./components/page/drafts/DraftSync";
import { invalidationKeys } from "./lib/invalidation";
import { flushAllDrafts } from "./lib/DraftStore";
import InboxPage from "./components/page/InboxPage";
import { Separator } from "./components/ui/separator";
import { MessageView } from "./components/page/inbox/MessageView";

import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { commands, events, type MailBox } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { accountSyncState, useSyncStore } from "@/lib/SyncStore";
import { cn } from "@/lib/utils";

const DraftComposer = lazy(() => import("./components/page/drafts/DraftComposer"));

function App() {
  // because the server actively invalidates the cache, we can set an infinite
  // stale time to avoid unnecessary refetches, might lead to cache stale issues though
  const [queryClient, _] = useState(
    () =>
      new QueryClient({
        defaultOptions: {
          queries: {
            staleTime: Infinity,
          },
        },
      }),
  );

  // tanstack query client invalidation is handled
  // via events emitted from the rust backend in the repo layer
  useEffect(() => {
    const unlisten = events.invalidateEvent.listen(ev => {
      for (const queryKey of invalidationKeys(ev.payload)) void queryClient.invalidateQueries({ queryKey });
    });

    return () => {
      unlisten.then(f => f());
    };
  }, [queryClient]);

  useEffect(() => {
    const appWindow = getCurrentWindow();
    let closing = false;
    const listener = appWindow.onCloseRequested(async (event) => {
      if (closing) return;
      event.preventDefault();
      try { await flushAllDrafts(); closing = true; await appWindow.close(); }
      catch { closing = false; }
    });
    return () => { void listener.then((unlisten) => unlisten()); };
  }, []);

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
  }, []);

  const inbox = useNavigationStore((it) => it.inbox);
  const settings = useNavigationStore((it) => it.settings);
  const closeSettings = useNavigationStore((it) => it.closeSettings);
  const rightPanel = useNavigationStore((it) => it.rightPanel);

  return (
    <QueryClientProvider client={queryClient}>
      <DraftSync />
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
                inbox.inboxId === "DRAFT" ? <DraftsPage key={inbox.accountId} accountId={inbox.accountId} /> : <InboxPage
                  key={`${inbox.accountId}:${inbox.inboxId}`}
                  accountId={inbox.accountId}
                  inboxId={inbox.inboxId}
                />
              )}
              <Separator orientation="vertical" />
              <div className="flex-1 min-w-0 overflow-y-auto">
                {rightPanel ? (
                  rightPanel.view === "draft" ? <Suspense fallback={<p className="p-4 text-sm">Loading composer…</p>}><DraftComposer key={rightPanel.draftId} accountId={rightPanel.accountId} draftId={rightPanel.draftId} /></Suspense> : <MessageView
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

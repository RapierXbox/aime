/* eslint-disable @typescript-eslint/no-unused-vars */
import { useEffect, useState, type ReactNode } from "react";
import "./App.css";

import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { AppSidebar } from "./components/ui/AppSidebar";
import { Separator } from "./components/ui/separator";
import {
  useNavigationStore,
  selectCurrentPage,
  type Page,
} from "./lib/NavigationStore";
import LoginPage from "./components/page/AimeLogin";
import { ArrowLeft, ArrowRight } from "lucide-react";
import MainPage from "./components/page/MainPage";
import Settings from "./components/page/Settings";
import InboxPage from "./components/page/InboxPage";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

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

  const currentPage = useNavigationStore(selectCurrentPage);
  const navigateBack = useNavigationStore((it) => it.navigateBack);
  const navigateForward = useNavigationStore((it) => it.navigateForward);

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
            {/* Navigation buttons */}
            <div className="flex flex-row items-center flex-1">
              <button
                className="p-2 hover:bg-border rounded-l-md h-6 flex items-center "
                aria-label="Previous page"
                onClick={navigateBack}
              >
                <ArrowLeft color="var(--muted-foreground)" size={16} />
              </button>
              <Separator orientation="vertical" className="h-6" />

              <button
                className="p-2 hover:bg-border rounded-r-md h-6 flex items-center"
                aria-label="Next page"
                onClick={navigateForward}
              >
                <ArrowRight color="var(--muted-foreground)" size={16} />
              </button>
            </div>
            <span
              id="currentPage"
              className="font-heading text-muted-foreground mx-2 select-none ml-auto"
            >
              {currentPage.page}
            </span>
          </header>
          <div className="flex flex-1 min-h-0">{renderPage(currentPage)}</div>
        </SidebarInset>
      </SidebarProvider>
    </QueryClientProvider>
  );
}

/// Renders the component for the current page.
function renderPage(page: Page): ReactNode {
  switch (page.page) {
    case "login":
      return <LoginPage />;
    case "main":
      return <MainPage />;
    case "settings":
      return <Settings />;
    case "inbox":
      // key forces a remount when switching inbox, so no state leaks across them
      return (
        <InboxPage
          key={`${page.accountId}:${page.inboxId}`}
          accountId={page.accountId}
          inboxId={page.inboxId}
        />
      );
  }
}

export default App;

import { useEffect, type ReactNode } from "react";
import "./App.css";

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { AppSidebar } from "./components/AppSidebar";
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

function App() {
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

  const currentPage = useNavigationStore(selectCurrentPage);
  const navigateBack = useNavigationStore((state) => state.navigateBack);
  const navigateForward = useNavigationStore((state) => state.navigateForward);

  return (
    <SidebarProvider>
      <AppSidebar side="left" className="" />
      <SidebarInset className="min-h-0 overflow-hidden">
        <header
          className="flex h-8 shrink-0 items-center gap-2 transition-[width,height]
          ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12
          outline outline-border bg-muted
          "
        >

          <SidebarTrigger className="" color="var(--muted-foreground)" />
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
        <div className="flex flex-1 min-h-0">{pageMap[currentPage.page]()}</div>
      </SidebarInset>
    </SidebarProvider>
  );
}

/// Map of page components by page name
const pageMap: Record<Page["page"], () => ReactNode> = {
  login: () => <LoginPage />,
  main: () => <MainPage />,
  settings: () => <Settings />,
};

export default App;

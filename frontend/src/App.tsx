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
import LoginPage from "./components/page/LoginPage";
import { ArrowLeft, ArrowRight } from "lucide-react";

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
      <SidebarInset>
        <header
          className="flex h-8 shrink-0 items-center gap-2 transition-[width,height]
          ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12
          "
        >
          <div className="flex items-center gap-2 px-4">
            <SidebarTrigger
              className="-ml-1 "
              color="var(--muted-foreground)"
            />
            <div className="flex flex-row items-center">
              <div
                className="p-2  hover:bg-muted rounded-l-md h-6 flex items-center "
                aria-label="Previous page"
                onClick={navigateBack}
              >
                <ArrowLeft color="var(--muted-foreground)" size={16} />
              </div>
              <Separator orientation="vertical" className="h-6" />
              <div
                className="p-2 hover:bg-muted rounded-r-md h-6 flex items-center"
                aria-label="Next page"
                onClick={navigateForward}
              >
                <ArrowRight color="var(--muted-foreground)" size={16} />
              </div>
            </div>
          </div>
        </header>
        <div className="flex flex-1 flex-col gap-4 p-4 pt-0">
          {pageMap[currentPage.page]()}
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}

const pageMap: Record<Page["page"], () => ReactNode> = {
  login: () => <LoginPage />,
  main: () => <div>Main</div>,
  settings: () => <div>Settings</div>,
};

export default App;

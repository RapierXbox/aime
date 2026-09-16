import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { useAuthStore, type AimeAccount } from "@/lib/AuthStore";
import { LogOutIcon, Settings } from "lucide-react";
import { useNavigationStore } from "@/lib/NavigationStore";
import { Button } from "./ui/button";

export function SidebarUser() {
  const aimeAccount: AimeAccount | null = useAuthStore(
    (state) => state.account,
  );

  const navigateTo = useNavigationStore((state) => state.navigateTo);

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <SidebarMenuButton
              size="lg"
              className="data-[state=open]:bg-sidebar-accent data-[state=open]:text-sidebar-accent-foreground
              w-full m-0"
            >
              <div className="h-8 w-8 rounded-full text-center flex bg-muted items-center justify-center outline-border outline">
                {aimeAccount ? aimeAccount?.name.substring(0, 2) : "!"}
              </div>
              <div className="grid flex-1 text-left text-sm leading-tight">
                <span className="truncate font-medium">
                  {aimeAccount?.name ?? "Logged Out"}
                </span>
                <span className="truncate text-xs text-muted-foreground">
                  {aimeAccount?.email ?? "Log in to use aime"}
                </span>
              </div>
            </SidebarMenuButton>
          </DropdownMenuTrigger>
          {/* Dropdown Menu when you click on the user */}
          <DropdownMenuContent
            className="w-fit"
            side="right"
            align="end"
            sideOffset={4}
          >
            <DropdownMenuLabel className="p-0 font-normal">
              <Button
                variant="ghost"
                className="flex items-center gap-2 px-1 py-6 text-left text-sm"
                onClick={() => navigateTo({ page: "login", ephemeral: true })}
              >
                <div
                  className="h-8 w-8 rounded-full text-center flex bg-muted
                  items-center justify-center outline outline-border"
                >
                  {aimeAccount ? aimeAccount?.name.substring(0, 2) : "!"}
                </div>
                <div className="grid flex-1 text-left text-sm leading-tight">
                  <span className="truncate font-medium">
                    {aimeAccount?.name ?? "Logged Out"}
                  </span>
                  <span className="truncate text-xs">{aimeAccount?.email}</span>
                </div>
              </Button>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onClick={() =>
                navigateTo({ page: "settings", section: "application" })
              }
            >
              <Settings />
              Settings
            </DropdownMenuItem>
            {!!aimeAccount && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem>
                  <LogOutIcon />
                  Log out
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}

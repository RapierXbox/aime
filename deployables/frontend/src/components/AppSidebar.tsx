import * as React from "react";
import { SidebarUser } from "@/components/SidebarUser";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarRail,
} from "@/components/ui/sidebar";
import { Separator } from "@/components/ui/separator";
import { useNavigationStore } from "@/lib/NavigationStore";

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const navigateTo = useNavigationStore((state) => state.navigateTo);

  return (
    <Sidebar {...props}>
      <SidebarHeader className="flex">
        <button className=" flex-1 font-heading text-2xl text-right select-none"
          onClick={() => navigateTo({ page: "main" })}>
          Aime
        </button>
      </SidebarHeader>
      <SidebarContent></SidebarContent>
      <Separator />
      <SidebarFooter>
        <SidebarUser />
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}

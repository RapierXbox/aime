import * as React from "react";
import { SidebarUser } from "@/components/SidebarUser";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarRail,
} from "@/components/ui/sidebar";
import { Separator } from "@/components/ui/separator";
import { useQuery } from "@tanstack/react-query";
import { commands } from "@/bindings";
import SidebarEmailInbox from "./SidebarEmailInbox";
import { qk } from "@/lib/queryKeys";

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const email_accs = useQuery({
    queryKey: qk.accounts,
    queryFn: () =>
      commands.emailListAccounts().then((it) => {
        if (it.status === "error") throw it.error;
        return it.data;
      }),
  });

  return (
    <Sidebar {...props}>
      <SidebarHeader className="flex">
        <span className=" flex-1 font-heading text-2xl text-right select-none text-foreground">
          AiMe
        </span>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarMenu>
            {email_accs.data?.map((it) => (
              <SidebarEmailInbox acc={it} key={it.id} />
            ))}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
      <Separator />
      <SidebarFooter>
        <SidebarUser />
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}

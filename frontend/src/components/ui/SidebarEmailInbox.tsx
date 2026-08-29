/**
 * wrap inside a SidebarGroup>SidebarMenu, returns a SidebarMenuItem
 * that is collapsible and expands to show available inboxes for a given account
 */

import { type ListEmailEntry } from "@/bindings";
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "./sidebar";
import { cn, EMAIL_INBOXES } from "@/lib/utils";
import { Button } from "./button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./collapsible";
import { ChevronDown } from "lucide-react";
import { selectCurrentPage, useNavigationStore } from "@/lib/NavigationStore";

const SidebarEmailInbox: React.FC<{
  acc: ListEmailEntry;
}> = ({ acc }) => {
  const navigateTo = useNavigationStore((state) => state.navigateTo);
  const curr = useNavigationStore(selectCurrentPage);

  const active =
    curr.page === "inbox" && curr.accountId === acc.id ? curr.inboxId : null;

  return (
    <Collapsible defaultOpen className="group/collapsible">
      <SidebarGroupLabel asChild>
        <CollapsibleTrigger>
          {acc.name}
          <ChevronDown className="ml-auto transition-transform group-data-[state=open]/collapsible:rotate-180" />
        </CollapsibleTrigger>
      </SidebarGroupLabel>
      <CollapsibleContent>
        <SidebarMenuSub>
          {EMAIL_INBOXES.map((it) => (
            <SidebarMenuSubItem key={it.id}>
              <SidebarMenuSubButton asChild>
                <button
                  type="button"
                  className={cn("w-full cursor-pointer", {
                    "bg-sidebar-accent": active === it.id,
                  })}
                  onClick={() =>
                    navigateTo({
                      page: "inbox",
                      accountId: acc.id,
                      inboxId: it.id,
                    })
                  }
                >
                  <it.icon />
                  {it.name}
                </button>
              </SidebarMenuSubButton>
            </SidebarMenuSubItem>
          ))}
        </SidebarMenuSub>
      </CollapsibleContent>
    </Collapsible>
  );
};

export default SidebarEmailInbox;

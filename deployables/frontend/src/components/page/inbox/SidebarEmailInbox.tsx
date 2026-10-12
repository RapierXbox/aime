/**
 * wrap inside a SidebarGroup>SidebarMenu, returns a SidebarMenuItem
 * that is collapsible and expands to show available inboxes for a given account
 */

import { type ListEmailEntry } from "@/bindings";
import {
  SidebarGroupLabel,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import { cn, EMAIL_INBOXES } from "@/lib/utils";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { ChevronDown } from "lucide-react";
import { useNavigationStore } from "@/lib/NavigationStore";

const SidebarEmailInbox: React.FC<{
  acc: ListEmailEntry;
}> = ({ acc }) => {
  const openInbox = useNavigationStore((state) => state.openInbox);
  const active = useNavigationStore((state) =>
    state.inbox?.accountId === acc.id ? state.inbox.inboxId : null,
  );

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
                  onClick={() => openInbox(acc.id, it.id)}
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

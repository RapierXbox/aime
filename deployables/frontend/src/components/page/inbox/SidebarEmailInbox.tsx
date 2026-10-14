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
import { ChevronDown, PencilLine } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { emailError } from "@/lib/drafts";
import { useNavigationStore } from "@/lib/NavigationStore";

const SidebarEmailInbox: React.FC<{
  acc: ListEmailEntry;
}> = ({ acc }) => {
  const openInbox = useNavigationStore((state) => state.openInbox);
  const active = useNavigationStore((state) =>
    state.inbox?.accountId === acc.id ? state.inbox.inboxId : null,
  );

  const [composeError, setComposeError] = useState<string | null>(null);

  return (
    <Collapsible defaultOpen className="group/collapsible">
      {/* the compose button is a sibling of the trigger: a button inside a button is invalid HTML */}
      <div className="flex items-center">
        <SidebarGroupLabel asChild className="flex-1">
          <CollapsibleTrigger>
            {acc.name}
            <ChevronDown className="ml-auto transition-transform group-data-[state=open]/collapsible:rotate-180" />
          </CollapsibleTrigger>
        </SidebarGroupLabel>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="rounded-none text-sidebar-foreground/70"
          aria-label={`Compose from ${acc.name}`}
          title="Compose"
          onClick={() => {
            setComposeError(null);
            void useNavigationStore
              .getState()
              .compose(acc.id, "New", null)
              .catch((e) => setComposeError(emailError(e)));
          }}
        >
          <PencilLine />
        </Button>
      </div>
      {composeError && (
        <p role="alert" className="px-2 text-xs text-destructive">
          {composeError}
        </p>
      )}
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

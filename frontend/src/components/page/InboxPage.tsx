import { commands, type MailBox, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import React from "react";
import { cn } from "@/lib/utils";
import { useNavigationStore } from "@/lib/NavigationStore";


const InboxPage: React.FC<{
  accountId: string;
  inboxId: MailBox;
}> = ({ accountId, inboxId }) => {
  const selectEmail = useNavigationStore((state) => state.selectEmail);
  const selectedId = useNavigationStore((state) =>
    state.rightPanel?.accountId === accountId
      ? state.rightPanel.message.provider_msg_id
      : null,
  );

  const fetchMessages = async ({ pageParam = 0 }) => {
    const res = await commands.listMessages(accountId, inboxId, pageParam);
    if (res.status === "ok") {
      return res.data;
    } else {
      throw res.error;
    }
  };

  // query the list of mail
  const q = useInfiniteQuery({
    queryKey: qk.inbox(inboxId, accountId),
    queryFn: fetchMessages,
    getNextPageParam: (lastPage, _) => {
      return lastPage.next_page_param;
    },

    initialPageParam: 0,
  });

  return (
    <div className="w-[20rem] space-y-2 divide-y divide-accent overflow-y-scroll h-full">
      {q.data?.pages?.map((group, i) => (
        <React.Fragment key={i}>
          {group.messages.map((msg) => (
            <EmailPreview
              key={msg.provider_msg_id}
              message={msg}
              active={msg.provider_msg_id === selectedId}
              onSelect={() => selectEmail(accountId, msg)}
            />
          ))}
        </React.Fragment>
      ))}
    </div>
  );
};

// Claude --
// MM:HH for recent messages, DD.MM.YY for older ones
const formatDateHeader = (d: Date) =>
  Date.now() - d.getTime() < 864e5
    ? d.toLocaleTimeString("de-DE", { timeStyle: "short" })
    : d.toLocaleDateString("de-DE", { dateStyle: "short" });

// "Name" <a@b> | Name <a@b> | a@b
const displayName = (addr: string) =>
  addr.match(/^\s*"?(.*?)"?\s*<.*>\s*$/)?.[1] || addr;

// --

const EmailPreview: React.FC<{
  message: Message;
  active: boolean;
  onSelect: () => void;
}> = ({ message, active, onSelect }) => {
  const parsed = message.date_header && new Date(message.date_header);

  return (
    <div
      className={cn("px-2 py-1 cursor-pointer", {
        "bg-sidebar-accent": active,
      })}
      onClick={onSelect}
    >
      <div className="flex">
        <p className="font-semibold text-sm line-clamp-1">
          {displayName(message.from_addr)}
        </p>
        <p className="text-xs ml-auto text-muted-foreground">
          {formatDateHeader(parsed)}
        </p>
      </div>

      <p className="text-xs line-clamp-1">{message.subject}</p>
      <p className="text-xxs line-clamp-2 text-muted-foreground">
        {message.snippet}
      </p>
    </div>
  );
};

export default InboxPage;

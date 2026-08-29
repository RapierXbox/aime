import { commands, type MailBox, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Separator } from "../ui/separator";
import React from "react";


const InboxPage: React.FC<{
  accountId: string;
  inboxId: MailBox;
}> = ({ accountId, inboxId }) => {
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
    <div className="flex flex-1 flex-row min-h-0">
      <div className="w-[20rem] space-y-2 divide-y divide-accent overflow-y-scroll h-full">
        {q.data?.pages?.map((group, i) => (
          <React.Fragment key={i}>
            {group.messages.map((msg) => (
              <EmailPreview key={msg.provider_msg_id} message={msg} />
            ))}
          </React.Fragment>
        ))}
      </div>
      <Separator orientation="vertical" />
      <div className="flex-1 overflow-y-auto">
        Inbox {inboxId} of {accountId}
      </div>
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
}> = ({ message }) => {
  const parsed = message.date_header && new Date(message.date_header);

  return (
    <div className="px-2 py-1">
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

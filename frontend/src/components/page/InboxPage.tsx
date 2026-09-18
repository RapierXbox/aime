import { commands, type MailBox, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import React from "react";
import { cn } from "@/lib/utils";
import { useNavigationStore } from "@/lib/NavigationStore";
import { accountSyncState, useSyncStore } from "@/lib/SyncStore";
import { Progress as ProgressBar } from "radix-ui";


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

  // load the next page once the bottom of the list scrolls into view
  const sentinel = React.useRef<HTMLDivElement>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = q;
  React.useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const io = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting && hasNextPage && !isFetchingNextPage) {
        fetchNextPage();
      }
    });
    io.observe(el);
    return () => io.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  return (
    <div className="w-[20rem] flex flex-col min-h-0">
      <SyncProgress accountId={accountId} />
      <div className="flex-1 pt-2 space-y-2 divide-y divide-accent overflow-y-scroll">
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
        <div ref={sentinel} />
      </div>
    </div>
  );
};

// sync status strip above the list; renders nothing while idle
const SyncProgress: React.FC<{ accountId: string }> = ({ accountId }) => {
  const { progress, statusMessage } = useSyncStore((s) =>
    accountSyncState(s, accountId),
  );

  if (progress === null) return null;

  return (
    <div className="shrink-0 flex flex-col gap-1 px-2 pt-2">
      {statusMessage !== null && (
        <span className="text-xxs text-shimmer">{statusMessage}</span>
      )}
      <ProgressBar.Root
        value={progress * 100}
        className="w-full h-1 bg-muted overflow-hidden"
      >
        <ProgressBar.Indicator
          className="h-full bg-primary transition-transform"
          style={{ transform: `translateX(-${100 - progress * 100}%)` }}
        />
      </ProgressBar.Root>
    </div>
  );
};

// MM:HH for recent messages, DD.MM.YY for older ones
const formatDateHeader = (d: Date) =>
  Date.now() - d.getTime() < 864e5
    ? d.toLocaleTimeString("de-DE", { timeStyle: "short" })
    : d.toLocaleDateString("de-DE", { dateStyle: "short" });

// "Name" <a@b> | Name <a@b> | a@b
const displayName = (addr: string) =>
  addr.match(/^\s*"?(.*?)"?\s*<.*>\s*$/)?.[1] || addr;

const EmailPreview: React.FC<{
  message: Message;
  active: boolean;
  onSelect: () => void;
}> = ({ message, active, onSelect }) => {
  const parsed = message.date_header && new Date(message.date_header);

  // ponytail: fixed 72px row, h-18 (py-1 8 + text-sm 20 + text-xs 16 + 2x text-xxs 28).
  // Keep in sync with the type scale; lets a virtualizer use a plain estimateSize.
  return (
    <div
      className={cn("px-2 py-1 h-18 overflow-hidden cursor-pointer", {
        "bg-sidebar-accent": active,
      })}
      onClick={onSelect}
    >
      <div className="flex">
        <p className="font-semibold text-sm line-clamp-1 select-none">
          {displayName(message.from_addr)}
        </p>
        <p className="text-xs ml-auto text-muted-foreground select-none">
          {formatDateHeader(parsed)}
        </p>
      </div>

      <p className="text-xs line-clamp-1 select-none">{message.subject}</p>
      <p className="text-xxs line-clamp-2 text-muted-foreground select-none">
        {message.snippet}
      </p>
    </div>
  );
};

export default InboxPage;

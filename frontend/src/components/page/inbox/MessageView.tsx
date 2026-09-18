import { commands, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useQuery } from "@tanstack/react-query";
import React from "react";
import DOMPurify from "dompurify";

export const MessageViewHeader: React.FC<{ message: Message }> =
  ({ message }) => {
    return (
      <div className="p-4 space-y-1 border-b border-border">
        <p className="text-sm font-semibold">{message.subject}</p>
        <p className="text-xs text-muted-foreground">
          From: {message.from_addr}
        </p>
        {message.to_addrs && (
          <p className="text-xs text-muted-foreground">
            To: {message.to_addrs}
          </p>
        )}
        {message.cc_addrs && (
          <p className="text-xs text-muted-foreground">
            Cc: {message.cc_addrs}
          </p>
        )}
        {message.date_header && (
          <p className="text-xs text-muted-foreground">
            {new Date(message.date_header).toLocaleString("de-DE")}
          </p>
        )}
      </div>
    );
  }
  ;

const pickBody = (contents: Message["contents"]) =>
  contents.find((c) => c.mime_type === "text/html") ??
  contents.find((c) => c.mime_type === "text/plain") ??
  null;

// ponytail: stub, wire to tauri-plugin-opener (openUrl) once external links are wanted
const openExternal = (href: string) => console.info("link click blocked:", href);

// email links must never navigate the webview; left and middle clicks both land here
const blockLinks = (e: React.MouseEvent) => {
  const a = (e.target as Element).closest("a");
  if (!a) return;
  e.preventDefault();
  openExternal(a.href);
};

// headers come from the list row in the nav store, so they paint immediately;
// only the bodies are fetched here
export const MessageView: React.FC<{ accountId: string; message: Message }> =
  React.memo(({ accountId, message }) => {
    const msgId = message.provider_msg_id;

    const q = useQuery({
      queryKey: qk.message(accountId, msgId ?? ""),
      queryFn: async () => {
        const res = await commands.getMessage(accountId, msgId!);
        if (res.status === "error") throw res.error;
        return res.data;
      },
      enabled: msgId !== null,
    });

    const body = q.data ? pickBody(q.data.contents) : null;

    return (
      <div className="min-w-0">
        <MessageViewHeader message={message} />
        {/* wide email HTML scrolls here, so the header stays put */}
        <div className="p-4 overflow-x-auto">
          {msgId === null ? null : q.isPending ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : q.isError ? (
            <p className="text-sm text-destructive">Failed to load message</p>
          ) : body === null ? (
            // a skeleton row is listed but not hydrated yet: not an error
            <p className="text-sm text-muted-foreground">
              Message not found on disk
            </p>
          ) : body.mime_type === "text/html" ? (
            <div
              className="text-sm"
              onClick={blockLinks}
              onAuxClick={blockLinks}
              // remote images not blocked yet, add when attachment/cid handling lands
              dangerouslySetInnerHTML={{
                __html: DOMPurify.sanitize(body.body),
              }}
            />
          ) : (
            <div className="text-sm whitespace-pre-wrap">{body.body}</div>
          )}
        </div>
      </div>
    );
  });

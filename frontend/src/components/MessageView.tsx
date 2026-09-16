import type { Message } from "@/bindings";
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

export const MessageView: React.FC<{ message: Message }> = React.memo(
  ({ message }) => {
    console.log(message.contents);
    const body = pickBody(message.contents);

    return (
      <div>
        <MessageViewHeader message={message} />
        <div className="p-4">
          {body === null ? null : body.mime_type === "text/html" ? (
            <div
              className="text-sm"
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
  },
);

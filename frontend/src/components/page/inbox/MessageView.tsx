import { commands, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useQuery } from "@tanstack/react-query";
import React from "react";
import DOMPurify from "dompurify";
import { DevOnly } from "@/components/dev/DevOnly";

// email links must never navigate anywhere yet, so strip hrefs at sanitize time
// rather than trying to intercept clicks across the iframe boundary
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.removeAttribute("href");
    node.removeAttribute("target");
  }
});

export const MessageViewHeader: React.FC<{ message: Message, accountId: string }> =
  ({ message, accountId }) => {
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
        <DevOnly><DevEmailHeader accountId={accountId} message={message} /></DevOnly>
      </div>
    );
  }
  ;

const pickBody = (contents: Message["contents"]) =>
  contents.find((c) => c.mime_type === "text/html") ??
  contents.find((c) => c.mime_type === "text/plain") ??
  null;

// an isolated document has no app CSS to inherit, so the browser default
// (serif) shows through for mail that doesn't set its own font; this matches
// the app's sans-serif default without leaking further into the email's cascade
const DEFAULT_BODY_STYLE =
  "font-family: ui-sans-serif, system-ui, sans-serif; font-size: 14px;";

// isolates email HTML in its own document so <style>/font-family rules can't
// leak into the app chrome; sandboxed with no scripts, no same-origin access.
// fills the available space and scrolls internally instead of resizing to content
const EmailBodyFrame: React.FC<{ html: string }> = React.memo(({ html }) => (
  <iframe
    sandbox="allow-same-origin"
    className="w-full h-full border-0"
    srcDoc={`<style>body{${DEFAULT_BODY_STYLE}}</style>${DOMPurify.sanitize(html)}`}
  />
));

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
      <div className="min-w-0 h-full flex flex-col">
        <MessageViewHeader message={message} accountId={accountId} />

        {/* wide email HTML scrolls here, so the header stays put */}
        <div className="p-4 flex-1 min-h-0 overflow-auto">
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
            // remote images not blocked yet, add when attachment/cid handling lands
            <EmailBodyFrame html={body.body} />
          ) : (
            <div className="text-sm whitespace-pre-wrap">{body.body}</div>
          )}
        </div>
      </div>
    );
  });



const DevEmailHeader: React.FC<{ accountId: string, message: Message }> = ({ accountId, message }) => {
  return (<div className="text-xs text-muted-foreground font-mono">
    {Object.keys(message).map(it => (<>{it}: {message[it] as string}<br></br></>))
    }
  </div >);
};
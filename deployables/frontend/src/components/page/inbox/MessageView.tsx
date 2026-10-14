import { commands, type Message } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useQuery } from "@tanstack/react-query";
import React, { useState } from "react";
import { useNavigationStore } from "@/lib/NavigationStore";
import { emailError } from "@/lib/drafts";
import { Button } from "@/components/ui/button";
import DOMPurify from "dompurify";
import { DevOnly } from "@/components/dev/DevOnly";
import { supportsDarkMode } from "@/lib/emailColorScheme";
import { Forward, Reply, ReplyAll } from "lucide-react";

// email links must never navigate anywhere yet, so strip hrefs at sanitize time
// rather than trying to intercept clicks across the iframe boundary
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.removeAttribute("href");
    node.removeAttribute("target");
  }
});

export const MessageViewHeader: React.FC<{ message: Message, accountId: string }> =
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
        <DevOnly><DevEmailHeader message={message} /></DevOnly>
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
// fills the available space and scrolls internally, or with `fit` grows to its
// content height so it scrolls with the surrounding page
export const EmailBodyFrame: React.FC<{ html: string; fit?: boolean }> = React.memo(({ html, fit }) => {
  // dark-safe mail follows the app theme on a transparent canvas; everything else
  // is pinned to light on white paper, since its colors assume a white background
  const scheme = supportsDarkMode(html) ? "light dark" : "light";
  return <iframe
    sandbox="allow-same-origin"
    className="w-full h-full border-0"
    // must match the document's scheme, or the browser paints an opaque canvas behind it
    style={{ colorScheme: scheme }}
    // fit: flow-root keeps child margins/floats inside body so offsetHeight is the full
    // content height; vertical overflow hidden so only wide content scrolls (sideways) here.
    // WHOLE_DOCUMENT keeps <html>/<body> attributes: mail often hangs its dark-mode
    // class and background on <body>, and a fragment sanitize drops them
    srcDoc={`<style>:root{color-scheme:${scheme}${scheme === "light" ? ";background:#fff" : ""}}body{${DEFAULT_BODY_STYLE}}::-webkit-scrollbar{width:8px;height:8px}::-webkit-scrollbar-track,::-webkit-scrollbar-corner{background:transparent}::-webkit-scrollbar-thumb{background-color:rgba(128,128,128,.45);background-clip:content-box;border:2px solid transparent;border-radius:4px}${fit ? "html{overflow-x:auto;overflow-y:hidden}body{display:flow-root}" : ""}</style>${DOMPurify.sanitize(html, { WHOLE_DOCUMENT: true })}`}
    onLoad={fit ? (e) => {
      const frame = e.currentTarget;
      const root = frame.contentDocument!.documentElement;
      // re-fit when a width change reflows the content
      new ResizeObserver(() => { frame.style.height = `${root.offsetHeight}px`; }).observe(root);
    } : undefined}
  />;
});

// headers come from the list row in the nav store, so they paint immediately;
// only the bodies are fetched here
export const MessageView: React.FC<{ accountId: string; message: Message }> =
  React.memo(({ accountId, message }) => {
    const msgId = message.provider_msg_id ?? null;

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
        <MessageViewHeader message={q.data ?? message} accountId={accountId} />
        <ReplyActions accountId={accountId} messageId={msgId} />

        {/* wide email HTML scrolls here, so the header stays put */}
        <div className="p-4 pt-4 flex-1 min-h-0 overflow-auto">
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



const DevEmailHeader: React.FC<{ message: Message }> = ({ message }) => {
  return (<pre className="text-xs text-muted-foreground font-mono">
    {JSON.stringify(message, null, 2)}
  </pre >);
};
function ReplyActions({ accountId, messageId }: { accountId: string; messageId: string | null }) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // zero-height row: the pill straddles the header/body border
  return <div className="relative h-0 z-10">
    <div className="absolute left-1/2 -translate-x-1/2 -translate-y-1/2 flex flex-col items-center gap-1">
      <div className="flex rounded-full border border-border bg-background overflow-hidden">
        {(["Reply", "ReplyAll", "Forward"] as const).map((mode) => {
          const Icon = { Reply, ReplyAll, Forward }[mode];
          const label = mode === "ReplyAll" ? "Reply all" : mode;
          return <Button key={mode} className="rounded-none" variant="ghost" size="icon-sm" aria-label={label} title={label} disabled={busy || !messageId} onClick={() => { setBusy(true); setError(null); void useNavigationStore.getState().compose(accountId, mode, messageId).catch((e) => setError(emailError(e))).finally(() => setBusy(false)); }}><Icon /></Button>;
        })}
      </div>
      {error && <p role="alert" className="text-sm text-destructive bg-background px-2">{error}</p>}
    </div>
  </div>;
}

import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { commands, type ConflictChoice } from "@/bindings";
import { qk } from "@/lib/queryKeys";
import { useDraftStore, flushDraft, syncDraftAccount } from "@/lib/DraftStore";
import { editable, emailError, unwrap } from "@/lib/drafts";
import { useNavigationStore } from "@/lib/NavigationStore";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DevOnly } from "@/components/dev/DevOnly";
import { EmailBodyFrame } from "@/components/page/inbox/MessageView";
import { RichTextEditor } from "./RichTextEditor";

const BODY_KEYS = ["body_text", "body_html", "quote_html"];
// JSON.stringify replacer for the dev dump: it shows draft metadata only, so
// message content never lands in screenshots or bug reports.
const hideBodies = (key: string, value: unknown) =>
  BODY_KEYS.includes(key) && typeof value === "string"
    ? `<${value.length} chars>`
    : value;

export default function DraftComposer({
  draftId,
  accountId,
}: {
  draftId: string;
  accountId: string;
}) {
  const query = useQuery({
    queryKey: qk.draft(accountId, draftId),
    queryFn: () => unwrap(commands.getDraft(draftId)),
  });
  const syncError = useDraftStore((s) => s.syncErrors[accountId]);
  const syncing = useDraftStore((s) => s.syncing[accountId]);
  const buffer = useDraftStore((s) => s.buffers[draftId]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const accounts = useQuery({
    queryKey: qk.accounts,
    queryFn: () => unwrap(commands.emailListAccounts()),
  });
  const name =
    accounts.data?.find((a) => a.id === accountId)?.name ?? accountId;
  useEffect(() => {
    if (query.data) useDraftStore.getState().adopt(query.data);
  }, [query.data]);
  useEffect(() => {
    void syncDraftAccount(accountId).catch((e) => setError(emailError(e)));
  }, [accountId, draftId]);
  async function action(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (e) {
      setError(emailError(e));
      try {
        useDraftStore
          .getState()
          .adopt(await unwrap(commands.getDraft(draftId)));
      } catch {
        /* Keep buffer. */
      }
    } finally {
      setBusy(false);
    }
  }
  if (!buffer)
    return (
      <p className="p-4 text-sm" role={query.isError ? "alert" : undefined}>
        {query.isError ? emailError(query.error) : "Loading draft…"}
      </p>
    );
  const d = buffer.draft;
  const locked = !editable(d) || busy;
  const conflict =
    buffer.remoteChanged || d.status === "Conflict" || d.status === "Missing";
  const status = buffer.saving
    ? "Saving locally…"
    : syncing
      ? "Syncing to Gmail…"
      : buffer.dirty
        ? "Unsaved local edits"
        : d.status === "Synced"
          ? "Saved locally · Synced to Gmail"
          : d.status === "Local"
            ? !d.remote && !d.fields.body_text.trim()
              ? "Saved locally" // untouched drafts are never uploaded
              : "Saved locally · Waiting for Gmail sync"
            : d.status === "Unknown"
              ? "Gmail status unknown"
              : d.status;
  const change = (field: "to" | "cc" | "bcc" | "subject", value: string) =>
    useDraftStore.getState().change(draftId, { [field]: value });
  const resolve = async (choice: ConflictChoice) => {
    if (choice === "UseGmail" && d.status !== "Conflict") {
      useDraftStore
        .getState()
        .discardBuffer(draftId, await unwrap(commands.getDraft(draftId)));
      return;
    }
    const resolved = await unwrap(
      commands.resolveDraftConflict(
        d.id,
        d.revision,
        choice,
        choice === "KeepLocalAsCopy" ? buffer.fields : null,
      ),
    );
    useDraftStore
      .getState()
      .discardBuffer(d.id, await unwrap(commands.getDraft(d.id)));
    useDraftStore.getState().adopt(resolved);
    await useNavigationStore.getState().selectDraft(accountId, resolved.id);
  };
  return (
    <section
      className="h-full flex flex-col min-w-0"
      aria-label="Email composer"
    >
      <header className="p-3 border-b border-border flex items-center gap-2 flex-wrap">
        <strong className="text-sm">From: {name}</strong>
        <span role="status" className="ml-auto text-xs text-muted-foreground">
          {status}
        </span>
        <Button
          variant="ghost"
          className="rounded-none"
          disabled={busy}
          onClick={() =>
            void action(() => useNavigationStore.getState().closePanel())
          }
        >
          Close
        </Button>
      </header>
      {(error || buffer.error || syncError) && (
        <p role="alert" className="p-3 text-sm text-destructive">
          {error || buffer.error || syncError}
        </p>
      )}
      {d.status === "Local" && d.issues.length > 0 && (
        <p role="status" className="p-3 text-xs border-b border-border">
          Not uploaded to Gmail: {d.issues.join("; ")}. Fix it to sync.
        </p>
      )}
      {d.status === "Unsupported" && (
        <div className="p-3 text-sm border-b border-border">
          <p>
            This Gmail draft is read-only: {d.issues.join("; ")}. Edit or send
            it in Gmail to preserve its content.
          </p>
          <Button
            className="rounded-none mt-2"
            onClick={() =>
              void action(() => unwrap(commands.openGmail(accountId, false)))
            }
          >
            Open in Gmail
          </Button>
        </div>
      )}
      {conflict && (
        <div className="p-3 border-b border-border text-sm">
          <p>
            {d.status === "Missing"
              ? "The Gmail draft disappeared. Your local content is retained."
              : "The saved draft changed. Both versions are preserved."}
          </p>
          {d.conflict && (
            <details className="my-2">
              <summary>Gmail version</summary>
              <p>To: {d.conflict.fields.to}</p>
              <p>Subject: {d.conflict.fields.subject}</p>
              <pre className="whitespace-pre-wrap text-xs">
                {d.conflict.fields.body_text}
              </pre>
            </details>
          )}
          <div className="flex gap-2 mt-2">
            {d.status !== "Missing" && (
              <Button
                className="rounded-none"
                disabled={busy}
                onClick={() => void action(() => resolve("UseGmail"))}
              >
                {d.status === "Conflict"
                  ? "Use Gmail version"
                  : "Load saved version (discard edits)"}
              </Button>
            )}
            <Button
              variant="outline"
              className="rounded-none"
              disabled={busy}
              onClick={() => void action(() => resolve("KeepLocalAsCopy"))}
            >
              Keep mine as a separate draft
            </Button>
          </div>
        </div>
      )}
      {d.status === "Unknown" && (
        <div className="p-3 border-b border-border text-sm">
          <p>
            The request may have reached Gmail. It will not be retried
            automatically.
          </p>
          <div className="flex gap-2 my-2">
            <Button
              className="rounded-none"
              disabled={busy}
              onClick={() => void action(() => syncDraftAccount(accountId))}
            >
              Check again
            </Button>
            <Button
              variant="outline"
              className="rounded-none"
              onClick={() =>
                void action(() =>
                  unwrap(
                    commands.openGmail(accountId, d.attempt_kind === "send"),
                  ),
                )
              }
            >
              Open {d.attempt_kind === "send" ? "Sent" : "Drafts"} in Gmail
            </Button>
          </div>
          {d.attempt_kind === "send" && d.attempt_id && (
            <>
              <label className="flex gap-2 items-start">
                <input
                  type="checkbox"
                  checked={acknowledged}
                  onChange={(e) => setAcknowledged(e.target.checked)}
                />
                I checked Gmail. Another send may duplicate delivery.
              </label>
              <div className="flex gap-2 mt-2">
                <Button
                  className="rounded-none"
                  disabled={busy || !acknowledged}
                  onClick={() =>
                    void action(async () => {
                      const result = await unwrap(
                        commands.resolveSendAttempt(
                          d.attempt_id!,
                          "AllowManualRetry",
                        ),
                      );
                      useDraftStore.getState().discardBuffer(d.id, result);
                      setAcknowledged(false);
                    })
                  }
                >
                  Return to editing
                </Button>
                <Button
                  variant="outline"
                  className="rounded-none"
                  disabled={busy}
                  onClick={() =>
                    void action(async () => {
                      useDraftStore
                        .getState()
                        .adopt(
                          await unwrap(
                            commands.resolveSendAttempt(
                              d.attempt_id!,
                              "ConfirmSent",
                            ),
                          ),
                        );
                    })
                  }
                >
                  I confirmed it was sent
                </Button>
              </div>
            </>
          )}
        </div>
      )}
      {d.status === "Sent" ? (
        <p className="p-4 text-sm">Sent. Gmail accepted the message.</p>
      ) : (
        <>
          <div className="px-4 py-2 space-y-2 border-b border-border">
            {(["to", "cc", "bcc", "subject"] as const).map((key) => (
              <label key={key} className="flex items-center gap-3 text-sm">
                <span className="w-14">
                  {key === "subject"
                    ? "Subject"
                    : key[0].toUpperCase() + key.slice(1)}
                </span>
                <Input
                  className="rounded-none"
                  aria-label={
                    key === "subject"
                      ? "Subject"
                      : key[0].toUpperCase() + key.slice(1)
                  }
                  value={buffer.fields[key]}
                  disabled={locked}
                  onChange={(e) => change(key, e.target.value)}
                />
              </label>
            ))}
          </div>
          <div className="flex-1 overflow-auto">
            <RichTextEditor
              key={`${d.id}:${buffer.editorVersion}`}
              initialHtml={buffer.fields.body_html}
              readOnly={locked}
              onChange={(body) =>
                useDraftStore.getState().change(d.id, body)
              }
            />
            {buffer.fields.quote_html && (
              <details open className="border-t border-border text-sm">
                <summary className="px-4 py-2 cursor-pointer select-none text-muted-foreground">
                  Quoted original
                </summary>
                {/* the quote is only ever shown inside the sandboxed frame */}
                <EmailBodyFrame html={buffer.fields.quote_html} fit />
              </details>
            )}
          </div>
          <footer className="p-3 border-t border-border flex gap-2">
            <Button
              className="rounded-none"
              disabled={locked || conflict}
              onClick={() =>
                void action(async () => {
                  if (!navigator.onLine) throw "EmailNetworkError";
                  const saved = await flushDraft(d.id);
                  const sent = await unwrap(
                    commands.sendDraft(saved.id, saved.revision),
                  );
                  useDraftStore.getState().discardBuffer(d.id, sent);
                })
              }
            >
              Send
            </Button>
            <Button
              variant="outline"
              className="rounded-none"
              disabled={locked || conflict}
              onClick={() =>
                void action(async () => {
                  await flushDraft(d.id);
                  await syncDraftAccount(accountId);
                })
              }
            >
              Save & sync
            </Button>
            <Button
              variant="ghost"
              className="rounded-none ml-auto"
              disabled={locked || conflict}
              onClick={() =>
                void action(async () => {
                  const saved = await flushDraft(d.id);
                  await unwrap(commands.deleteDraft(saved.id, saved.revision));
                  await useNavigationStore.getState().closePanel();
                })
              }
            >
              Discard draft
            </Button>
          </footer>
        </>
      )}
      <DevOnly>
        <pre className="text-xs whitespace-pre-wrap">
          {JSON.stringify(
            {
              draft: d,
              buffer: {
                dirty: buffer.dirty,
                generation: buffer.generation,
                saving: buffer.saving,
                remoteChanged: buffer.remoteChanged,
                editorVersion: buffer.editorVersion,
                error: buffer.error,
              },
            },
            hideBodies,
            2,
          )}
        </pre>
      </DevOnly>
    </section>
  );
}

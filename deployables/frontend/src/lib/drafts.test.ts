import { describe, expect, it, vi, beforeEach } from "vitest";
import { commands, type Draft } from "@/bindings";
import { draftTools } from "./drafts";
import {
  useDraftStore,
  flushDraft,
  syncDraftAccount,
} from "./DraftStore";
import { invalidationKeys } from "./invalidation";
import { valueToText } from "./draftText";
vi.mock("@/bindings", () => ({
  commands: {
    saveDraft: vi.fn(),
    getDraft: vi.fn(),
    syncDrafts: vi.fn(),
    sendDraft: vi.fn(),
    createDraft: vi.fn(),
    listDrafts: vi.fn(),
  },
}));
const fixture = (overrides: Partial<Draft> = {}): Draft => ({
  id: "draft",
  account_id: "1",
  fields: {
    to: "you@example.com",
    cc: "",
    bcc: "",
    subject: "Hello",
    body_text: "Original",
    body_html: "<div>Original</div>",
    quote_html: "",
  },
  revision: 1,
  acknowledged_revision: 0,
  status: "Local",
  remote: null,
  conflict: null,
  issues: [],
  thread_id: null,
  in_reply_to: null,
  references: null,
  attempt_id: null,
  attempt_kind: null,
  sent_message_id: null,
  ...overrides,
});
beforeEach(() => {
  vi.clearAllMocks();
  useDraftStore.setState({ buffers: {}, syncing: {}, syncErrors: {} });
});
describe("draft integrity", () => {
  it("preserves paragraphs and blank lines", () => {
    const blocks = ["First", "", "Third", ""].map((text) => ({
      type: "p",
      children: [{ text }],
    }));
    expect(valueToText(blocks)).toBe("First\n\nThird\n");
  });
  it("adopts a new reply with its quote as a clean buffer", () => {
    const quote = '<div class="gmail_quote">older</div>';
    useDraftStore
      .getState()
      .adopt(fixture({ fields: { ...fixture().fields, quote_html: quote } }));
    const b = useDraftStore.getState().buffers.draft;
    expect(b.dirty).toBe(false);
    expect(b.fields.quote_html).toBe(quote);
  });
  it("does not expose send tools", () => {
    expect(Object.keys(draftTools)).toEqual(["list", "read", "create", "edit"]);
  });
  it("invalidates the correct account prefixes", () => {
    expect(invalidationKeys("Accounts")).toEqual([["accounts"]]);
    expect(invalidationKeys({ GetMessage: { account_id: "2" } })).toEqual([
      ["message", "2"],
    ]);
    expect(invalidationKeys({ DraftsChanged: { account_id: "2" } })).toEqual([
      ["drafts", "2"],
      ["draft", "2"],
    ]);
  });
  it("keeps dirty fields on remote changes and blocks overwrite", async () => {
    useDraftStore.getState().adopt(fixture());
    useDraftStore
      .getState()
      .change("draft", { body_text: "My unsaved version" });
    useDraftStore
      .getState()
      .adopt(
        fixture({
          revision: 2,
          fields: { ...fixture().fields, body_text: "Outside version" },
        }),
      );
    expect(useDraftStore.getState().buffers.draft.fields.body_text).toBe(
      "My unsaved version",
    );
    await expect(flushDraft("draft")).rejects.toThrow();
    expect(commands.saveDraft).not.toHaveBeenCalled();
  });
  it("keeps the buffer when a local write fails", async () => {
    useDraftStore.getState().adopt(fixture());
    useDraftStore.getState().change("draft", { subject: "Edited" });
    vi.mocked(commands.saveDraft).mockResolvedValue({
      status: "error",
      error: "DraftRevisionConflict",
    });
    vi.mocked(commands.getDraft).mockResolvedValue({
      status: "ok",
      data: fixture(),
    });
    await expect(flushDraft("draft")).rejects.toBe("DraftRevisionConflict");
    const b = useDraftStore.getState().buffers.draft;
    expect(b.dirty).toBe(true);
    expect(b.fields.subject).toBe("Edited");
    expect(b.error).toContain("changed");
  });
  it("does not mistake its own save event for an outside edit", async () => {
    const d = fixture();
    useDraftStore.getState().adopt(d);
    useDraftStore.getState().change(d.id, { subject: "Saved" });
    const saved = fixture({
      revision: 2,
      fields: { ...d.fields, subject: "Saved" },
    });
    vi.mocked(commands.saveDraft).mockImplementation(async () => {
      useDraftStore.getState().adopt(saved);
      return { status: "ok", data: saved };
    });
    await flushDraft(d.id);
    const b = useDraftStore.getState().buffers[d.id];
    expect(b.remoteChanged).toBe(false);
    expect(b.dirty).toBe(false);
  });
  it("keeps the editor mounted when its own save comes back from sync", async () => {
    const d = fixture();
    useDraftStore.getState().adopt(d);
    const version = useDraftStore.getState().buffers[d.id].editorVersion;
    const body = { body_html: "<div><strong>Edited</strong></div>", body_text: "Edited" };
    useDraftStore.getState().change(d.id, body);
    const saved = fixture({ revision: 2, fields: { ...d.fields, ...body } });
    vi.mocked(commands.saveDraft).mockResolvedValue({ status: "ok", data: saved });
    await flushDraft(d.id);
    useDraftStore.getState().adopt({ ...saved, revision: 4, status: "Synced" });
    const b = useDraftStore.getState().buffers[d.id];
    expect(b.editorVersion).toBe(version);
    expect(b.fields.body_html).toBe(body.body_html);
    expect(b.draft.status).toBe("Synced");
  });
  it("serializes saves and flushes edits made during a save", async () => {
    const d = fixture();
    useDraftStore.getState().adopt(d);
    useDraftStore.getState().change(d.id, { subject: "First" });
    let finish!: (result: { status: "ok"; data: Draft }) => void;
    vi.mocked(commands.saveDraft)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      )
      .mockImplementation(async (_id, revision, fields) => ({
        status: "ok",
        data: fixture({ revision: revision + 1, fields }),
      }));
    const work = flushDraft(d.id);
    useDraftStore.getState().change(d.id, { subject: "Second" });
    finish({
      status: "ok",
      data: fixture({ revision: 2, fields: { ...d.fields, subject: "First" } }),
    });
    await work;
    expect(commands.saveDraft).toHaveBeenCalledTimes(2);
    expect(useDraftStore.getState().buffers[d.id].fields.subject).toBe(
      "Second",
    );
    expect(useDraftStore.getState().buffers[d.id].dirty).toBe(false);
  });
  it("updates attempt state even when Gmail reconciliation fails", async () => {
    useDraftStore.getState().adopt(fixture());
    vi.mocked(commands.syncDrafts).mockResolvedValue({
      status: "error",
      error: "EmailOutcomeUnknown",
    });
    vi.mocked(commands.getDraft).mockResolvedValue({
      status: "ok",
      data: fixture({
        revision: 3,
        status: "Unknown",
        attempt_kind: "create",
        attempt_id: "attempt",
      }),
    });
    await expect(syncDraftAccount("1")).rejects.toBe("EmailOutcomeUnknown");
    expect(useDraftStore.getState().buffers.draft.draft.status).toBe("Unknown");
  });
});

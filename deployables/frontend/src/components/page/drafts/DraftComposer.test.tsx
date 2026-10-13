import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import {
  render,
  screen,
  cleanup,
  fireEvent,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { commands, type Draft } from "@/bindings";
import { useDraftStore } from "@/lib/DraftStore";
import DraftComposer from "./DraftComposer";
vi.mock("@/bindings", () => ({
  commands: {
    getDraft: vi.fn(),
    saveDraft: vi.fn(),
    syncDrafts: vi.fn(),
    sendDraft: vi.fn(),
    emailListAccounts: vi.fn(),
    openGmail: vi.fn(),
    resolveSendAttempt: vi.fn(),
  },
}));
vi.mock("./RichTextEditor", () => ({
  RichTextEditor: ({
    initialHtml,
    readOnly,
    onChange,
  }: {
    initialHtml: string;
    readOnly: boolean;
    onChange: (c: { body_html: string; body_text: string }) => void;
  }) => (
    <textarea
      aria-label="Message body"
      defaultValue={initialHtml}
      disabled={readOnly}
      onChange={(e) =>
        onChange({ body_html: e.target.value, body_text: e.target.value })
      }
    />
  ),
}));
const draft = (status: Draft["status"]): Draft => ({
  id: "draft",
  account_id: "1",
  fields: {
    to: "you@example.com",
    cc: "",
    bcc: "",
    subject: "Hello",
    body_text: "Body",
    body_html: "<div>Body</div>",
    quote_html: "",
  },
  revision: 1,
  acknowledged_revision: 1,
  status,
  remote: null,
  conflict: null,
  issues: status === "Unsupported" ? ["Attachment or inline image"] : [],
  thread_id: null,
  in_reply_to: null,
  references: null,
  attempt_id: status === "Unknown" ? "attempt" : null,
  attempt_kind: status === "Unknown" ? "send" : null,
  sent_message_id: null,
});
function show(d: Draft) {
  vi.mocked(commands.getDraft).mockResolvedValue({ status: "ok", data: d });
  useDraftStore.getState().adopt(d);
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DraftComposer accountId="1" draftId="draft" />
    </QueryClientProvider>,
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  useDraftStore.setState({ buffers: {}, syncErrors: {}, syncing: {} });
  vi.mocked(commands.syncDrafts).mockResolvedValue({
    status: "ok",
    data: null,
  });
  vi.mocked(commands.emailListAccounts).mockResolvedValue({
    status: "ok",
    data: [{ id: "1", name: "me@example.com" }],
  });
});
afterEach(cleanup);
describe("composer", () => {
  it("keeps unsupported content read-only and offers Gmail", () => {
    show(draft("Unsupported"));
    expect(
      (screen.getByLabelText("Message body") as HTMLTextAreaElement).disabled,
    ).toBe(true);
    expect(
      (screen.getByRole("button", { name: "Send" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(screen.getByRole("button", { name: "Open in Gmail" })).toBeTruthy();
    expect(commands.sendDraft).not.toHaveBeenCalled();
  });
  it("requires acknowledgement before returning to editing", () => {
    show(draft("Unknown"));
    const button = screen.getByRole("button", {
      name: "Return to editing",
    }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox"));
    expect(button.disabled).toBe(false);
    expect(commands.sendDraft).not.toHaveBeenCalled();
  });
  it("blocks offline Send without calling Gmail", async () => {
    const online = vi.spyOn(navigator, "onLine", "get").mockReturnValue(false);
    show(draft("Local"));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain(
        "Sending has not been queued",
      ),
    );
    expect(commands.sendDraft).not.toHaveBeenCalled();
    online.mockRestore();
  });
  it("sends only after the user clicks Send", async () => {
    const d = draft("Local");
    vi.mocked(commands.sendDraft).mockResolvedValue({
      status: "ok",
      data: { ...d, status: "Sent" },
    });
    show(d);
    await waitFor(() =>
      expect(screen.getByText("From: me@example.com")).toBeTruthy(),
    );
    expect(commands.sendDraft).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(commands.sendDraft).toHaveBeenCalledTimes(1));
    expect(commands.sendDraft).toHaveBeenCalledWith("draft", 1);
  });
  it("shows the quoted original in a frame and keeps bodies out of the dev dump", () => {
    const d = draft("Local");
    d.fields.body_text = "SECRET-TEXT";
    d.fields.body_html = "<div>SECRET-HTML</div>";
    d.fields.quote_html = '<div class="gmail_quote">SECRET-QUOTE</div>';

    const { container } = show(d);
    expect(container.querySelector("details iframe")).toBeTruthy();
    fireEvent.click(container.querySelector("svg.lucide-bug")!);
    const dump = container.querySelector("pre")!.textContent!;
    expect(dump).toContain('"subject": "Hello"');
    expect(dump).toContain('"quote_html": "<43 chars>"');
    expect(dump).not.toContain("SECRET");
  });
});

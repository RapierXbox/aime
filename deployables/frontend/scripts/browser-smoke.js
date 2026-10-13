// Run with the Playwright page tool against `vite --host 127.0.0.1 --port 5174 --strictPort`.
// This replaces Tauri IPC with in-memory fixtures; it never accesses Gmail or local account data.
// prettier-ignore
async (page) => {
  await page.addInitScript(() => {
    let sequence = 0;
    window.__emailSmoke = { drafts: {}, calls: [], sends: 0 };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main" },
      },
      transformCallback: () => ++sequence,
      invoke: async (command, args = {}) => {
        const state = window.__emailSmoke;
        state.calls.push(command);
        if (command === "email_list_accounts")
          return [{ id: "1", name: "test@example.com" }];
        if (command === "sync_drafts") return null;
        if (command === "create_draft") {
          const id = "draft-" + ++sequence;
          const draft = {
            id,
            account_id: "1",
            fields: {
              to: "",
              cc: "",
              bcc: "",
              subject: "",
              body_text: "",
              body_html: "<div><br></div>",
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
          };
          state.drafts[id] = draft;
          return structuredClone(draft);
        }
        if (command === "get_draft")
          return structuredClone(state.drafts[args.draftId]);
        if (command === "list_drafts") return Object.values(state.drafts);
        if (command === "save_draft") {
          const draft = state.drafts[args.draftId];
          if (draft.revision !== args.expectedRevision)
            throw "DraftRevisionConflict";
          draft.fields = args.fields;
          draft.revision++;
          return structuredClone(draft);
        }
        if (command === "send_draft") {
          state.sends++;
          const draft = state.drafts[args.draftId];
          draft.status = "Sent";
          return structuredClone(draft);
        }
        if (command === "plugin:window|theme") return "Light";
        if (command === "plugin:event|listen") return ++sequence;
        if (command === "plugin:event|unlisten") return null;
        throw new Error("Unexpected mock command " + command);
      },
    };
  });
  await page.goto("http://127.0.0.1:5174");
  await page.getByRole("button", { name: "Compose from test@example.com" }).click();
  await page.getByLabel("To", { exact: true }).fill("recipient@example.com");
  await page.getByLabel("Subject", { exact: true }).fill("Browser smoke");
  const editor = page.getByRole("textbox", { name: "Message body" });
  await editor.click();
  await editor.pressSequentially("First line");
  await editor.press("Enter");
  await editor.press("Enter");
  await editor.pressSequentially("Last line");
  await page.getByRole("button", { name: "Save & sync" }).click();
  const saved = await page.evaluate(
    () => Object.values(window.__emailSmoke.drafts)[0],
  );
  if (saved.fields.body_text !== "First line\n\nLast line")
    throw new Error("Paragraph preservation failed");
  await editor.click();
  await editor.press("ControlOrMeta+End");
  await editor.evaluate((element) => {
    const data = new DataTransfer();
    data.setData("text/plain", "Pasted\n\ntext");
    element.dispatchEvent(
      new ClipboardEvent("paste", {
        clipboardData: data,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
  await page.getByRole("button", { name: "Save & sync" }).click();
  const pasted = await page.evaluate(
    () => Object.values(window.__emailSmoke.drafts)[0].fields.body_text,
  );
  if (!pasted.includes("Pasted\n\ntext"))
    throw new Error("Plain-text paste failed");
  await editor.click();
  await editor.press("ControlOrMeta+z");
  await page.getByRole("button", { name: "Save & sync" }).click();
  const undone = await page.evaluate(
    () => Object.values(window.__emailSmoke.drafts)[0].fields.body_text,
  );
  if (undone !== saved.fields.body_text)
    throw new Error("Paste undo must preserve earlier typing");
  await editor.click();
  await editor.press("ControlOrMeta+Shift+z");
  await page.getByRole("button", { name: "Save & sync" }).click();
  const redone = await page.evaluate(
    () => Object.values(window.__emailSmoke.drafts)[0].fields.body_text,
  );
  if (redone !== pasted) throw new Error("Paste redo failed");
  if ((await page.evaluate(() => window.__emailSmoke.sends)) !== 0)
    throw new Error("Implicit send");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await page.getByText("Sent. Gmail accepted the message.").waitFor();
  const sends = await page.evaluate(() => window.__emailSmoke.sends);
  if (sends !== 1)
    throw new Error("Expected exactly one user-triggered mock send");
  return {
    paragraphs: true,
    paste: true,
    undo: true,
    redo: true,
    sends,
    mockBackend: true,
  };
}

import { describe, expect, it, vi, afterEach } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { PlateEditor } from "platejs/react";
import { RichTextEditor } from "./RichTextEditor";
let editor: PlateEditor | null | undefined;
vi.mock("platejs/react", async (orig) => {
  const m = await orig<typeof import("platejs/react")>();
  return {
    ...m,
    usePlateEditor: (...args: Parameters<typeof m.usePlateEditor>) =>
      (editor = m.usePlateEditor(...args)),
  };
});
afterEach(cleanup);
describe("RichTextEditor", () => {
  it("renders initialHtml with bold", () => {
    render(
      <RichTextEditor
        initialHtml="<div>Hello <strong>bold</strong></div>"
        onChange={vi.fn()}
        readOnly={false}
      />,
    );
    const body = screen.getByRole("textbox", { name: "Message body" });
    expect(body.textContent).toBe("Hello bold");
    expect(body.querySelector("strong")?.textContent).toBe("bold");
  });
  it("keeps locked drafts non-editable", () => {
    render(
      <RichTextEditor initialHtml="<div>Read only</div>" onChange={vi.fn()} readOnly />,
    );
    expect(
      screen
        .getByRole("textbox", { name: "Message body" })
        .getAttribute("contenteditable"),
    ).toBe("false");
    expect(
      (screen.getByRole("button", { name: "Bold" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
  it("emits html and text on change", async () => {
    const onChange = vi.fn();
    render(
      <RichTextEditor initialHtml="<div>Hi</div>" onChange={onChange} readOnly={false} />,
    );
    // jsdom can't drive Slate through DOM selection, so use the captured editor
    act(() => {
      editor!.tf.select({ path: [0, 0], offset: 0 });
      editor!.tf.toggleBlock("h1");
    });
    await waitFor(() =>
      expect(onChange).toHaveBeenLastCalledWith({
        body_html: "<h1>Hi</h1>",
        body_text: "Hi",
      }),
    );
  });
  it("inserts a link from the inline URL field", async () => {
    const onChange = vi.fn();
    render(
      <RichTextEditor initialHtml="<div>Hi</div>" onChange={onChange} readOnly={false} />,
    );
    act(() => {
      editor!.tf.select({ anchor: { path: [0, 0], offset: 0 }, focus: { path: [0, 0], offset: 2 } });
    });
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    const url = screen.getByRole("textbox", { name: "Link URL" });
    fireEvent.change(url, { target: { value: "https://example.com/" } });
    fireEvent.keyDown(url, { key: "Enter" });
    await waitFor(() =>
      expect(onChange).toHaveBeenLastCalledWith({
        body_html: '<div><a href="https://example.com/">Hi</a></div>',
        body_text: "Hi",
      }),
    );
    expect(screen.queryByRole("textbox", { name: "Link URL" })).toBeNull();
  });
});

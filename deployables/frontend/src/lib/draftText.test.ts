import { describe, expect, it } from "vitest";
import { createPlateEditor, ParagraphPlugin } from "platejs/react";
import type { Value } from "platejs";
import {
  BlockquotePlugin,
  BoldPlugin,
  CodePlugin,
  H1Plugin,
  H2Plugin,
  H3Plugin,
  ItalicPlugin,
  StrikethroughPlugin,
  UnderlinePlugin,
} from "@platejs/basic-nodes/react";
import { ListPlugin } from "@platejs/list/react";
import { toggleList } from "@platejs/list";
import { LinkPlugin } from "@platejs/link/react";
import { htmlToValue, valueToHtml, valueToText } from "./draftText";
// registers the app's global href-stripping DOMPurify hook, as in the composer
import "@/components/page/inbox/MessageView";

const editor = () =>
  createPlateEditor({
    plugins: [
      ParagraphPlugin,
      BoldPlugin,
      ItalicPlugin,
      UnderlinePlugin,
      StrikethroughPlugin,
      CodePlugin,
      BlockquotePlugin,
      H1Plugin,
      H2Plugin,
      H3Plugin,
      ListPlugin,
      LinkPlugin,
    ],
  });
const p = (...children: object[]) => ({ type: "p", children });
const html = (value: object[]) => valueToHtml(value as Value);

describe("valueToHtml", () => {
  it("wraps marks and escapes text", () => {
    expect(
      html([
        p(
          { text: "a <b> & \"c\"", bold: true, italic: true },
          { text: "u", underline: true },
          { text: "s", strikethrough: true },
          { text: "k", code: true },
        ),
      ]),
    ).toBe(
      "<div><em><strong>a &#60;b&#62; &#38; &#34;c&#34;</strong></em><u>u</u><s>s</s><code>k</code></div>",
    );
  });
  it("serializes blocks, soft breaks and empty blocks", () => {
    expect(
      html([
        { type: "h2", children: [{ text: "H" }] },
        { type: "blockquote", children: [{ text: "q" }] },
        p({ text: "x\ny" }),
        p({ text: "" }),
      ]),
    ).toBe("<h2>H</h2><blockquote>q</blockquote><div>x<br>y</div><div><br></div>");
  });
  it("groups indent-list blocks into nested ul/ol", () => {
    const li = (text: string, indent: number, listStyleType: string) => ({
      ...p({ text }),
      indent,
      listStyleType,
    });
    expect(
      html([
        li("a", 1, "disc"),
        li("b", 2, "decimal"),
        li("c", 1, "disc"),
        p({ text: "after" }),
        li("d", 1, "decimal"),
      ]),
    ).toBe(
      "<ul><li>a<ol><li>b</li></ol></li><li>c</li></ul><div>after</div><ol><li>d</li></ol>",
    );
  });
  it("allows only http, https and mailto links and escapes attributes", () => {
    const a = (url: string) => ({ type: "a", url, children: [{ text: "t" }] });
    expect(html([p(a("https://x.y/?a=1&b=\"2\""))])).toBe(
      '<div><a href="https://x.y/?a=1&#38;b=%222%22">t</a></div>',
    );
    expect(html([p(a("mailto:me@x.y"))])).toContain('href="mailto:me@x.y"');
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "/relative", " "])
      expect(html([p(a(bad))])).toBe("<div>t</div>");
  });
});

describe("htmlToValue", () => {
  it("drops scripts and unregistered markup", () => {
    const v = htmlToValue(editor(), '<div>ok<script>alert(1)</script><img src=x onerror=y></div>');
    expect(valueToText(v)).toBe("ok");
  });
  it("round-trips through a real editor", () => {
    const e = editor();
    const source =
      '<div><strong>bold</strong> <em>it</em> <a href="https://x.y/">l</a></div>' +
      "<h1>T</h1><blockquote>q</blockquote>" +
      "<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul><ol><li>n</li></ol><div>end</div>";
    const once = valueToHtml(htmlToValue(e, source));
    expect(once).toBe(source);
    expect(valueToHtml(htmlToValue(e, once))).toBe(once);
  });
  it("keeps blank lines stable", () => {
    const blank = "<div>a</div><div><br></div><div>b</div>";
    expect(valueToHtml(htmlToValue(editor(), blank))).toBe(blank);
  });
  it("matches what the editor's list toggle produces", () => {
    const e = editor();
    e.children = [p({ text: "hello" })] as Value;
    e.tf.select({ path: [0, 0], offset: 0 });
    toggleList(e, { listStyleType: "decimal" });
    expect(e.children[0]).toMatchObject({ indent: 1, listStyleType: "decimal" });
    expect(valueToHtml(e.children)).toBe("<ol><li>hello</li></ol>");
  });
  it("formats through the editor's mark and block transforms", () => {
    const e = editor();
    e.children = [p({ text: "hi" })] as Value;
    e.tf.select({
      anchor: { path: [0, 0], offset: 0 },
      focus: { path: [0, 0], offset: 2 },
    });
    e.tf.toggleMark(BoldPlugin.key);
    e.tf.toggleBlock(BlockquotePlugin.key);
    expect(valueToHtml(e.children)).toBe("<blockquote><strong>hi</strong></blockquote>");
  });
  it("returns one empty paragraph for empty html", () => {
    expect(htmlToValue(editor(), "")).toEqual([p({ text: "" })]);
  });
});

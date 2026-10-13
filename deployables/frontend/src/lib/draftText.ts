import DOMPurify from "dompurify";
import { deserializeHtml, type Descendant, type SlateEditor, type TElement, type Value } from "platejs";
import { ULIST_STYLE_TYPES } from "@platejs/list";

/** Own instance: MessageView's global hook strips every href, which would drop the draft's links. */
const purify = DOMPurify(window);

/** Plain-text rendering of an editor value: one line per block. */
export function valueToText(value: Value): string {
  const text = (node: Descendant): string =>
    "text" in node ? String(node.text) : node.children.map(text).join("");
  return value.map(text).join("\n");
}

const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

/** Marks in wrap order, innermost first. */
const MARKS = [
  ["code", "code"],
  ["bold", "strong"],
  ["italic", "em"],
  ["underline", "u"],
  ["strikethrough", "s"],
] as const;

/** `href` is only emitted for these schemes; everything else drops the link. */
const SAFE_SCHEMES = ["http:", "https:", "mailto:"];

function safeHref(url: unknown): string | null {
  try {
    const u = new URL(String(url));
    return SAFE_SCHEMES.includes(u.protocol) ? u.href : null;
  } catch {
    return null;
  }
}

/** Serializes inline nodes (text leaves and links); all text and attributes are escaped. */
function inlineHtml(node: Descendant): string {
  if ("text" in node) {
    const text = escapeHtml(String(node.text)).replace(/\n/g, "<br>");
    return MARKS.reduce(
      (html, [mark, tag]) => (node[mark] ? `<${tag}>${html}</${tag}>` : html),
      text,
    );
  }
  const inner = node.children.map(inlineHtml).join("");
  switch (node.type) {
    case "a": {
      const href = safeHref(node.url);
      return href ? `<a href="${escapeHtml(href)}">${inner}</a>` : inner;
    }
    default:
      return inner;
  }
}

/** Serializes one non-list block. Empty blocks get a `<br>` so they keep their height. */
function blockHtml(node: TElement): string {
  const inner = node.children.map(inlineHtml).join("") || "<br>";
  switch (node.type) {
    case "h1":
    case "h2":
    case "h3":
    case "blockquote":
      return `<${node.type}>${inner}</${node.type}>`;
    default:
      return `<div>${inner}</div>`;
  }
}

/**
 * Serializes an editor value to the HTML that goes into the email.
 * Plate's list plugin has no ul/li nodes: a list item is a block carrying
 * `listStyleType` and `indent` (1-based depth), so consecutive such blocks are
 * regrouped here into nested ul/ol.
 */
export function valueToHtml(value: Value): string {
  const open: string[] = []; // tags of the currently open lists, outermost first
  let out = "";
  const closeTo = (depth: number) => {
    while (open.length > depth) out += `</li></${open.pop()}>`;
  };
  for (const node of value as TElement[]) {
    if (!node.listStyleType) {
      closeTo(0);
      out += blockHtml(node);
      continue;
    }
    const tag = (ULIST_STYLE_TYPES as readonly string[]).includes(
      String(node.listStyleType),
    )
      ? "ul"
      : "ol";
    const depth = Math.max(1, Number(node.indent) || 1);
    closeTo(depth);
    if (open.length === depth) {
      if (open[depth - 1] === tag) out += "</li>";
      else closeTo(depth - 1);
    }
    // ponytail: a skipped indent level nests directly, no empty <li> between
    while (open.length < depth) {
      out += `<${tag}>`;
      open.push(tag);
    }
    out += `<li>${node.children.map(inlineHtml).join("")}`;
  }
  closeTo(0);
  return out;
}

/** Converts one ul/ol element into list blocks (nested lists recurse). */
function listBlocks(editor: SlateEditor, list: Element, depth = 1): TElement[] {
  const listStyleType = list.tagName === "OL" ? "decimal" : "disc";
  return [...list.children].flatMap((li) => {
    const nested = [...li.children].filter((c) => /^[UO]L$/.test(c.tagName));
    nested.forEach((n) => n.remove());
    const item = deserializeHtml(editor, { element: `<div>${li.innerHTML}</div>` });
    const blocks = (item.length ? item : [{ type: "p", children: [{ text: "" }] }]) as TElement[];
    return [
      ...blocks.map((b) => ({ ...b, indent: depth, listStyleType })),
      ...nested.flatMap((n) => listBlocks(editor, n, depth + 1)),
    ];
  });
}

/**
 * Parses stored HTML into an editor value. The HTML is sanitized first, and
 * `deserializeHtml` then keeps only nodes of plugins registered on `editor`.
 * ul/ol are converted by hand, because the list plugin's own deserializer does
 * not produce `listStyleType` blocks.
 */
export function htmlToValue(editor: SlateEditor, html: string): Value {
  const body = new DOMParser().parseFromString(
    purify.sanitize(html, { FORBID_TAGS: ["style"], FORBID_ATTR: ["style"] }),
    "text/html",
  ).body;
  const value: TElement[] = [];
  let run = "";
  const flush = () => {
    if (run.trim()) value.push(...(deserializeHtml(editor, { element: run }) as TElement[]));
    run = "";
  };
  for (const node of body.childNodes) {
    if (node instanceof Element && /^[UO]L$/.test(node.tagName)) {
      flush();
      value.push(...listBlocks(editor, node));
    } else run += node instanceof Element ? node.outerHTML : escapeHtml(node.textContent ?? "");
  }
  flush();
  return value.length ? value : [{ type: "p", children: [{ text: "" }] }];
}

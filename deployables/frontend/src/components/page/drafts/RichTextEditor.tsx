import { useState, type ComponentProps } from "react";
import {
  Bold,
  Code,
  Heading1,
  Heading2,
  Heading3,
  Italic,
  Link,
  List,
  ListOrdered,
  Quote,
  Strikethrough,
  Underline,
} from "lucide-react";
import {
  Plate,
  PlateContent,
  PlateElement,
  PlateLeaf,
  ParagraphPlugin,
  usePlateEditor,
  type PlateElementProps,
  type PlateLeafProps,
} from "platejs/react";
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
import { upsertLink } from "@platejs/link";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { htmlToValue, valueToHtml, valueToText } from "@/lib/draftText";

const leaf =
  (as: "strong" | "em" | "u" | "s" | "code", className?: string) =>
  (props: PlateLeafProps) => <PlateLeaf {...props} as={as} className={className} />;

/** List items are plain blocks with `listStyleType`/`indent`; list-item display draws the marker. */
function block(as: "div" | "h1" | "h2" | "h3" | "blockquote", className?: string) {
  return function Block(props: PlateElementProps) {
    const { listStyleType, indent } = props.element as {
      listStyleType?: string;
      indent?: number;
    };
    return (
      <PlateElement
        {...props}
        as={as}
        className={className}
        // ponytail: ordered numbering runs across nesting levels; no per-level counters
        style={
          listStyleType
            ? {
                display: "list-item",
                listStyleType,
                marginLeft: `${(indent ?? 1) * 1.5}em`,
              }
            : undefined
        }
      />
    );
  };
}

const PLUGINS = [
  ParagraphPlugin.withComponent(block("div", "min-h-5")),
  BoldPlugin.withComponent(leaf("strong")),
  ItalicPlugin.withComponent(leaf("em")),
  UnderlinePlugin.withComponent(leaf("u")),
  StrikethroughPlugin.withComponent(leaf("s")),
  CodePlugin.withComponent(leaf("code", "bg-muted px-1 font-mono text-[0.9em]")),
  BlockquotePlugin.withComponent(
    block("blockquote", "border-l border-border pl-3 text-muted-foreground"),
  ),
  H1Plugin.withComponent(block("h1", "text-xl font-semibold")),
  H2Plugin.withComponent(block("h2", "text-lg font-semibold")),
  H3Plugin.withComponent(block("h3", "text-base font-semibold")),
  ListPlugin,
  LinkPlugin.withComponent((props: PlateElementProps) => (
    <PlateElement {...props} as="a" className="underline" />
  )),
];

function ToolbarButton({
  label,
  disabled,
  onClick,
  children,
}: Pick<ComponentProps<typeof Button>, "disabled" | "onClick" | "children"> & {
  label: string;
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      className="rounded-none"
      aria-label={label}
      title={label}
      disabled={disabled}
      // keep the editor selection when the button is pressed
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
    >
      {children}
    </Button>
  );
}

export function RichTextEditor({
  initialHtml,
  onChange,
  readOnly,
}: {
  initialHtml: string;
  onChange: (change: { body_html: string; body_text: string }) => void;
  readOnly: boolean;
}) {
  const editor = usePlateEditor({
    plugins: PLUGINS,
    value: (e) => htmlToValue(e, initialHtml),
  });
  const mark = (key: string) => () => editor.tf.toggleMark(key);
  const blockType = (type: string) => () => editor.tf.toggleBlock(type);
  const list = (listStyleType: string) => () =>
    toggleList(editor, { listStyleType });
  // Inline URL field: WKWebView (Tauri on macOS) has no window.prompt UI.
  const [linkUrl, setLinkUrl] = useState<string | null>(null);
  const insertLink = (url: string) => {
    if (url) upsertLink(editor, { url });
    setLinkUrl(null);
  };
  return (
    <Plate
      editor={editor}
      readOnly={readOnly}
      onValueChange={({ value }) =>
        onChange({ body_html: valueToHtml(value), body_text: valueToText(value) })
      }
    >
      <div
        role="toolbar"
        aria-label="Formatting"
        className="flex flex-wrap items-center border-b border-border px-2"
      >
        <ToolbarButton label="Bold" disabled={readOnly} onClick={mark(BoldPlugin.key)}><Bold /></ToolbarButton>
        <ToolbarButton label="Italic" disabled={readOnly} onClick={mark(ItalicPlugin.key)}><Italic /></ToolbarButton>
        <ToolbarButton label="Underline" disabled={readOnly} onClick={mark(UnderlinePlugin.key)}><Underline /></ToolbarButton>
        <ToolbarButton label="Strikethrough" disabled={readOnly} onClick={mark(StrikethroughPlugin.key)}><Strikethrough /></ToolbarButton>
        <ToolbarButton label="Code" disabled={readOnly} onClick={mark(CodePlugin.key)}><Code /></ToolbarButton>
        <span className="mx-1 h-4 w-px bg-border" />
        <ToolbarButton label="Heading 1" disabled={readOnly} onClick={blockType(H1Plugin.key)}><Heading1 /></ToolbarButton>
        <ToolbarButton label="Heading 2" disabled={readOnly} onClick={blockType(H2Plugin.key)}><Heading2 /></ToolbarButton>
        <ToolbarButton label="Heading 3" disabled={readOnly} onClick={blockType(H3Plugin.key)}><Heading3 /></ToolbarButton>
        <ToolbarButton label="Quote" disabled={readOnly} onClick={blockType(BlockquotePlugin.key)}><Quote /></ToolbarButton>
        <span className="mx-1 h-4 w-px bg-border" />
        <ToolbarButton label="Bulleted list" disabled={readOnly} onClick={list("disc")}><List /></ToolbarButton>
        <ToolbarButton label="Numbered list" disabled={readOnly} onClick={list("decimal")}><ListOrdered /></ToolbarButton>
        <ToolbarButton label="Link" disabled={readOnly} onClick={() => setLinkUrl("")}><Link /></ToolbarButton>
        {linkUrl !== null && (
          <Input
            autoFocus
            aria-label="Link URL"
            placeholder="https://…"
            className="h-7 w-56 rounded-none"
            value={linkUrl}
            onChange={(e) => setLinkUrl(e.target.value)}
            onBlur={() => setLinkUrl(null)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                insertLink(linkUrl.trim());
              } else if (e.key === "Escape") setLinkUrl(null);
            }}
          />
        )}
      </div>
      <PlateContent
        aria-label="Message body"
        role="textbox"
        aria-multiline="true"
        className="flex-1 min-h-64 p-4 outline-none text-sm"
      />
    </Plate>
  );
}

/**
 * Render parsed markdown as React elements.
 *
 * Elements, never an HTML string — nothing here calls
 * `dangerouslySetInnerHTML`, so there is no path from text somebody typed, or
 * pasted out of a database, to markup the browser will execute. The parser
 * already refuses any link scheme that is not http, https or mailto.
 */

import { useMemo } from "react";
import { parseMarkdown } from "@/lib/markdown";
import type { Block, Inline } from "@/lib/markdown";

export function Markdown({ source }: { source: string }) {
  const blocks = useMemo(() => parseMarkdown(source), [source]);

  if (blocks.length === 0) {
    return <p className="text-[12px] text-text-muted/60 italic">Empty note</p>;
  }

  return <div className="space-y-2">{blocks.map((block, i) => renderBlock(block, i))}</div>;
}

function renderBlock(block: Block, key: number) {
  switch (block.kind) {
    case "heading": {
      const classes = {
        1: "text-[15px] font-semibold text-text",
        2: "text-[13.5px] font-semibold text-text",
        3: "text-[12.5px] font-medium text-text",
      }[block.level];
      // The level decides the tag, so the document outline is real rather than
      // a matter of font size.
      const Tag = (["h1", "h2", "h3"] as const)[block.level - 1] ?? "h3";
      return (
        <Tag key={key} className={classes}>
          {renderInline(block.children)}
        </Tag>
      );
    }

    case "paragraph":
      return (
        <p key={key} className="text-[12px] leading-relaxed text-text">
          {renderInline(block.children)}
        </p>
      );

    case "list": {
      const Tag = block.ordered ? "ol" : "ul";
      return (
        <Tag
          key={key}
          className={`ml-4 space-y-0.5 text-[12px] text-text ${
            block.ordered ? "list-decimal" : "list-disc"
          }`}
        >
          {block.items.map((item, i) => (
            <li key={i}>{renderInline(item)}</li>
          ))}
        </Tag>
      );
    }

    case "quote":
      return (
        <blockquote
          key={key}
          className="border-l-2 border-border pl-2 text-[12px] text-text-muted italic"
        >
          {renderInline(block.children)}
        </blockquote>
      );

    case "code":
      return (
        <pre
          key={key}
          className="overflow-x-auto rounded border border-border bg-surface-2 p-2 font-mono text-[11px] text-text"
          data-selectable
        >
          {block.text}
        </pre>
      );

    case "rule":
      return <hr key={key} className="border-border" />;
  }
}

function renderInline(nodes: Inline[]) {
  return nodes.map((node, i) => {
    switch (node.kind) {
      case "text":
        return <span key={i}>{node.text}</span>;
      case "code":
        return (
          <code key={i} className="rounded bg-surface-2 px-1 font-mono text-[11px] text-text">
            {node.text}
          </code>
        );
      case "strong":
        return (
          <strong key={i} className="font-semibold">
            {renderInline(node.children)}
          </strong>
        );
      case "em":
        return <em key={i}>{renderInline(node.children)}</em>;
      case "link":
        return (
          <a
            key={i}
            href={node.href}
            target="_blank"
            // noreferrer as well as noopener: the opened page has no business
            // knowing which document sent it.
            rel="noopener noreferrer"
            className="text-accent underline decoration-dotted underline-offset-2"
          >
            {renderInline(node.children)}
          </a>
        );
    }
  });
}

/**
 * Lazily expanded object tree.
 *
 * Children are fetched on first expand and cached thereafter. A production
 * database can hold tens of thousands of objects, so introspecting the whole
 * catalog on connect would stall the UI for something the user may never open.
 */

import { useCallback, useEffect, useState } from "react";
import { ipc } from "@/lib/ipc";
import { Spinner, cx } from "../ui/primitives";
import type { NodeKind, SchemaNode } from "@/lib/types";

/** Glyph per node kind. Text rather than icons keeps the tree dense and crisp. */
const GLYPH: Partial<Record<NodeKind, string>> = {
  schema: "◈",
  database: "◈",
  table: "▤",
  view: "▥",
  materialized_view: "▩",
  column: "·",
  collection: "▤",
};

interface TreeState {
  children: Record<string, SchemaNode[]>;
  expanded: Set<string>;
  loading: Set<string>;
  failed: Record<string, string>;
}

export function SchemaTree({
  connectionId,
  onOpenTable,
}: {
  connectionId: string;
  /** Double-clicking a table asks the workspace to query it. */
  onOpenTable: (node: SchemaNode) => void;
}) {
  const [roots, setRoots] = useState<SchemaNode[] | null>(null);
  const [rootError, setRootError] = useState<string | null>(null);
  const [tree, setTree] = useState<TreeState>({
    children: {},
    expanded: new Set(),
    loading: new Set(),
    failed: {},
  });

  useEffect(() => {
    let cancelled = false;
    setRoots(null);
    setRootError(null);
    setTree({ children: {}, expanded: new Set(), loading: new Set(), failed: {} });

    ipc
      .browse(connectionId)
      .then((nodes) => {
        // The connection may have been switched while this was in flight.
        if (!cancelled) setRoots(nodes);
      })
      .catch((e) => {
        if (!cancelled) setRootError((e as Error).message);
      });

    return () => {
      cancelled = true;
    };
  }, [connectionId]);

  const toggle = useCallback(
    async (node: SchemaNode) => {
      const isExpanded = tree.expanded.has(node.id);
      if (isExpanded) {
        setTree((t) => {
          const expanded = new Set(t.expanded);
          expanded.delete(node.id);
          return { ...t, expanded };
        });
        return;
      }

      setTree((t) => ({ ...t, expanded: new Set(t.expanded).add(node.id) }));

      // Already fetched: expanding again must not re-query.
      if (tree.children[node.id]) return;

      setTree((t) => ({ ...t, loading: new Set(t.loading).add(node.id) }));
      try {
        const children = await ipc.browse(connectionId, node.id);
        setTree((t) => {
          const loading = new Set(t.loading);
          loading.delete(node.id);
          const failed = { ...t.failed };
          delete failed[node.id];
          return { ...t, children: { ...t.children, [node.id]: children }, loading, failed };
        });
      } catch (e) {
        setTree((t) => {
          const loading = new Set(t.loading);
          loading.delete(node.id);
          return { ...t, loading, failed: { ...t.failed, [node.id]: (e as Error).message } };
        });
      }
    },
    [connectionId, tree.expanded, tree.children],
  );

  if (rootError) {
    return (
      <p role="alert" className="px-2 py-3 text-[11px] text-danger">
        {rootError}
      </p>
    );
  }

  if (!roots) {
    return (
      <div className="flex items-center justify-center py-6">
        <Spinner className="text-text-muted" />
      </div>
    );
  }

  if (roots.length === 0) {
    return <p className="px-2 py-3 text-[11px] text-text-muted">No objects.</p>;
  }

  return (
    <ul className="py-1">
      {roots.map((node) => (
        <TreeNode
          key={node.id}
          node={node}
          depth={0}
          tree={tree}
          onToggle={toggle}
          onOpenTable={onOpenTable}
        />
      ))}
    </ul>
  );
}

function TreeNode({
  node,
  depth,
  tree,
  onToggle,
  onOpenTable,
}: {
  node: SchemaNode;
  depth: number;
  tree: TreeState;
  onToggle: (node: SchemaNode) => void;
  onOpenTable: (node: SchemaNode) => void;
}) {
  const expanded = tree.expanded.has(node.id);
  const loading = tree.loading.has(node.id);
  const children = tree.children[node.id];
  const error = tree.failed[node.id];
  const isTable = node.kind === "table" || node.kind === "view" || node.kind === "materialized_view";

  return (
    <li>
      <div
        role="treeitem"
        aria-expanded={node.expandable ? expanded : undefined}
        tabIndex={0}
        onClick={() => node.expandable && onToggle(node)}
        onDoubleClick={() => isTable && onOpenTable(node)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (isTable) onOpenTable(node);
            else if (node.expandable) onToggle(node);
          }
        }}
        style={{ paddingLeft: depth * 12 + 6 }}
        className="flex cursor-default items-center gap-1.5 py-[3px] pr-2 hover:bg-surface-2"
        title={node.detail ? `${node.name} — ${node.detail}` : node.name}
      >
        <span className="w-2.5 shrink-0 text-[8px] text-text-muted">
          {node.expandable ? (expanded ? "▾" : "▸") : ""}
        </span>
        <span className="shrink-0 text-[10px] text-text-muted">{GLYPH[node.kind] ?? "·"}</span>
        <span className="truncate text-[11.5px] text-text">{node.name}</span>
        {node.detail && (
          <span className="ml-auto shrink-0 truncate pl-2 font-mono text-[9.5px] text-text-muted/70">
            {node.detail}
          </span>
        )}
        {loading && <Spinner className="ml-auto size-2.5 text-text-muted" />}
      </div>

      {expanded && error && (
        <p style={{ paddingLeft: (depth + 1) * 12 + 20 }} className="py-1 text-[10.5px] text-danger">
          {error}
        </p>
      )}

      {expanded && children && (
        <ul>
          {children.length === 0 ? (
            <li
              style={{ paddingLeft: (depth + 1) * 12 + 20 }}
              className="py-1 text-[10.5px] text-text-muted/60"
            >
              empty
            </li>
          ) : (
            children.map((child) => (
              <TreeNode
                key={child.id}
                node={child}
                depth={depth + 1}
                tree={tree}
                onToggle={onToggle}
                onOpenTable={onOpenTable}
              />
            ))
          )}
        </ul>
      )}
    </li>
  );
}

/** Build a browse query for a table node. */
export function selectFor(node: SchemaNode, quote: string): string {
  const q = (name: string) => `${quote}${name.replaceAll(quote, quote + quote)}${quote}`;
  // Node ids are `schema.table` where the driver reports schemas, otherwise the
  // bare table name.
  const parts = node.id.split(".");
  const qualified = parts.length > 1 ? parts.map(q).join(".") : q(node.name);
  return `SELECT * FROM ${qualified};`;
}

export { cx };

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
  folder: "▸",
  function: "ƒ",
  procedure: "ƒ",
  trigger: "⚡",
  sequence: "#",
  index: "⋮",
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

/** Where a node sits, gathered from what the tree actually rendered above it. */
export interface NodeContext {
  database?: string | undefined;
  schema?: string | undefined;
}

export function SchemaTree({
  connectionId,
  activeDatabase,
  onOpenTable,
  onSelectDatabase,
  onOpenScript,
  onContextMenu,
}: {
  connectionId: string;
  /** The database the session is pointed at, marked in the list. */
  activeDatabase: string | null;
  /** Clicking an object asks the workspace to open it as a tab. */
  onOpenTable: (node: SchemaNode & NodeContext) => void;
  /** Clicking a database asks the session to switch to it. */
  onSelectDatabase: (name: string) => void;
  /** Clicking an object whose script *is* the object opens that script. */
  onOpenScript: (node: SchemaNode & NodeContext) => void;
  /**
   * Right-clicking asks for a menu at that point. The third argument reloads
   * this node's children, which only the tree can do — it owns the cache — and
   * is null for a node that has none.
   */
  onContextMenu: (
    node: SchemaNode & NodeContext,
    at: { x: number; y: number },
    refresh: (() => void) | null,
  ) => void;
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

  /** Forget a node's children and fetch them again. */
  const reload = useCallback(
    async (node: SchemaNode) => {
      setTree((t) => {
        const children = { ...t.children };
        delete children[node.id];
        return { ...t, children, loading: new Set(t.loading).add(node.id) };
      });
      try {
        const children = await ipc.browse(connectionId, node.id);
        setTree((t) => {
          const loading = new Set(t.loading);
          loading.delete(node.id);
          const failed = { ...t.failed };
          delete failed[node.id];
          return {
            ...t,
            children: { ...t.children, [node.id]: children },
            expanded: new Set(t.expanded).add(node.id),
            loading,
            failed,
          };
        });
      } catch (e) {
        setTree((t) => {
          const loading = new Set(t.loading);
          loading.delete(node.id);
          return { ...t, loading, failed: { ...t.failed, [node.id]: (e as Error).message } };
        });
      }
    },
    [connectionId],
  );

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
          context={{}}
          activeDatabase={activeDatabase}
          onToggle={toggle}
          onOpenTable={onOpenTable}
          onSelectDatabase={onSelectDatabase}
          onOpenScript={onOpenScript}
          onContextMenu={onContextMenu}
          onReload={reload}
        />
      ))}
    </ul>
  );
}

function TreeNode({
  node,
  depth,
  tree,
  context,
  activeDatabase,
  onToggle,
  onOpenTable,
  onSelectDatabase,
  onOpenScript,
  onContextMenu,
  onReload,
}: {
  node: SchemaNode;
  depth: number;
  tree: TreeState;
  /** Database and schema of everything rendered above this node. */
  context: NodeContext;
  activeDatabase: string | null;
  onToggle: (node: SchemaNode) => void;
  onOpenTable: (node: SchemaNode & NodeContext) => void;
  onSelectDatabase: (name: string) => void;
  onOpenScript: (node: SchemaNode & NodeContext) => void;
  onContextMenu: (
    node: SchemaNode & NodeContext,
    at: { x: number; y: number },
    refresh: (() => void) | null,
  ) => void;
  onReload: (node: SchemaNode) => void;
}) {
  const expanded = tree.expanded.has(node.id);
  const loading = tree.loading.has(node.id);
  const children = tree.children[node.id];
  const error = tree.failed[node.id];

  // Anything with rows to show opens as a tab. Functions, triggers, and
  // sequences are listed but not opened: there is nothing to select from them.
  const opens = node.kind === "table" || node.kind === "view" || node.kind === "materialized_view";
  // For a routine or a trigger the script *is* the object: there are no rows to
  // show, so a click that did nothing would be the only thing this list does
  // nothing for.
  const scripted = node.kind === "function" || node.kind === "procedure" || node.kind === "trigger";
  const isDatabase = node.kind === "database";
  const isActiveDatabase = isDatabase && node.name === activeDatabase;

  // Each level contributes its own name to what its children inherit.
  const childContext: NodeContext = {
    database: isDatabase ? node.name : context.database,
    schema: node.kind === "schema" ? node.name : context.schema,
  };

  const activate = () => {
    if (opens) {
      onOpenTable({ ...node, ...context });
      return;
    }
    if (scripted) {
      onOpenScript({ ...node, ...context });
      return;
    }
    // Selecting a database points the session at it *and* opens it, because
    // both are what "I want to work in this one" means.
    if (isDatabase && !isActiveDatabase) onSelectDatabase(node.name);
    if (node.expandable) onToggle(node);
  };

  return (
    <li>
      <div
        role="treeitem"
        aria-expanded={node.expandable ? expanded : undefined}
        aria-current={isActiveDatabase || undefined}
        tabIndex={0}
        onClick={activate}
        onContextMenu={(e) => {
          e.preventDefault();
          onContextMenu(
            { ...node, ...context },
            { x: e.clientX, y: e.clientY },
            // Only a node with children has anything to re-fetch.
            node.expandable ? () => onReload(node) : null,
          );
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            activate();
          }
        }}
        style={{ paddingLeft: depth * 12 + 6 }}
        className={cx(
          "flex cursor-default items-center gap-1.5 py-[3px] pr-2 hover:bg-surface-2",
          isActiveDatabase && "bg-surface-2",
        )}
        title={node.detail ? `${node.name} — ${node.detail}` : node.name}
      >
        {/* An object's columns are behind the chevron, since its row is now the
            open-data target. Rendered as a button so it takes the click without
            also opening the object, and so it is reachable from the keyboard. */}
        {node.expandable && opens ? (
          <button
            aria-label={expanded ? `Collapse ${node.name}` : `Expand ${node.name}`}
            onClick={(e) => {
              e.stopPropagation();
              onToggle(node);
            }}
            className="-my-1 w-2.5 shrink-0 py-1 text-[8px] text-text-muted hover:text-text"
          >
            {expanded ? "▾" : "▸"}
          </button>
        ) : (
          <span className="w-2.5 shrink-0 text-[8px] text-text-muted">
            {node.expandable ? (expanded ? "▾" : "▸") : ""}
          </span>
        )}
        <span className="shrink-0 text-[10px] text-text-muted">{GLYPH[node.kind] ?? "·"}</span>
        <span
          className={cx(
            "truncate text-[11.5px]",
            // The database in use is the one every unqualified statement runs
            // against, which is worth more than a subtle highlight.
            isActiveDatabase ? "font-medium text-accent" : "text-text",
            node.kind === "folder" && "text-text-muted uppercase tracking-wide text-[10px]",
          )}
        >
          {node.name}
        </span>
        {node.detail && (
          <span className="ml-auto shrink-0 truncate pl-2 font-mono text-[9.5px] text-text-muted/70">
            {node.detail}
          </span>
        )}
        {loading && <Spinner className="ml-auto size-2.5 text-text-muted" />}
      </div>

      {expanded && error && (
        <p
          style={{ paddingLeft: (depth + 1) * 12 + 20 }}
          className="py-1 text-[10.5px] text-danger"
        >
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
                context={childContext}
                activeDatabase={activeDatabase}
                onToggle={onToggle}
                onOpenTable={onOpenTable}
                onSelectDatabase={onSelectDatabase}
                onOpenScript={onOpenScript}
                onContextMenu={onContextMenu}
                onReload={onReload}
              />
            ))
          )}
        </ul>
      )}
    </li>
  );
}

export { cx };

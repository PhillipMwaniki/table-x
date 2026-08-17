/**
 * Lazily expanded object tree.
 *
 * Children are fetched on first expand and cached thereafter. A production
 * database can hold tens of thousands of objects, so introspecting the whole
 * catalog on connect would stall the UI for something the user may never open.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "@/lib/ipc";
import { Spinner, cx } from "../ui/primitives";
import { matchesName, splitHighlight } from "@/lib/tree";
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

/**
 * How long to wait before applying a filter, in ms.
 *
 * Long enough to skip the letters of a word being typed, short enough that the
 * list still feels attached to the box.
 */
const FILTER_DELAY = 120;

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
  activeObject,
  onOpenTable,
  onSelectDatabase,
  onOpenScript,
  onContextMenu,
}: {
  connectionId: string;
  /** The database the session is pointed at, marked in the list. */
  activeDatabase: string | null;
  /**
   * The object the active tab is showing, marked in the list.
   *
   * Carries its schema and database because a name alone is ambiguous: two
   * schemas holding a `users` table is the normal case, not a corner one, and
   * marking both would point at a table nobody opened.
   */
  activeObject: { name: string; schema?: string | undefined; database?: string | undefined } | null;
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
  const [filter, setFilter] = useState("");
  /**
   * The filter actually applied, a beat behind what is being typed.
   *
   * Every keystroke otherwise re-walks and re-renders the whole tree, and on
   * the schema this feature exists for — thousands of loaded objects — that is
   * felt as the box not keeping up with typing. The input stays instant; only
   * the work behind it waits.
   */
  const [applied, setApplied] = useState("");
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

  useEffect(() => {
    // Clearing is instant — there is no work to defer, and a box that empties
    // but leaves the tree filtered for another beat reads as broken.
    if (filter === "") {
      setApplied("");
      return;
    }
    const timer = setTimeout(() => setApplied(filter), FILTER_DELAY);
    return () => clearTimeout(timer);
  }, [filter]);

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

  // Deliberately not `filter`: see `applied` above.
  const needle = applied.trim();

  /**
   * Which nodes survive the filter, and how many matched by name.
   *
   * Computed once for the whole tree rather than per node: with several
   * thousand objects loaded, asking each row to search its own subtree turns
   * one pass into thousands of overlapping ones.
   *
   * A node survives if its own name matches or anything beneath it does —
   * otherwise a match three levels down would be unreachable, which is the
   * same as not matching.
   *
   * `null` means no filter is on, which is different from a filter that
   * matched nothing.
   */
  const { visible, onPath, matched } = useMemo(() => {
    if (!needle) {
      return { visible: null as Set<string> | null, onPath: new Set<string>(), matched: 0 };
    }

    const ids = new Set<string>();
    // Nodes with a match *below* them, which are the only ones worth opening
    // on the filter's behalf. A node that matches by its own name keeps
    // whatever expansion state the user gave it — filtering for a table should
    // find the table, not splay its columns open.
    const path = new Set<string>();
    let count = 0;

    const walk = (node: SchemaNode): boolean => {
      // Children first, so a matching descendant is recorded even when the
      // parent matches too and would otherwise short-circuit the walk.
      let below = false;
      for (const child of tree.children[node.id] ?? []) {
        if (walk(child)) below = true;
      }

      const self = matchesName(node.name, needle);
      if (self) count += 1;
      if (below) path.add(node.id);
      if (self || below) {
        ids.add(node.id);
        return true;
      }
      return false;
    };

    for (const root of roots ?? []) walk(root);
    return { visible: ids, onPath: path, matched: count };
  }, [roots, tree.children, needle]);

  /**
   * The filter box.
   *
   * Sticky rather than scrolling away with the tree: on the list this exists
   * for, scrolling back to the top to change the filter is the problem.
   */
  const search = (
    <div className="sticky top-0 z-10 border-b border-border bg-surface-1 p-1.5">
      <div className="relative">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setFilter("");
          }}
          placeholder="Filter objects…"
          aria-label="Filter objects by name"
          spellCheck={false}
          className="h-6 w-full rounded border border-border bg-surface-0 pr-5 pl-1.5 text-[11px] outline-none focus:border-accent"
        />
        {filter && (
          <button
            onClick={() => setFilter("")}
            aria-label="Clear the filter"
            className="absolute inset-y-0 right-0 w-5 text-[11px] text-text-muted hover:text-text"
          >
            ✕
          </button>
        )}
      </div>
      {needle && (
        <p className="px-0.5 pt-1 text-[10px] text-text-muted">
          {matched === 0 ? "Nothing loaded matches" : `${matched} matching`}
          {/* Said plainly, because a filter that searched only part of the tree
              and did not say so would read as "this database has no such
              table". */}
          <span className="text-text-muted/60"> · searches what is expanded</span>
        </p>
      )}
    </div>
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
    <>
      {search}
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
            activeObject={activeObject}
            visible={visible}
            onPath={onPath}
            needle={needle}
          />
        ))}
      </ul>
    </>
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
  activeObject,
  visible,
  onPath,
  needle,
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
  activeObject: { name: string; schema?: string | undefined; database?: string | undefined } | null;
  /** Node ids that survive the filter, or `null` when no filter is on. */
  visible: Set<string> | null;
  /** Node ids with a match somewhere beneath them. */
  onPath: Set<string>;
  needle: string;
}) {
  // A filtered-out node renders nothing at all rather than being hidden with
  // CSS: on a schema with thousands of objects the point is to stop building
  // the rows, not to build them and then not show them.
  if (visible && !visible.has(node.id)) return null;

  const filtering = visible !== null;
  const userExpanded = tree.expanded.has(node.id);
  // Anything on the path to a match opens itself: a match three levels down
  // that you still have to click your way to is a match you had to already
  // know about. Only the path, though — a node that matched by its own name
  // keeps the state the user gave it.
  //
  // Written to a local rather than into `tree.expanded`, so clearing the
  // filter puts the tree back exactly as they had it.
  const expanded = userExpanded || (filtering && onPath.has(node.id));
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

  // The object the active tab is showing. Matched on schema and database as
  // well as name, because two schemas holding a `users` table is the normal
  // case and marking both would point at a table nobody opened. A side that is
  // unknown on either the node or the tab is not evidence of a mismatch, so it
  // does not count against the comparison.
  const isActiveObject =
    (opens || scripted) &&
    activeObject !== null &&
    node.name === activeObject.name &&
    (!activeObject.schema || !context.schema || activeObject.schema === context.schema) &&
    (!activeObject.database || !context.database || activeObject.database === context.database);

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
          (isActiveDatabase || isActiveObject) && "bg-surface-2",
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
            // Two different kinds of "you are here", both worth more than a
            // subtle highlight: the database every unqualified statement runs
            // against, and the object the tab in front of you is showing.
            isActiveDatabase && "font-bold text-accent",
            isActiveObject && !isActiveDatabase && "font-bold text-text",
            !isActiveDatabase && !isActiveObject && "text-text",
            node.kind === "folder" && "text-[10px] tracking-wide text-text-muted uppercase",
          )}
        >
          {/* Split around the filter so it is visible *why* a row is in the
              list — over thousands of similar names, "it matched somewhere" is
              not enough to scan by. */}
          {needle
            ? splitHighlight(node.name, needle).map((part, i) => (
                <span key={i} className={part.match ? "text-accent underline" : undefined}>
                  {part.text}
                </span>
              ))
            : node.name}
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
          {children.length === 0 && !filtering ? (
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
                activeObject={activeObject}
                visible={visible}
                onPath={onPath}
                needle={needle}
              />
            ))
          )}
        </ul>
      )}
    </li>
  );
}

export { cx };

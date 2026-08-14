/**
 * Grouping for the connection sidebar.
 *
 * A flat folder name per connection rather than a tree. One level is what
 * separates work from personal, or staging from production — which is what
 * people actually do with folders here — and it avoids the move-between-parents
 * problem for a list that is rarely longer than a screen.
 */

import type { ConnectionConfig } from "./types";

export interface ConnectionGroup {
  /** Folder name, or null for connections filed under none. */
  folder: string | null;
  connections: ConnectionConfig[];
}

/** A folder name reduced to what should be stored: trimmed, or nothing. */
export function normalizeFolder(value: string | undefined | null): string | undefined {
  const trimmed = (value ?? "").trim();
  return trimmed === "" ? undefined : trimmed;
}

/**
 * Connections in display order: folders alphabetically, then the ungrouped.
 *
 * Ungrouped last because a new connection starts there — putting it first would
 * push the folders someone deliberately organised down the sidebar every time
 * they add one.
 *
 * Folder names are compared case-insensitively so "Work" and "work" are one
 * folder rather than two that look identical in the list. The first spelling
 * seen wins as the label, since that is the one the user typed.
 */
export function groupConnections(connections: ConnectionConfig[]): ConnectionGroup[] {
  const groups = new Map<string, ConnectionGroup>();
  const ungrouped: ConnectionConfig[] = [];

  for (const connection of connections) {
    const folder = normalizeFolder(connection.folder);
    if (!folder) {
      ungrouped.push(connection);
      continue;
    }
    const key = folder.toLowerCase();
    const existing = groups.get(key);
    if (existing) existing.connections.push(connection);
    else groups.set(key, { folder, connections: [connection] });
  }

  const sorted = [...groups.values()].sort((a, b) =>
    (a.folder ?? "").localeCompare(b.folder ?? "", undefined, { sensitivity: "base" }),
  );

  // An empty ungrouped section would be a heading with nothing under it.
  if (ungrouped.length > 0) sorted.push({ folder: null, connections: ungrouped });
  return sorted;
}

/** Existing folder names, for the picker in the connection dialog. */
export function folderNames(connections: ConnectionConfig[]): string[] {
  const seen = new Map<string, string>();
  for (const c of connections) {
    const folder = normalizeFolder(c.folder);
    if (folder && !seen.has(folder.toLowerCase())) seen.set(folder.toLowerCase(), folder);
  }
  return [...seen.values()].sort((a, b) =>
    a.localeCompare(b, undefined, { sensitivity: "base" }),
  );
}

/**
 * Typed wrappers over the Tauri command surface.
 *
 * Every call funnels through `call`, which normalizes rejections into a real
 * `IpcError`. Without that, a rejected `invoke` surfaces as a bare object and
 * every call site has to guess at its shape.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  BackendInfo,
  CompletionScope,
  ConnectionConfig,
  DriverInfo,
  ErrorCategory,
  ErrorPayload,
  ExportFormat,
  HistoryEntry,
  HistoryQuery,
  QueryOutcome,
  RowEdit,
  SchemaNode,
  SessionInfo,
  Snippet,
  SshConfig,
  TableDetail,
} from "./types";

/** An error from the backend, carrying its category so callers can branch. */
export class IpcError extends Error {
  readonly category: ErrorCategory;
  readonly retryable: boolean;
  /** 1-based character offset into the statement, when the database reports one. */
  readonly position: number | undefined;
  /** Vendor code, e.g. PostgreSQL SQLSTATE. */
  readonly code: string | undefined;

  constructor(payload: ErrorPayload) {
    super(payload.message);
    this.name = "IpcError";
    this.category = payload.category;
    this.retryable = payload.retryable;
    this.position = payload.position;
    this.code = payload.code;
  }
}

function isErrorPayload(value: unknown): value is ErrorPayload {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as ErrorPayload).message === "string" &&
    typeof (value as ErrorPayload).category === "string"
  );
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (raw) {
    if (isErrorPayload(raw)) throw new IpcError(raw);
    // A rejection that is not an ErrorPayload means the command panicked or the
    // IPC layer itself failed. Surface it as internal rather than swallowing it.
    throw new IpcError({
      message: typeof raw === "string" ? raw : `${command} failed: ${String(raw)}`,
      category: "internal",
      retryable: false,
    });
  }
}

// ---------------------------------------------------------------------------

export const ipc = {
  backendInfo: () => call<BackendInfo>("backend_info"),
  listDrivers: () => call<DriverInfo[]>("list_drivers"),

  listConnections: () => call<ConnectionConfig[]>("list_connections"),
  openConnections: () => call<string[]>("open_connections"),

  /**
   * Create or update a saved connection.
   *
   * Pass a secret as `undefined` to leave its existing keychain entry alone —
   * that is what lets an edit dialog save without forcing the user to retype a
   * password it never displayed. Pass `""` to explicitly clear it.
   *
   * `ssh_secret` is the SSH password or key passphrase, stored under a separate
   * keychain entry so saving one credential never overwrites the other.
   */
  saveConnection: (config: ConnectionConfig, secret?: string, sshSecret?: string) =>
    call<void>("save_connection", {
      config,
      secret: secret ?? null,
      ssh_secret: sshSecret ?? null,
    }),

  deleteConnection: (id: string) => call<void>("delete_connection", { id }),

  connect: (id: string) => call<void>("connect", { id }),
  disconnect: (id: string) => call<void>("disconnect", { id }),

  /** Validate a config that may not be saved yet, tunnel included. */
  testConnection: (config: ConnectionConfig, secret?: string, sshSecret?: string) =>
    call<void>("test_connection", {
      config,
      secret: secret ?? null,
      ssh_secret: sshSecret ?? null,
    }),

  execute: (request: {
    connection_id: string;
    sql: string;
    max_rows?: number;
    offset?: number;
    timeout_secs?: number;
  }) => call<QueryOutcome>("execute", { request }),

  browse: (connection_id: string, parent?: string) =>
    call<SchemaNode[]>("browse", { connection_id, parent: parent ?? null }),

  tableDetail: (connection_id: string, table: string, schema?: string) =>
    call<TableDetail>("table_detail", { connection_id, table, schema: schema ?? null }),

  /**
   * The statement that would recreate an object, for viewing and editing.
   *
   * Takes the tree node's id: the driver built that path and knows which
   * catalogue to ask.
   */
  objectDefinition: (connection_id: string, node_id: string) =>
    call<string>("object_definition", { connection_id, node_id }),

  /**
   * Write a table to a file, returning how many rows were written.
   *
   * The path is chosen by the frontend's save dialog and the file is written in
   * Rust: the webview has no filesystem access of its own, and should not.
   */
  exportTable: (args: {
    /** Identifies this export in progress events and to `cancelExport`. */
    id: string;
    connection_id: string;
    qualified: string;
    schema?: string | undefined;
    table: string;
    format: ExportFormat;
    path: string;
  }) =>
    call<number>("export_table", {
      request: {
        id: args.id,
        connection_id: args.connection_id,
        qualified: args.qualified,
        schema: args.schema ?? null,
        table: args.table,
        format: args.format,
        path: args.path,
      },
    }),

  /** Dump a whole database to one SQL file, returning rows written. */
  exportDatabase: (args: {
    id: string;
    connection_id: string;
    database: string;
    path: string;
  }) => call<number>("export_database", { request: args }),

  /** Run every statement in a SQL file, returning how many were applied. */
  importSql: (args: { id: string; connection_id: string; path: string }) =>
    call<number>("import_sql", { request: args }),

  /** Ask a running export or import to stop. */
  cancelExport: (id: string) => call<void>("cancel_export", { id }),

  applyEdit: (connection_id: string, edit: RowEdit) =>
    call<void>("apply_edit", { connection_id, edit }),

  completionScope: (connection_id: string) =>
    call<CompletionScope>("completion_scope", { connection_id }),

  /** What database a live session is pointed at. */
  sessionInfo: (connection_id: string) => call<SessionInfo>("session_info", { connection_id }),

  /**
   * Point a session at another database, returning the one now in force.
   *
   * PostgreSQL reconnects behind this call, since a connection there is bound
   * to one database for its lifetime; the other engines switch in place.
   */
  useDatabase: (connection_id: string, database: string) =>
    call<string>("use_database", { connection_id, database }),

  /**
   * Read an SSH server's host key fingerprint for the user to confirm.
   *
   * Must be done before a tunnelled connection can be opened: `connect` refuses
   * to tunnel to a host whose key is not already known.
   */
  sshHostFingerprint: (ssh: SshConfig) => call<string>("ssh_host_fingerprint", { ssh }),

  /** Saved queries, most recently edited first. */
  listSnippets: () => call<Snippet[]>("list_snippets"),

  /**
   * Create or update a saved query, returning it as stored.
   *
   * The store owns the timestamps and the trimmed name, so the answer is what
   * to display — not the object that was sent.
   */
  saveSnippet: (snippet: Snippet) => call<Snippet>("save_snippet", { snippet }),

  deleteSnippet: (id: string) => call<void>("delete_snippet", { id }),

  /** Search executed statements, newest first. */
  queryHistory: (query: HistoryQuery) =>
    call<HistoryEntry[]>("query_history", {
      query: {
        connection_id: query.connection_id ?? null,
        text: query.text ?? null,
        limit: query.limit ?? null,
      },
    }),

  /** Forget history for one connection, or all of it when `id` is omitted. */
  clearQueryHistory: (connectionId?: string) =>
    call<void>("clear_query_history", { connection_id: connectionId ?? null }),
};

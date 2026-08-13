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
  QueryOutcome,
  RowEdit,
  SchemaNode,
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
   * Pass `secret: undefined` to leave any existing keychain entry alone — that is
   * what lets an edit dialog save without forcing the user to retype a password
   * it never displayed. Pass `""` to explicitly clear the stored secret.
   */
  saveConnection: (config: ConnectionConfig, secret?: string) =>
    call<void>("save_connection", { config, secret: secret ?? null }),

  deleteConnection: (id: string) => call<void>("delete_connection", { id }),

  connect: (id: string) => call<void>("connect", { id }),
  disconnect: (id: string) => call<void>("disconnect", { id }),

  /** Validate a config that may not be saved yet. */
  testConnection: (config: ConnectionConfig, secret?: string) =>
    call<void>("test_connection", { config, secret: secret ?? null }),

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

  applyEdit: (connection_id: string, edit: RowEdit) =>
    call<void>("apply_edit", { connection_id, edit }),

  completionScope: (connection_id: string) =>
    call<CompletionScope>("completion_scope", { connection_id }),

  /**
   * Read an SSH server's host key fingerprint for the user to confirm.
   *
   * Must be done before a tunnelled connection can be opened: `connect` refuses
   * to tunnel to a host whose key is not already known.
   */
  sshHostFingerprint: (ssh: SshConfig) => call<string>("ssh_host_fingerprint", { ssh }),
};

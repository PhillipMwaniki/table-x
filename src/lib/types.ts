/**
 * TypeScript mirrors of the Rust types crossing the IPC boundary.
 *
 * Naming: everything is `snake_case`, matching serde's default for the Rust
 * structs. The Tauri commands are declared `rename_all = "snake_case"` so
 * argument names follow the same convention — one rule for the whole surface
 * rather than camelCase arguments wrapping snake_case payloads.
 *
 * These are maintained by hand. They are small, they change rarely, and the
 * alternative (a codegen step) would add a build dependency for less benefit
 * than it costs here. The IPC integration tests are what catch drift.
 */

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/** Coarse classification, used to pick alignment and colour in the grid. */
export type ValueKind =
  | "null"
  | "bool"
  | "integer"
  | "number"
  | "text"
  | "binary"
  | "uuid"
  | "temporal"
  | "interval"
  | "json"
  | "array"
  | "unknown";

/**
 * A single cell. Serialized by serde as an internally tagged enum:
 * `{ kind: "int", value: 42 }`, with `{ kind: "null" }` carrying no value.
 */
export type Value =
  | { kind: "null" }
  | { kind: "bool"; value: boolean }
  | { kind: "int"; value: number }
  | { kind: "u_int"; value: number }
  | { kind: "float"; value: number }
  /** Exact numerics travel as strings so no digits are lost in JSON. */
  | { kind: "numeric"; value: string }
  | { kind: "text"; value: string }
  | { kind: "bytes"; value: number[] }
  | { kind: "uuid"; value: string }
  | { kind: "date"; value: string }
  | { kind: "time"; value: string }
  | { kind: "date_time"; value: string }
  | { kind: "timestamp_tz"; value: string }
  | { kind: "interval"; value: { months: number; days: number; micros: number } }
  | { kind: "json"; value: unknown }
  | { kind: "array"; value: Value[] }
  | { kind: "unsupported"; value: { type_name: string; raw: string } };

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export type ErrorCategory =
  | "connection"
  | "auth"
  | "query"
  | "cancelled"
  | "timeout"
  | "unsupported"
  | "config"
  | "internal";

/** The single error shape every command rejects with. */
export interface ErrorPayload {
  message: string;
  category: ErrorCategory;
  retryable: boolean;
  /** 1-based character offset into the statement, for editor underlining. */
  position?: number | undefined;
  /** Vendor error code, e.g. PostgreSQL SQLSTATE. */
  code?: string | undefined;
}

// ---------------------------------------------------------------------------
// Drivers
// ---------------------------------------------------------------------------

export type PlaceholderStyle = "question" | "dollar" | "at_p";

export interface Capabilities {
  transactions: boolean;
  cancel: boolean;
  multi_statement: boolean;
  explain: boolean;
  schemas: boolean;
  /** Whether one server holds several databases the user can move between. */
  databases: boolean;
  foreign_keys: boolean;
  views: boolean;
  stored_procedures: boolean;
  /** Whether a table's CREATE statement can be shown. Not every engine has one. */
  table_scripts: boolean;
  column_provenance: boolean;
  streaming: boolean;
  placeholder_style: PlaceholderStyle;
  identifier_quote: string;
}

export interface DriverInfo {
  id: string;
  name: string;
  default_port: number | null;
  /** Embedded databases take a file path instead of host/port. */
  file_based: boolean;
  capabilities: Capabilities;
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

export type TlsMode = "disable" | "prefer" | "verify_full";
export type SshAuth = "password" | "public_key" | "agent";

export interface TlsConfig {
  mode: TlsMode;
  ca_cert_path?: string | undefined;
  client_cert_path?: string | undefined;
  client_key_path?: string | undefined;
}

export interface SshConfig {
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
  /** Private key path for `public_key` auth. Not a secret; the passphrase is. */
  key_path?: string | undefined;
  /**
   * Expected host key, in OpenSSH `SHA256:...` form. Required before a tunnel
   * will open — there is no trust-on-first-use, since an unverified tunnel
   * gives no protection against the attacker a bastion exists to defend against.
   */
  host_key_fingerprint?: string | undefined;
}

/**
 * A saved connection. Deliberately has no password field — secrets live in the
 * OS keychain and are passed to `save_connection` as a separate argument.
 */
export interface ConnectionConfig {
  id: string;
  name: string;
  driver: string;
  host?: string | undefined;
  port?: number | undefined;
  database?: string | undefined;
  username?: string | undefined;
  file_path?: string | undefined;
  tls: TlsConfig;
  ssh?: SshConfig | undefined;
  /** Sidebar group. One flat level, absent for connections filed under none. */
  folder?: string | undefined;
  color?: string | undefined;
  read_only: boolean;
  options: Record<string, string>;
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

export interface ColumnSource {
  schema?: string | undefined;
  table: string;
  column: string;
}

export interface Column {
  name: string;
  type_name: string;
  nullable?: boolean | undefined;
  source?: ColumnSource | undefined;
}

export interface ResultSet {
  columns: Column[];
  rows: Value[][];
  /** More rows exist beyond the fetched page. */
  truncated: boolean;
  editable: boolean;
  key_columns: string[];
}

export type StatementResult =
  | ({ type: "rows" } & ResultSet)
  | { type: "affected"; rows_affected: number; last_insert_id?: number };

export interface QueryOutcome {
  statements: StatementResult[];
  elapsed_ms: number;
  notices: string[];
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

export type NodeKind =
  | "database"
  | "schema"
  /** A grouping level in the tree — "Tables", "Views" — not a database object. */
  | "folder"
  | "table"
  | "view"
  | "materialized_view"
  | "column"
  | "index"
  | "foreign_key"
  | "function"
  | "procedure"
  | "trigger"
  | "sequence"
  | "enum"
  | "collection";

export interface SchemaNode {
  /** Encoded path; opaque to the frontend, and the argument for browsing deeper. */
  id: string;
  name: string;
  kind: NodeKind;
  /** Can be expanded. Distinct from "has zero children" — unknown until asked. */
  expandable: boolean;
  children?: SchemaNode[] | undefined;
  detail?: string | undefined;
  /**
   * The object's name as SQL should refer to it, quoted and qualified by the
   * driver. Present on objects that can be queried; absent on folders.
   */
  qualified?: string | undefined;
}

/** File formats a table can be written out as. */
export type ExportFormat = "csv" | "json" | "sql";

/** The first rows of a delimited file, for mapping its columns. */
export interface CsvPreview {
  delimiter: string;
  rows: string[][];
}

/** A query the user chose to keep. */
export interface Snippet {
  id: string;
  name: string;
  sql: string;
  /** RFC 3339, UTC. Set when first saved and preserved across edits. */
  created_at: string;
  updated_at: string;
  folder?: string | undefined;
}

/** What a live session is pointed at, beyond being open. */
export interface SessionInfo {
  database?: string | undefined;
}

export interface ColumnDef {
  name: string;
  type_name: string;
  nullable: boolean;
  default?: string | undefined;
  auto_increment: boolean;
  ordinal: number;
  comment?: string | undefined;
}

export interface IndexDef {
  name: string;
  columns: string[];
  unique: boolean;
  primary: boolean;
  method?: string | undefined;
}

export interface ForeignKeyDef {
  name: string;
  columns: string[];
  referenced_schema?: string | undefined;
  referenced_table: string;
  referenced_columns: string[];
  on_delete?: string | undefined;
  on_update?: string | undefined;
}

export interface TableDetail {
  schema?: string | undefined;
  name: string;
  columns: ColumnDef[];
  indexes: IndexDef[];
  foreign_keys: ForeignKeyDef[];
  primary_key: string[];
  /** Planner estimate, not an exact count. */
  estimated_rows?: number | undefined;
  comment?: string | undefined;
}

export interface CompletionScope {
  schemas: string[];
  /** Qualified table name paired with its column names. */
  tables: [string, string[]][];
  functions: string[];
  keywords: string[];
}

export interface RowEdit {
  schema?: string | undefined;
  table: string;
  changes: [string, Value][];
  key: [string, Value][];
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/**
 * One executed submission. The connection's name and driver are copied in at
 * record time, so an entry still says where it ran after the connection itself
 * has been deleted.
 */
export interface HistoryEntry {
  id: string;
  connection_id: string;
  connection_name: string;
  driver: string;
  sql: string;
  /** RFC 3339, UTC. */
  ran_at: string;
  elapsed_ms: number;
  /** Rows returned or affected. Absent when the statement failed. */
  rows?: number | undefined;
  succeeded: boolean;
  error?: string | undefined;
}

export interface HistoryQuery {
  /** Omit to search every connection. */
  connection_id?: string | undefined;
  /** Whitespace-separated terms; an entry must contain all of them. */
  text?: string | undefined;
  limit?: number | undefined;
}

export interface BackendInfo {
  version: string;
  drivers: string[];
}

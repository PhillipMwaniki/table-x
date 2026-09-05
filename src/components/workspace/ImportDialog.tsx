/**
 * A file's rows mapped onto a table — or onto a table that does not exist
 * yet — before anything is written.
 *
 * The preview is the point. An import that starts from a file picker and
 * reports "4,812 rows" afterwards gives you no way to notice that every column
 * shifted by one — which is exactly what a stray delimiter does. Seeing the
 * first rows under the columns they will land in catches it before it happens.
 *
 * Two targets, one dialog. Into an existing table, each file column picks the
 * table column it lands in. Into a new table, each file column is a column to
 * be, with a name and a type guessed from the values and open to correction:
 * the guess is shown beside the values it was made from, which is the only
 * place a guess can be judged.
 */

import { useEffect, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Banner, Button, Checkbox, Field, Input, Select } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { ColumnDef, ImportPreview, ImportSource } from "@/lib/types";

/** Shown in the delimiter picker; the file is sniffed for one first. */
const DELIMITERS = [
  { value: ",", label: "Comma" },
  { value: ";", label: "Semicolon" },
  { value: "\t", label: "Tab" },
  { value: "|", label: "Pipe" },
];

const SKIP = " skip";

/** Where the rows are going. */
export type ImportTarget =
  | {
      kind: "existing";
      table: string;
      schema?: string | undefined;
      qualified: string;
      columns: ColumnDef[];
    }
  | { kind: "new"; schema?: string | undefined };

/** What the dialog settles, for the import to run. */
export interface ImportPlan {
  source: ImportSource;
  hasHeader: boolean;
  nullAsEmpty: boolean;
  /** Target column per file column; null skips it. */
  mapping: (string | null)[];
  /** The table's name — the existing one, or the one to create. */
  table: string;
  /** Columns to create first, when the target is new. */
  create?: { name: string; type_name: string }[] | undefined;
}

/** A column of the table to be, as the dialog holds it. */
interface Draft {
  include: boolean;
  name: string;
  typeName: string;
}

/** Whether a path names a workbook rather than delimited text. */
function sourceFor(path: string): ImportSource {
  return /\.(xlsx|xlsm|xls|ods)$/i.test(path) ? { kind: "sheet" } : { kind: "csv" };
}

/** A table name to suggest from a file, or a worksheet: the stem, made plain. */
function suggestedName(path: string, sheet: string | null): string {
  const stem = path
    .split(/[\\/]/)
    .pop()!
    .replace(/\.[^.]+$/, "");
  const base = sheet && !/^(sheet|rows)\s*\d*$/i.test(sheet) ? sheet : stem;
  const plain = base
    .trim()
    .replace(/[^\p{L}\p{N}]+/gu, "_")
    .replace(/^_+|_+$/g, "")
    .toLowerCase();
  return plain || "imported";
}

export function ImportDialog({
  open,
  path,
  connectionId,
  target,
  onClose,
  onImport,
}: {
  open: boolean;
  /** The file chosen before this dialog opened. */
  path: string;
  connectionId: string;
  target: ImportTarget;
  onClose: () => void;
  onImport: (plan: ImportPlan) => void;
}) {
  const [source, setSource] = useState<ImportSource>(() => sourceFor(path));
  const [hasHeader, setHasHeader] = useState(true);
  const [nullAsEmpty, setNullAsEmpty] = useState(true);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [mapping, setMapping] = useState<(string | null)[]>([]);
  const [drafts, setDrafts] = useState<Draft[]>([]);
  const [tableName, setTableName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Re-read whenever the file, the source or the header changes: changing the
  // delimiter is how someone fixes a preview that came out wrong, so it has to
  // re-parse, and the type guesses depend on which row is the header.
  useEffect(() => {
    if (!open || !path) return;
    let cancelled = false;
    setLoading(true);
    setError(null);

    ipc
      .previewImport(connectionId, path, source, hasHeader)
      .then((next) => {
        if (cancelled) return;
        setPreview(next);
        setLoading(false);
        // The guesses become the draft table, fresh each time the sample
        // changes shape; a name typed over a guess survives only until the
        // header or the sheet is changed, which is when it stops applying.
        setDrafts(
          next.columns.map((c) => ({ include: true, name: c.name, typeName: c.type_name })),
        );
        setTableName((was) => was || suggestedName(path, next.sheet));
        // Settle the source on what was actually used, so the picker agrees
        // with the preview.
        if (next.delimiter && source.kind === "csv" && source.delimiter !== next.delimiter) {
          setSource({ kind: "csv", delimiter: next.delimiter });
        } else if (next.sheet && source.kind === "sheet" && source.name !== next.sheet) {
          setSource({ kind: "sheet", name: next.sheet });
        }
      })
      .catch((e) => {
        if (cancelled) return;
        setError((e as IpcError).message);
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open, path, connectionId, source, hasHeader]);

  // Into an existing table: map by name, case-insensitively, whenever the
  // header changes meaning.
  useEffect(() => {
    if (target.kind !== "existing") return;
    const header = preview?.rows[0];
    if (!header) return;
    setMapping(
      header.map((cell, index) => {
        if (!hasHeader) {
          // No header: line the file up positionally, which is what a file
          // exported from this same table looks like.
          return target.columns[index]?.name ?? null;
        }
        const wanted = cell.trim().toLowerCase();
        return target.columns.find((c) => c.name.toLowerCase() === wanted)?.name ?? null;
      }),
    );
  }, [preview, hasHeader, target]);

  const rows = preview?.rows ?? [];
  const dataRows = hasHeader ? rows.slice(1) : rows;
  const width = Math.max(rows[0]?.length ?? 0, preview?.columns.length ?? 0);

  const mappedCount = mapping.filter(Boolean).length;
  const included = drafts.filter((d) => d.include);
  const duplicateNames = new Set(
    included
      .map((d) => d.name.trim().toLowerCase())
      .filter((n, i, all) => n && all.indexOf(n) !== i),
  );
  const newProblem =
    target.kind === "new"
      ? !tableName.trim()
        ? "Name the table"
        : included.length === 0
          ? "Include at least one column"
          : included.some((d) => !d.name.trim())
            ? "Every included column needs a name"
            : included.some((d) => !d.typeName.trim())
              ? "Every included column needs a type"
              : duplicateNames.size > 0
                ? "Two columns share a name"
                : null
      : null;
  const ready =
    !loading && !error && (target.kind === "existing" ? mappedCount > 0 : newProblem === null);

  const unmatched =
    target.kind === "existing" && hasHeader
      ? (rows[0] ?? []).filter((_, i) => mapping[i] == null).length
      : 0;

  const submit = () => {
    if (target.kind === "existing") {
      onImport({ source, hasHeader, nullAsEmpty, mapping, table: target.table });
      return;
    }
    onImport({
      source,
      hasHeader,
      nullAsEmpty,
      mapping: drafts.map((d) => (d.include ? d.name.trim() : null)),
      table: tableName.trim(),
      create: included.map((d) => ({ name: d.name.trim(), type_name: d.typeName.trim() })),
    });
  };

  const fileName = path.split(/[\\/]/).pop();

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={target.kind === "existing" ? `Import into ${target.table}` : "Import as a new table"}
      description={
        target.kind === "existing"
          ? "Rows are appended. Nothing is emptied or replaced."
          : "The table is created with the columns below, then the rows go in. Types are guessed from the values and can be changed."
      }
      width="wide"
      footer={
        <div className="flex items-center gap-2">
          <span className="text-[11px] text-text-muted">
            {target.kind === "existing"
              ? `${mappedCount} of ${mapping.length} file columns mapped`
              : (newProblem ?? `${included.length} of ${drafts.length} columns included`)}
          </span>
          <div className="flex-1" />
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!ready} onClick={submit}>
            {target.kind === "existing" ? "Import" : "Create and import"}
          </Button>
        </div>
      }
    >
      <div className="space-y-3">
        {error && <Banner tone="error">{error}</Banner>}

        {unmatched > 0 && (
          <Banner tone="info">
            {unmatched} file column{unmatched === 1 ? "" : "s"} did not match a column by name and
            will be skipped unless you map {unmatched === 1 ? "it" : "them"} below.
          </Banner>
        )}

        <div className="grid grid-cols-2 gap-3">
          {source.kind === "csv" ? (
            <Field
              label="Delimiter"
              hint="Detected from the file; change it if the preview looks wrong."
            >
              <Select
                value={source.delimiter ?? ","}
                onChange={(e) => setSource({ kind: "csv", delimiter: e.target.value })}
              >
                {DELIMITERS.map((d) => (
                  <option key={d.value} value={d.value}>
                    {d.label}
                  </option>
                ))}
              </Select>
            </Field>
          ) : (
            <Field label="Worksheet" hint="One sheet per import.">
              <Select
                value={source.name ?? preview?.sheet ?? ""}
                onChange={(e) => setSource({ kind: "sheet", name: e.target.value })}
              >
                {(preview?.sheets ?? []).map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </Select>
            </Field>
          )}

          <div className="flex flex-col justify-end gap-2 pb-1">
            <Checkbox label="First row is a header" checked={hasHeader} onChange={setHasHeader} />
            <Checkbox
              label="Empty fields are NULL"
              hint="Otherwise they import as an empty string — different values, and a file cannot tell them apart."
              checked={nullAsEmpty}
              onChange={setNullAsEmpty}
            />
          </div>
        </div>

        {target.kind === "new" && (
          <Field
            label="Table name"
            hint={
              target.schema
                ? `Created in ${target.schema}. Quoted as written, so the case is kept.`
                : "Quoted as written, so the case is kept."
            }
          >
            <Input
              value={tableName}
              spellCheck={false}
              onChange={(e) => setTableName(e.target.value)}
              className="font-mono"
            />
          </Field>
        )}

        <div className="overflow-x-auto rounded-md border border-border">
          <table className="w-full border-collapse text-[11px]">
            <thead>
              <tr>
                {Array.from({ length: width }, (_, index) => (
                  <th
                    key={index}
                    className="min-w-36 border-b border-border bg-surface-2 p-1 text-left align-top"
                  >
                    {target.kind === "existing" ? (
                      <Select
                        value={mapping[index] ?? SKIP}
                        onChange={(e) =>
                          setMapping((was) => {
                            const next = [...was];
                            next[index] = e.target.value === SKIP ? null : e.target.value;
                            return next;
                          })
                        }
                        className="h-6"
                      >
                        <option value={SKIP}>— skip —</option>
                        {target.columns.map((c) => (
                          <option key={c.name} value={c.name}>
                            {c.name}
                          </option>
                        ))}
                      </Select>
                    ) : (
                      <NewColumnHead
                        draft={drafts[index] ?? { include: false, name: "", typeName: "" }}
                        duplicate={duplicateNames.has(
                          (drafts[index]?.name ?? "").trim().toLowerCase(),
                        )}
                        onChange={(change) =>
                          setDrafts((was) => {
                            const next = [...was];
                            next[index] = { ...(next[index] ?? drafts[index]!), ...change };
                            return next;
                          })
                        }
                      />
                    )}
                    {hasHeader && (
                      <span className="mt-0.5 block truncate px-1 font-mono text-[10px] text-text-muted">
                        {rows[0]?.[index]}
                      </span>
                    )}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {dataRows.slice(0, 8).map((row, r) => (
                <tr key={r}>
                  {Array.from({ length: width }, (_, c) => {
                    const kept =
                      target.kind === "existing" ? Boolean(mapping[c]) : drafts[c]?.include;
                    return (
                      <td
                        key={c}
                        className={
                          // A skipped column is shown greyed rather than hidden:
                          // seeing what is being dropped is the point of a preview.
                          kept
                            ? "border-b border-border/50 px-1.5 py-0.5 font-mono text-text"
                            : "border-b border-border/50 px-1.5 py-0.5 font-mono text-text-muted/40 line-through"
                        }
                      >
                        {row[c] ?? ""}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <p className="text-[11px] text-text-muted">
          {loading
            ? "Reading the file…"
            : `Previewing the first ${dataRows.length} rows of ${fileName}${
                preview?.sheet ? `, sheet ${preview.sheet}` : ""
              }. Types were guessed from up to a thousand rows.`}
        </p>
      </div>
    </Dialog>
  );
}

/** One column of the table to be: whether, what it is called, and what it is. */
function NewColumnHead({
  draft,
  duplicate,
  onChange,
}: {
  draft: Draft;
  duplicate: boolean;
  onChange: (change: Partial<Draft>) => void;
}) {
  return (
    <div className="flex flex-col gap-1">
      <label className="flex items-center gap-1.5">
        <input
          type="checkbox"
          checked={draft.include}
          onChange={(e) => onChange({ include: e.target.checked })}
          className="size-3.5 shrink-0 accent-[var(--color-accent)]"
          aria-label="Include this column"
        />
        <Input
          value={draft.name}
          disabled={!draft.include}
          spellCheck={false}
          onChange={(e) => onChange({ name: e.target.value })}
          aria-label="Column name"
          className={duplicate ? "h-6 font-mono border-danger" : "h-6 font-mono"}
        />
      </label>
      <Input
        value={draft.typeName}
        disabled={!draft.include}
        spellCheck={false}
        onChange={(e) => onChange({ typeName: e.target.value })}
        aria-label="Column type"
        title="The engine's own type name. Guessed from the values; change it if the guess is wrong."
        className="h-6 font-mono text-text-muted"
      />
    </div>
  );
}

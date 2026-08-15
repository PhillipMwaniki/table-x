/**
 * Choosing what to compare a schema against.
 *
 * Only connected connections are offered. Comparing against one that is closed
 * would mean opening it — prompting for a password, possibly a host key — from
 * inside a dialog that is about something else, so the list says what it can
 * reach and the user connects first if they want more.
 */

import { useEffect, useMemo, useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Banner, Button, Field, Select } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { ConnectionConfig, SchemaNode } from "@/lib/types";

export function CompareDialog({
  open,
  from,
  connections,
  connected,
  onClose,
  onCompare,
}: {
  open: boolean;
  /** The side the script would be run against. */
  from: { connectionId: string; schema: string | null; label: string };
  connections: ConnectionConfig[];
  /** Ids with a live session — the only ones that can be read. */
  connected: ReadonlySet<string>;
  onClose: () => void;
  onCompare: (to: { connectionId: string; schema: string | null; label: string }) => void;
}) {
  const reachable = useMemo(
    () => connections.filter((c) => connected.has(c.id)),
    [connections, connected],
  );

  const [connectionId, setConnectionId] = useState(from.connectionId);
  const [schema, setSchema] = useState<string>("");
  const [schemas, setSchemas] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Whatever the chosen connection calls its top level: schemas on the engines
  // that have them, databases on the ones that do not.
  useEffect(() => {
    if (!open || !connectionId) return;
    let cancelled = false;

    ipc
      .browse(connectionId)
      .then(async (roots) => {
        if (cancelled) return;
        const named = (nodes: SchemaNode[]) =>
          nodes.filter((n) => n.kind === "schema" || n.kind === "database").map((n) => n.name);

        let names = named(roots);
        // A database-per-server engine lists databases at the root and schemas
        // one level in; either is a level worth comparing, so the deeper one
        // wins when it exists.
        if (names.length === 1 && roots[0]) {
          const inner = await ipc.browse(connectionId, roots[0].id).catch(() => []);
          const deeper = named(inner);
          if (deeper.length > 0) names = deeper;
        }
        if (cancelled) return;
        setSchemas(names);
        setSchema((was) => (names.includes(was) ? was : (names[0] ?? "")));
        setError(null);
      })
      .catch((e) => {
        if (cancelled) return;
        setError((e as IpcError).message);
      });

    return () => {
      cancelled = true;
    };
  }, [open, connectionId]);

  const target = connections.find((c) => c.id === connectionId);
  const sameSide = connectionId === from.connectionId && schema === (from.schema ?? "");

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Compare schema"
      description={`Generates the statements that would turn ${from.label} into whatever you pick.`}
      footer={
        <div className="flex items-center gap-2">
          <div className="flex-1" />
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={sameSide || !connectionId}
            onClick={() =>
              onCompare({
                connectionId,
                schema: schema || null,
                label: `${target?.name ?? connectionId}${schema ? ` · ${schema}` : ""}`,
              })
            }
          >
            Compare
          </Button>
        </div>
      }
    >
      <div className="space-y-3">
        {error && <Banner tone="error">{error}</Banner>}

        <div className="rounded-md border border-border bg-surface-2 px-2 py-1.5 text-[11px]">
          <span className="text-text-muted">Changes will be written for</span>{" "}
          <span className="font-medium text-text">{from.label}</span>
        </div>

        <Field
          label="Compare against"
          hint={
            reachable.length < connections.length
              ? "Only connected connections are listed — connect another to compare against it."
              : undefined
          }
        >
          <Select value={connectionId} onChange={(e) => setConnectionId(e.target.value)}>
            {reachable.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </Select>
        </Field>

        {schemas.length > 0 && (
          <Field label="Schema">
            <Select value={schema} onChange={(e) => setSchema(e.target.value)}>
              {schemas.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </Select>
          </Field>
        )}

        {sameSide && (
          <Banner tone="info">That is the same schema — pick another side to compare with.</Banner>
        )}
      </div>
    </Dialog>
  );
}

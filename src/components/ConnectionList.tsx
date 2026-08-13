/**
 * The connection sidebar.
 */

import { useState } from "react";
import { Button, Spinner, cx } from "./ui/primitives";
import { Dialog } from "./ui/Dialog";
import { useConnections } from "@/store/connections";
import type { ConnectionConfig } from "@/lib/types";

/** `postgres@host:5432/db`, or the file path for embedded databases. */
function summarize(c: ConnectionConfig): string {
  if (c.file_path) return c.file_path;
  const user = c.username ? `${c.username}@` : "";
  const host = c.host ?? "localhost";
  const port = c.port ? `:${c.port}` : "";
  const db = c.database ? `/${c.database}` : "";
  return `${user}${host}${port}${db}`;
}

export function ConnectionList({
  onEdit,
  onNew,
}: {
  onEdit: (config: ConnectionConfig) => void;
  onNew: () => void;
}) {
  const { connections, open, busy, selectedId, select, connect, disconnect, remove } =
    useConnections();
  const [confirmDelete, setConfirmDelete] = useState<ConnectionConfig | null>(null);
  const [deleting, setDeleting] = useState(false);

  return (
    <>
      <div className="flex h-full flex-col">
        <div className="flex h-8 shrink-0 items-center justify-between px-2">
          <span className="text-[11px] font-semibold tracking-wide text-text-muted uppercase">
            Connections
          </span>
          <Button variant="ghost" onClick={onNew} title="New connection" className="h-6 px-1.5">
            +
          </Button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-2">
          {connections.length === 0 ? (
            <p className="px-1.5 py-6 text-center text-[12px] text-text-muted">
              No connections yet.
              <br />
              <button onClick={onNew} className="mt-1 text-accent hover:underline">
                Add one
              </button>
            </p>
          ) : (
            <ul className="space-y-px">
              {connections.map((c) => {
                const isOpen = open.has(c.id);
                const isBusy = busy.has(c.id);
                const isSelected = selectedId === c.id;
                return (
                  <li key={c.id}>
                    <div
                      role="button"
                      tabIndex={0}
                      onClick={() => select(c.id)}
                      // Double-click is the familiar "open this" gesture in every
                      // database client.
                      onDoubleClick={() => (isOpen ? undefined : connect(c.id))}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          select(c.id);
                        }
                      }}
                      className={cx(
                        "group flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left",
                        isSelected ? "bg-surface-3" : "hover:bg-surface-2",
                      )}
                    >
                      {/* Status dot: the colour tag when set, otherwise open state.
                          A loud red on production is a cheap guard against
                          running the wrong statement. */}
                      <span
                        aria-hidden
                        className={cx(
                          "size-1.5 shrink-0 rounded-full",
                          !c.color && (isOpen ? "bg-ok" : "bg-border"),
                        )}
                        style={c.color ? { backgroundColor: c.color } : undefined}
                      />

                      <span className="min-w-0 flex-1">
                        <span className="flex items-center gap-1.5">
                          <span className="truncate text-[12px] text-text">{c.name}</span>
                          {c.read_only && (
                            <span
                              title="Read-only"
                              className="shrink-0 rounded bg-surface-3 px-1 text-[9px] font-medium tracking-wide text-text-muted uppercase"
                            >
                              RO
                            </span>
                          )}
                        </span>
                        <span className="block truncate font-mono text-[10px] text-text-muted">
                          {summarize(c)}
                        </span>
                      </span>

                      {isBusy ? (
                        <Spinner className="text-text-muted" />
                      ) : (
                        <span className="flex shrink-0 items-center opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
                          <IconButton
                            label={isOpen ? "Disconnect" : "Connect"}
                            onClick={() => (isOpen ? disconnect(c.id) : connect(c.id))}
                          >
                            {isOpen ? "◼" : "▶"}
                          </IconButton>
                          <IconButton label="Edit" onClick={() => onEdit(c)}>
                            ✎
                          </IconButton>
                          <IconButton label="Delete" onClick={() => setConfirmDelete(c)}>
                            ✕
                          </IconButton>
                        </span>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </div>

      <Dialog
        open={confirmDelete !== null}
        onClose={() => setConfirmDelete(null)}
        title="Delete connection"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setConfirmDelete(null)} disabled={deleting}>
              Cancel
            </Button>
            <Button
              variant="danger"
              busy={deleting}
              onClick={async () => {
                if (!confirmDelete) return;
                setDeleting(true);
                try {
                  await remove(confirmDelete.id);
                  setConfirmDelete(null);
                } finally {
                  setDeleting(false);
                }
              }}
            >
              Delete
            </Button>
          </div>
        }
      >
        <p className="text-[12px] text-text">
          Delete <strong>{confirmDelete?.name}</strong> and its saved credentials?
        </p>
        {/* Saying plainly what is and is not destroyed — people hesitate here. */}
        <p className="mt-2 text-[11px] text-text-muted">
          This removes the connection from this app only. The database itself is not
          affected.
        </p>
      </Dialog>
    </>
  );
}

function IconButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      title={label}
      aria-label={label}
      onClick={(e) => {
        // The row itself is clickable; without this the action also selects.
        e.stopPropagation();
        onClick();
      }}
      className="flex size-5 items-center justify-center rounded text-[10px] text-text-muted hover:bg-surface-0 hover:text-text"
    >
      {children}
    </button>
  );
}

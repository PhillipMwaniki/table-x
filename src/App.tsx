/**
 * Application shell.
 *
 * The sidebar is real; the main pane is a placeholder until the schema browser,
 * editor, and result grid land.
 */

import { useEffect, useState } from "react";
import { ConnectionList } from "./components/ConnectionList";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { Workspace } from "./components/workspace/Workspace";
import { Banner, Button, Spinner } from "./components/ui/primitives";
import { useConnections } from "./store/connections";
import type { ConnectionConfig } from "./lib/types";

export default function App() {
  const {
    drivers,
    connections,
    open,
    busy,
    selectedId,
    loading,
    error,
    init,
    save,
    connect,
    clearError,
  } = useConnections();

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<ConnectionConfig | null>(null);

  useEffect(() => {
    void init();
  }, [init]);

  const selected = connections.find((c) => c.id === selectedId) ?? null;

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text">
      <header className="drag-region flex h-9 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-3">
        <span className="text-[12px] font-semibold tracking-wide">Table X</span>
        <span className="text-[11px] text-text-muted">
          {open.size > 0 && `${open.size} connected`}
        </span>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-60 shrink-0 flex-col border-r border-border bg-surface-1">
          {loading ? (
            <div className="flex flex-1 items-center justify-center">
              <Spinner className="text-text-muted" />
            </div>
          ) : (
            <ConnectionList
              onNew={() => {
                setEditing(null);
                setDialogOpen(true);
              }}
              onEdit={(config) => {
                setEditing(config);
                setDialogOpen(true);
              }}
            />
          )}
        </aside>

        <main className="flex min-w-0 flex-1 flex-col">
          {error && (
            <div className="shrink-0 p-2">
              <Banner tone="error" onDismiss={clearError}>
                {error}
              </Banner>
            </div>
          )}

          {selected && open.has(selected.id) ? (
            // Keyed by connection so switching rebuilds the schema tree rather
            // than showing the previous connection's objects against the new one.
            <Workspace
              key={selected.id}
              connection={selected}
              driver={drivers.find((d) => d.id === selected.driver)}
            />
          ) : (
            <div className="flex flex-1 items-center justify-center p-6">
              {selected ? (
                <div className="text-center">
                  <h2 className="text-[13px] font-semibold">{selected.name}</h2>
                  <p className="mt-1 font-mono text-[11px] text-text-muted">{selected.driver}</p>
                  <p className="mt-4 max-w-sm text-[12px] text-text-muted">
                    Not connected yet.
                  </p>
                  <Button
                    variant="primary"
                    className="mt-3"
                    busy={busy.has(selected.id)}
                    onClick={() => void connect(selected.id)}
                  >
                    Connect
                  </Button>
                </div>
              ) : (
                <div className="text-center">
                  <h2 className="text-[13px] font-semibold">No connection selected</h2>
                  <p className="mt-1 max-w-sm text-[12px] text-text-muted">
                    Select a connection from the sidebar, or create one to get started.
                  </p>
                  <Button
                    variant="primary"
                    className="mt-4"
                    onClick={() => {
                      setEditing(null);
                      setDialogOpen(true);
                    }}
                  >
                    New connection
                  </Button>
                </div>
              )}
            </div>
          )}
        </main>
      </div>

      <ConnectionDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
        drivers={drivers}
        editing={editing}
        onSaved={save}
      />
    </div>
  );
}

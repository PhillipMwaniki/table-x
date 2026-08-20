/**
 * Application shell.
 *
 * The sidebar is real; the main pane is a placeholder until the schema browser,
 * editor, and result grid land.
 */

import { useEffect, useState } from "react";
import { ConnectionList } from "./components/ConnectionList";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { CommandPalette } from "./components/ui/CommandPalette";
import { Workspace } from "./components/workspace/Workspace";
import { Banner, Button, Spinner, cx } from "./components/ui/primitives";
import { useConnections } from "./store/connections";
import { useSettings } from "./store/settings";
import { useUpdates } from "./store/updates";
import { useCommands } from "./store/commands";
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
    select,
    connect,
    clearError,
  } = useConnections();

  const [dialogOpen, setDialogOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [editing, setEditing] = useState<ConnectionConfig | null>(null);
  /**
   * Whether the connections pane is hidden.
   *
   * Deliberately not persisted: collapsing is something you do to get room for
   * one wide result, not a way you want the app to open tomorrow. A sidebar
   * that stays gone after a restart reads as a bug.
   */
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

  const initSettings = useSettings((s) => s.init);
  const settingsReady = useSettings((s) => s.ready);
  const checkForUpdates = useSettings((s) => s.checkForUpdates);
  const checkUpdate = useUpdates((s) => s.check);
  const update = useUpdates((s) => s.available);
  const setPaletteOpen = useCommands((s) => s.setOpen);
  const registerCommands = useCommands((s) => s.register);

  useEffect(() => {
    void init();
    // Appearance is loaded alongside the connections rather than after them:
    // it decides what the first paint looks like.
    void initSettings();
  }, [init, initSettings]);

  // After the settings have loaded, so a user who turned this off is not asked
  // once more on every launch before the file is read. The store itself decides
  // whether enough time has passed; failures are silent by design.
  useEffect(() => {
    if (!settingsReady) return;
    void checkUpdate(checkForUpdates);
  }, [settingsReady, checkForUpdates, checkUpdate]);

  // Ctrl+, is the settings shortcut everywhere else; there is no reason for
  // this app to be the exception. Ctrl+K opens the palette, which is where
  // every other shortcut can be discovered.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key === ",") {
        e.preventDefault();
        setSettingsOpen((was) => !was);
      } else if (e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(!useCommands.getState().open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setPaletteOpen]);

  // Commands that exist whatever is open, plus one per saved connection so the
  // palette can reach a connection without touching the sidebar.
  useEffect(() => {
    return registerCommands("app", [
      {
        id: "app.new-connection",
        title: "New connection",
        group: "Connection",
        run: () => {
          setEditing(null);
          setDialogOpen(true);
        },
      },
      {
        id: "app.settings",
        title: "Appearance settings",
        group: "View",
        shortcut: "Ctrl+,",
        run: () => setSettingsOpen(true),
      },
      ...connections.map((c) => ({
        id: `app.open.${c.id}`,
        title: open.has(c.id) ? `Go to ${c.name}` : `Connect to ${c.name}`,
        group: "Connection",
        run: () => {
          select(c.id);
          if (!open.has(c.id)) void connect(c.id);
        },
      })),
    ]);
  }, [registerCommands, connections, open, select, connect]);

  const selected = connections.find((c) => c.id === selectedId) ?? null;

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text">
      <header className="drag-region flex h-9 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-3">
        <span className="text-[12px] font-semibold tracking-wide">Table X</span>
        <span className="text-[11px] text-text-muted">
          {open.size > 0 && `${open.size} connected`}
        </span>

        <div className="flex-1" />

        <button
          onClick={() => setSidebarCollapsed((was) => !was)}
          title={sidebarCollapsed ? "Show connections" : "Hide connections"}
          aria-label={sidebarCollapsed ? "Show connections" : "Hide connections"}
          aria-pressed={sidebarCollapsed}
          className="no-drag flex size-7 items-center justify-center rounded text-text-muted hover:bg-surface-2 hover:text-text"
        >
          {/* Drawn rather than typed: a glyph that means "panel" is not in any
              font we can rely on, and an emoji would not follow the theme. */}
          <svg width="15" height="15" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <rect x="1.5" y="2.5" width="13" height="11" rx="2" stroke="currentColor" />
            <line x1="6" y1="2.5" x2="6" y2="13.5" stroke="currentColor" />
            {/* Filled while hidden, so the button shows the current state and
                not only the action -- the two read the same at this size. */}
            {sidebarCollapsed && <rect x="2" y="3" width="4" height="10" fill="currentColor" />}
          </svg>
        </button>

        <button
          onClick={() => setSettingsOpen(true)}
          title={update ? `Table X ${update.latest} is available` : "Appearance (Ctrl+,)"}
          aria-label="Appearance settings"
          className="no-drag relative flex size-7 items-center justify-center rounded text-[19px] leading-none text-text-muted hover:bg-surface-2 hover:text-text"
        >
          ⚙
          {/* A dot, not a banner: a new version is worth knowing and never worth
              interrupting a query for. The colour follows the notice, so an
              advisory reads differently from a routine release. */}
          {update && (
            <span
              aria-hidden="true"
              className={cx(
                "absolute top-0.5 right-0.5 size-1.5 rounded-full",
                update.notice?.severity === "critical" ? "bg-danger" : "bg-accent",
              )}
            />
          )}
        </button>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* Unmounted rather than hidden: the `hidden` attribute is a user-agent
            rule, and the `flex` class here is an author rule that beats it, so
            the pane would stay on screen. */}
        {!sidebarCollapsed && (
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
        )}

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
                  <p className="mt-4 max-w-sm text-[12px] text-text-muted">Not connected yet.</p>
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

      <SettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} />

      <CommandPalette />
    </div>
  );
}

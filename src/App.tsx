/**
 * Application shell.
 *
 * The sidebar is real; the main pane is a placeholder until the schema browser,
 * editor, and result grid land.
 */

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ConnectionList } from "./components/ConnectionList";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { SettingsDialog } from "./components/SettingsDialog";
import { Dialog } from "./components/ui/Dialog";
import { CommandPalette } from "./components/ui/CommandPalette";
import { Workspace } from "./components/workspace/Workspace";
import { Banner, Button, Spinner, cx } from "./components/ui/primitives";
import { useConnections } from "./store/connections";
import { useSettings } from "./store/settings";
import { useLayouts } from "./store/layouts";
import { useUpdates } from "./store/updates";
import { useCommands } from "./store/commands";
import { useWorkspace } from "./store/workspace";
import { DesignView } from "./components/workspace/DesignView";
import { DesignList } from "./components/workspace/DesignList";
import { ipc, IpcError } from "./lib/ipc";
import type { ConnectionConfig, Design } from "./lib/types";

/**
 * The event a `.erd` file arrives on when the app is already running.
 *
 * Matches `OPEN_DESIGN_EVENT` in the Rust shell. A double-click on a design
 * while Table X is open is forwarded to this window rather than starting a
 * second copy of the application.
 */
const OPEN_DESIGN_EVENT = "open-design-file";

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
  /**
   * A design opened from a file with no connection to put it in front of.
   *
   * Tabs belong to a connection, and a design does not need one — so a `.erd`
   * double-clicked before connecting to anything is shown here instead of being
   * refused until the user picks a database it has nothing to do with.
   */
  const [designFile, setDesignFile] = useState<Design | null>(null);
  const [designError, setDesignError] = useState<string | null>(null);
  /**
   * A design's SQL script, when there is no query tab to put it in.
   *
   * With a connection open the script opens as an editor tab, where it can be
   * run. With none there is nowhere to run it, so it is shown to be read and
   * copied — which is what a script for a database you have not connected to
   * is for anyway.
   */
  const [designScript, setDesignScript] = useState<string | null>(null);
  /**
   * Whether the designs browser has the pane.
   *
   * App-level rather than inside a connection's workspace, because a design is
   * not about a connection. Somebody who opens Table X to draw a schema should
   * not have to connect to a database first to find where that is done.
   */
  const [designsOpen, setDesignsOpen] = useState(false);

  const initSettings = useSettings((s) => s.init);
  const initLayouts = useLayouts((s) => s.init);
  const settingsReady = useSettings((s) => s.ready);
  const checkForUpdates = useSettings((s) => s.checkForUpdates);
  const checkUpdate = useUpdates((s) => s.check);
  const update = useUpdates((s) => s.available);
  const rowDetails = useSettings((s) => s.rowDetails);
  const setRowDetails = useSettings((s) => s.setRowDetails);
  const setPaletteOpen = useCommands((s) => s.setOpen);
  const registerCommands = useCommands((s) => s.register);
  const reconnect = useWorkspace((s) => s.reconnect);

  /**
   * Open a design file, wherever there is room for it.
   *
   * The stores are read through `getState` rather than through this component's
   * own values: this runs from an event listener that outlives the render it
   * was set up in, and a captured connection id would be the one that was
   * selected when the app started.
   */
  const openDesignFile = useCallback(async (path: string) => {
    try {
      const design = await ipc.readDesignFile(path);
      const connections = useConnections.getState();
      const target =
        connections.selectedId && connections.open.has(connections.selectedId)
          ? connections.selectedId
          : null;
      if (target) {
        useWorkspace.getState().openDesign(target, design);
        setDesignFile(null);
      } else {
        setDesignFile(design);
      }
      setDesignError(null);
    } catch (e) {
      setDesignError((e as IpcError).message);
    }
  }, []);

  /** Show a design, in a tab where there is one and in the pane where not. */
  const showDesign = useCallback((design: Design) => {
    const connections = useConnections.getState();
    const target =
      connections.selectedId && connections.open.has(connections.selectedId)
        ? connections.selectedId
        : null;
    if (target) {
      useWorkspace.getState().openDesign(target, design);
      setDesignsOpen(false);
      setDesignFile(null);
    } else {
      setDesignFile(design);
      setDesignsOpen(false);
    }
  }, []);

  // Two ways in, because a file can be double-clicked before the app is running
  // or while it already is. The first arrives as a launch argument the frontend
  // asks for once it exists; the second as an event from the single-instance
  // handler.
  useEffect(() => {
    void ipc.startupDesigns().then((paths) => {
      for (const path of paths) void openDesignFile(path);
    });
    const stop = listen<string[]>(OPEN_DESIGN_EVENT, (event) => {
      for (const path of event.payload) void openDesignFile(path);
    });
    return () => {
      void stop.then((off) => off());
    };
  }, [openDesignFile]);

  useEffect(() => {
    void init();
    // Appearance is loaded alongside the connections rather than after them:
    // it decides what the first paint looks like.
    void initSettings();
    // Column layouts can follow: nothing draws a grid before a connection is
    // open, and a layout that arrives a frame after the first one is applied
    // to it then.
    void initLayouts();
  }, [init, initSettings, initLayouts]);

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
      {
        id: "app.designs",
        title: "Designs",
        group: "Design",
        run: () => {
          setDesignsOpen(true);
          setDesignFile(null);
        },
      },
      // The panel has a button of its own on the grid's toolbar; this is the
      // way to it that does not involve finding a 13px icon.
      {
        id: "app.row-details",
        title: rowDetails ? "Hide row details" : "Show row details",
        group: "View",
        run: () => setRowDetails(!rowDetails),
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
      // Only where there is a session to rebuild. On a connection that was
      // never opened this would be a second, worse-named Connect.
      ...connections
        .filter((c) => open.has(c.id))
        .map((c) => ({
          id: `app.reconnect.${c.id}`,
          title: `Reconnect to ${c.name}`,
          group: "Connection",
          run: () => {
            select(c.id);
            void reconnect(c.id);
          },
        })),
    ]);
  }, [registerCommands, connections, open, select, connect, reconnect, rowDetails, setRowDetails]);

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
          onClick={() => {
            setDesignsOpen(true);
            setDesignFile(null);
          }}
          title="Schema designs"
          aria-label="Schema designs"
          className="no-drag flex h-7 items-center rounded px-2 text-[11px] text-text-muted hover:bg-surface-2 hover:text-text"
        >
          Designs
        </button>

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

          {designError && (
            <div className="shrink-0 p-2">
              <Banner tone="error" onDismiss={() => setDesignError(null)}>
                {designError}
              </Banner>
            </div>
          )}

          {/* A design file takes the pane when there is no connected workspace
              to put it in a tab of. It is the whole reason the app was started
              in that case, and a design needs no database to be worked on. */}
          {designsOpen ? (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
                <span className="text-[12px] font-semibold">Designs</span>
                <span className="text-[11px] text-text-muted">
                  Schemas as documents. No connection needed until you want a script.
                </span>
                <div className="flex-1" />
                <Button variant="ghost" className="h-6" onClick={() => setDesignsOpen(false)}>
                  Close
                </Button>
              </div>
              <DesignList drivers={drivers} onOpen={showDesign} />
            </div>
          ) : designFile && !(selected && open.has(selected.id)) ? (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-2">
                <span className="text-[11px] text-text-muted">
                  Design file · not tied to a connection
                </span>
                <div className="flex-1" />
                <Button variant="ghost" className="h-6" onClick={() => setDesignFile(null)}>
                  Close
                </Button>
              </div>
              <DesignView
                design={designFile}
                onChange={setDesignFile}
                onScript={(report) =>
                  setDesignScript(report.statements.map((s) => s.sql).join("\n\n"))
                }
              />
            </div>
          ) : selected && open.has(selected.id) ? (
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
                    Select a connection from the sidebar, or create one to get started. To draw a
                    schema rather than query one, open a design — that needs no connection at all.
                  </p>
                  <div className="mt-4 flex items-center justify-center gap-2">
                    <Button
                      variant="primary"
                      onClick={() => {
                        setEditing(null);
                        setDialogOpen(true);
                      }}
                    >
                      New connection
                    </Button>
                    <Button onClick={() => setDesignsOpen(true)}>Designs</Button>
                  </div>
                </div>
              )}
            </div>
          )}
        </main>
      </div>

      <Dialog
        open={designScript !== null}
        onClose={() => setDesignScript(null)}
        title="Create script"
        description="The statements that would build this design. Nothing here runs — there is no connection open."
        width="wide"
        footer={
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => void navigator.clipboard?.writeText(designScript ?? "")}
              title="Copy the whole script"
            >
              Copy
            </Button>
            <Button variant="primary" onClick={() => setDesignScript(null)}>
              Done
            </Button>
          </div>
        }
      >
        <pre
          data-selectable
          className="max-h-[26rem] overflow-auto rounded border border-border bg-surface-0 p-2 font-mono text-[length:var(--text-data)] whitespace-pre-wrap"
        >
          {designScript}
        </pre>
      </Dialog>

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

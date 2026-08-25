/**
 * Saved schema designs, and the three ways to get one.
 *
 * Beside the notebooks when there is a connection, and in the main pane when
 * there is not — because a design needs no database, and somebody who opens the
 * app wanting to draw a schema should not have to connect to one first.
 *
 * The three ways are deliberate. Start empty, when the schema is still in
 * somebody's head. Start from a database, because almost every schema worth
 * changing already exists and typing it in again is why people abandon tools
 * like this. Or open a file somebody sent.
 */

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, Field, Input, Select, Spinner } from "../ui/primitives";
import { Dialog } from "../ui/Dialog";
import { ipc, IpcError } from "@/lib/ipc";
import { defaultDriver } from "@/lib/connection";
import type { Design, DriverInfo } from "@/lib/types";

export function DesignList({
  connectionId,
  schema,
  drivers,
  onOpen,
}: {
  /** Absent when nothing is connected, which removes only "From schema". */
  connectionId?: string | undefined;
  /** The schema the workspace is looking at, which a new design starts from. */
  schema?: string | null | undefined;
  drivers: DriverInfo[];
  onOpen: (design: Design) => void;
}) {
  const [designs, setDesigns] = useState<Design[]>([]);
  const [loading, setLoading] = useState(true);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  /** The new-design form, when it is open. */
  const [creating, setCreating] = useState<{ name: string; driver: string } | null>(null);

  const refresh = async () => {
    setLoading(true);
    try {
      setDesigns(await ipc.listDesigns());
      setError(null);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setLoading(false);
    }
  };

  // The first load does not go through `refresh`, which sets state as it
  // starts: setting state synchronously inside an effect costs a cascading
  // render, and this one has nothing to show until the answer arrives anyway.
  useEffect(() => {
    let cancelled = false;
    ipc
      .listDesigns()
      .then((list) => {
        if (cancelled) return;
        setDesigns(list);
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError((e as IpcError).message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** Start from nothing, which is what designing usually means. */
  const create = async () => {
    if (!creating) return;
    const draft = creating;
    setCreating(null);
    try {
      const design = await ipc.saveDesign({
        // crypto.randomUUID is available in every webview Tauri v2 supports.
        id: crypto.randomUUID(),
        name: draft.name.trim() || "Untitled design",
        driver: draft.driver,
        tables: [],
        layout: [],
        created_at: "",
        updated_at: "",
      });
      await refresh();
      onOpen(design);
    } catch (e) {
      setError((e as IpcError).message);
    }
  };

  /** Read the schema in front of the user into a new design. */
  const reverseEngineer = async () => {
    if (!connectionId) return;
    setReading(true);
    setError(null);
    try {
      const name = schema ? `${schema} design` : "New design";
      const design = await ipc.designFromSchema(connectionId, schema ?? undefined, name);
      await refresh();
      // Opened straight away: reading a schema takes long enough that being
      // left looking at a list afterwards reads as nothing having happened.
      onOpen(design);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setReading(false);
    }
  };

  /** Open a `.erd` file, which also keeps it among this machine's designs. */
  const fromFile = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "Table X design", extensions: ["erd"] }],
    });
    if (typeof path !== "string") return;
    setError(null);
    try {
      const design = await ipc.readDesignFile(path);
      await refresh();
      onOpen(design);
    } catch (e) {
      setError((e as IpcError).message);
    }
  };

  const needle = filter.trim().toLowerCase();
  const visible = designs.filter(
    (d) => !needle || d.name.toLowerCase().includes(needle) || d.driver.includes(needle),
  );

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border px-2 py-1">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter designs…"
          className="h-5 min-w-24 flex-1 rounded border border-border bg-surface-0 px-1.5 text-[11px] outline-none focus:border-accent"
        />
        <Button
          variant="ghost"
          className="h-5"
          onClick={() => setCreating({ name: "", driver: defaultDriver(drivers)?.id ?? "" })}
          title="Start a design with no tables in it"
        >
          New
        </Button>
        {/* Only where there is a schema to read. Offered without one it would be
            a button whose failure message is the only answer it has. */}
        {connectionId && (
          <Button
            variant="ghost"
            className="h-5"
            busy={reading}
            onClick={() => void reverseEngineer()}
            title="Read this schema into a new design"
          >
            From schema
          </Button>
        )}
        <Button
          variant="ghost"
          className="h-5"
          onClick={() => void fromFile()}
          title="Open a .erd design file"
        >
          Open file…
        </Button>
      </div>

      {error && (
        <p role="alert" className="px-2 py-1 text-[11px] text-danger">
          {error}
        </p>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <div className="flex justify-center p-4">
            <Spinner className="text-text-muted" />
          </div>
        ) : visible.length === 0 ? (
          <p className="p-6 text-center text-[11px] text-text-muted">
            {designs.length === 0
              ? connectionId
                ? "No designs yet. Start an empty one, read this schema into a design, or open a .erd file."
                : "No designs yet. Start an empty one, or open a .erd file — a design needs no connection."
              : "Nothing matches that."}
          </p>
        ) : (
          <ul>
            {visible.map((design) => (
              <li key={design.id} className="group border-b border-border/50">
                <div className="flex items-center gap-2 px-2 py-1.5">
                  <button onClick={() => onOpen(design)} className="min-w-0 flex-1 text-left">
                    <span className="block truncate text-[12px] text-text">{design.name}</span>
                    <span className="block truncate text-[10.5px] text-text-muted">
                      {design.tables.length} table{design.tables.length === 1 ? "" : "s"} ·{" "}
                      <span className="font-mono">{design.driver}</span> ·{" "}
                      {new Date(design.updated_at).toLocaleString()}
                      {/* Named when it is also a file, since that is where
                          saving will write and what can be sent to somebody. */}
                      {design.path && <span className="ml-1 text-text-muted/70">· a file</span>}
                    </span>
                  </button>
                  <button
                    onClick={async () => {
                      await ipc.deleteDesign(design.id);
                      await refresh();
                    }}
                    className="rounded px-1 py-0.5 text-[10.5px] text-text-muted opacity-0 hover:text-danger group-hover:opacity-100"
                  >
                    Delete
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <Dialog
        open={creating !== null}
        onClose={() => setCreating(null)}
        title="New design"
        description="A design is a document. It needs no database until you ask it for a script."
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setCreating(null)}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void create()}>
              Create
            </Button>
          </div>
        }
      >
        <div className="space-y-3">
          <Field label="Name">
            <Input
              autoFocus
              value={creating?.name ?? ""}
              placeholder="Untitled design"
              onChange={(e) => setCreating((was) => (was ? { ...was, name: e.target.value } : was))}
              onKeyDown={(e) => {
                if (e.key === "Enter") void create();
              }}
            />
          </Field>
          {/* Asked at the start because it cannot be avoided later: a design's
              script has to be written for one engine, and AUTO_INCREMENT and
              SERIAL are not the same thing. */}
          <Field
            label="For engine"
            hint="Decides how the script is written. A design is not portable between engines."
          >
            <Select
              value={creating?.driver ?? ""}
              onChange={(e) =>
                setCreating((was) => (was ? { ...was, driver: e.target.value } : was))
              }
            >
              {drivers.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </Select>
          </Field>
        </div>
      </Dialog>
    </div>
  );
}

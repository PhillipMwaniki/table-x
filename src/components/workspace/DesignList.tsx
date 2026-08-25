/**
 * Saved schema designs.
 *
 * Beside the notebooks rather than mixed into them: a notebook is reasoning
 * about a database that exists, a design is a database that does not exist yet.
 *
 * The button that matters most is the one that starts a design from the schema
 * in front of you. Almost every schema worth designing already exists in some
 * form, and typing it in again to get it onto a canvas is the reason people
 * give up on tools like this.
 */

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button, Spinner } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { Design } from "@/lib/types";

export function DesignList({
  connectionId,
  schema,
  onOpen,
}: {
  connectionId: string;
  /** The schema the workspace is looking at, which a new design starts from. */
  schema: string | null;
  onOpen: (design: Design) => void;
}) {
  const [designs, setDesigns] = useState<Design[]>([]);
  const [loading, setLoading] = useState(true);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

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

  const reverseEngineer = async () => {
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
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1">
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter designs…"
          className="h-5 flex-1 rounded border border-border bg-surface-0 px-1.5 text-[11px] outline-none focus:border-accent"
        />
        <Button
          variant="ghost"
          className="h-5"
          busy={reading}
          onClick={() => void reverseEngineer()}
          title="Read this schema into a new design"
        >
          From schema
        </Button>
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
          <p className="p-4 text-center text-[11px] text-text-muted">
            {designs.length === 0
              ? "No designs yet. “From schema” reads the schema you are connected to into one, or open a .erd file."
              : "Nothing matches that."}
          </p>
        ) : (
          <ul>
            {visible.map((design) => (
              <li key={design.id} className="group border-b border-border/50">
                <div className="flex items-center gap-2 px-2 py-1.5">
                  <button onClick={() => onOpen(design)} className="min-w-0 flex-1 text-left">
                    <span className="block truncate text-[12px] text-text">{design.name}</span>
                    <span className="block text-[10.5px] text-text-muted">
                      {design.tables.length} table{design.tables.length === 1 ? "" : "s"} ·{" "}
                      <span className="font-mono">{design.driver}</span> ·{" "}
                      {new Date(design.updated_at).toLocaleString()}
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
    </div>
  );
}

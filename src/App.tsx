/**
 * Application shell.
 *
 * The real layout (connection sidebar, schema tree, editor, result grid) lands
 * with the UI milestones. This scaffold exists to prove the Vite + Tauri + Tailwind
 * pipeline end to end and to surface the backend version over IPC.
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface Backend {
  version: string;
  drivers: string[];
}

export default function App() {
  const [backend, setBackend] = useState<Backend | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<Backend>("backend_info")
      .then(setBackend)
      .catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text">
      <header className="drag-region flex h-9 shrink-0 items-center border-b border-border bg-surface-1 px-3">
        <span className="text-[12px] font-medium tracking-wide">TablePro X</span>
      </header>

      <main className="flex flex-1 items-center justify-center">
        <div className="text-center">
          <h1 className="mb-1 text-lg font-semibold">TablePro X</h1>
          <p className="mb-6 text-text-muted">
            A fast, cross-platform database client for developers.
          </p>

          {error && (
            <p className="font-mono text-danger" role="alert">
              {error}
            </p>
          )}

          {backend && (
            <div className="font-mono text-[12px] text-text-muted">
              <p>backend v{backend.version}</p>
              <p>
                {backend.drivers.length} driver
                {backend.drivers.length === 1 ? "" : "s"}: {backend.drivers.join(", ") || "none"}
              </p>
            </div>
          )}
        </div>
      </main>
    </div>
  );
}

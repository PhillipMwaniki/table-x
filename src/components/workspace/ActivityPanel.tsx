/**
 * What the server is doing right now.
 *
 * The question this answers is the one asked when something is slow and nobody
 * knows why: who is connected, what are they running, how long have they been
 * running it, and is anyone stuck behind anyone else. So the list is ordered
 * with the working sessions first, the long-running ones are marked, and a
 * blocked session names its blocker rather than just looking idle.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { Banner, Button, Spinner, cx } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { ServerActivity, ServerSession } from "@/lib/types";

/** How often the auto-refresh reads, in milliseconds. */
const REFRESH_MS = 5_000;

/**
 * Seconds after which a running statement is worth noticing.
 *
 * Not a threshold for anything to happen — just for the number to stop being
 * grey. Most statements finish in milliseconds, so anything still going after
 * ten seconds is either heavy or stuck, and both are worth the eye landing on.
 */
const SLOW_SECONDS = 10;

export function ActivityPanel({
  connectionId,
  readOnly,
  onOpenQuery,
}: {
  connectionId: string;
  /** A read-only connection cannot end someone else's session. */
  readOnly: boolean;
  /** Send a session's statement to a new query tab, to read or explain it. */
  onOpenQuery: (sql: string) => void;
}) {
  const [activity, setActivity] = useState<ServerActivity | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [auto, setAuto] = useState(true);
  /** The session a kill has been clicked for but not yet confirmed. */
  const [confirming, setConfirming] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // Held in a ref so the interval can skip a tick while one is in flight: over
  // a tunnel a read can take longer than the interval, and queueing them would
  // turn a monitor into a load generator.
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    try {
      setActivity(await ipc.serverActivity(connectionId));
      setError(null);
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      inFlight.current = false;
      setLoading(false);
    }
  }, [connectionId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!auto) return;
    const timer = setInterval(() => void refresh(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [auto, refresh]);

  const kill = async (session: ServerSession) => {
    setConfirming(null);
    try {
      await ipc.killSession(connectionId, session.id);
      setNotice(`Ended session ${session.id}.`);
      await refresh();
    } catch (e) {
      setError((e as IpcError).message);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border bg-surface-1 px-2 py-1.5">
        {activity?.stats.map((stat) => (
          <span
            key={stat.label}
            className="rounded border border-border bg-surface-0 px-1.5 py-0.5 text-[11px]"
          >
            <span className="text-text-muted">{stat.label}</span>{" "}
            <span className="font-medium text-text tabular-nums">{stat.value}</span>
          </span>
        ))}

        <div className="flex-1" />

        {loading && <Spinner className="text-text-muted" />}
        <label className="flex cursor-pointer items-center gap-1 text-[11px] text-text-muted">
          <input
            type="checkbox"
            checked={auto}
            onChange={(e) => setAuto(e.target.checked)}
            className="size-3 accent-[var(--color-accent)]"
          />
          Auto
        </label>
        <Button variant="ghost" className="h-6" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>

      {(error || notice) && (
        <div className="shrink-0 px-2 pt-2">
          {error && (
            <Banner tone="error" onDismiss={() => setError(null)}>
              {error}
            </Banner>
          )}
          {notice && (
            <Banner tone="success" onDismiss={() => setNotice(null)}>
              {notice}
            </Banner>
          )}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse text-[length:var(--text-data)]">
          <thead className="sticky top-0 z-10 bg-surface-2">
            <tr className="text-left text-[11px] text-text-muted">
              <Th className="w-16">ID</Th>
              <Th className="w-28">User</Th>
              <Th className="w-32">Client</Th>
              <Th className="w-28">Database</Th>
              <Th className="w-40">State</Th>
              <Th className="w-20 text-right">Time</Th>
              <Th>Statement</Th>
              <Th className="w-20" />
            </tr>
          </thead>
          <tbody>
            {activity?.sessions.map((session) => (
              <Row
                key={session.id}
                session={session}
                readOnly={readOnly}
                confirming={confirming === session.id}
                onConfirm={() => setConfirming(session.id)}
                onCancel={() => setConfirming(null)}
                onKill={() => void kill(session)}
                onOpenQuery={onOpenQuery}
              />
            ))}
          </tbody>
        </table>

        {activity && activity.sessions.length === 0 && (
          <p className="p-4 text-center text-[12px] text-text-muted">
            Nothing is connected but this session.
          </p>
        )}
      </div>
    </div>
  );
}

function Th({ children, className }: { children?: React.ReactNode; className?: string }) {
  return (
    <th className={cx("border-b border-border px-2 py-1 font-medium", className)}>{children}</th>
  );
}

function Row({
  session,
  readOnly,
  confirming,
  onConfirm,
  onCancel,
  onKill,
  onOpenQuery,
}: {
  session: ServerSession;
  readOnly: boolean;
  confirming: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  onKill: () => void;
  onOpenQuery: (sql: string) => void;
}) {
  const slow = (session.seconds ?? 0) >= SLOW_SECONDS;

  return (
    <tr
      className={cx(
        "border-b border-border/50",
        // A blocked session is the row someone opened this panel to find.
        session.blocked_by && "bg-warn/10",
      )}
    >
      <Td className="font-mono">
        {session.id}
        {session.is_self && (
          <span className="ml-1 rounded bg-accent/15 px-1 text-[10px] text-accent">you</span>
        )}
      </Td>
      <Td>{session.user ?? "—"}</Td>
      <Td className="truncate text-text-muted">{session.client ?? "—"}</Td>
      <Td>{session.database ?? "—"}</Td>
      <Td>
        <span className="truncate">{session.state ?? "—"}</span>
        {session.blocked_by && (
          <span className="ml-1 whitespace-nowrap text-[11px] font-medium text-warn">
            blocked by {session.blocked_by}
          </span>
        )}
      </Td>
      <Td className={cx("text-right tabular-nums", slow ? "text-warn" : "text-text-muted")}>
        {session.seconds == null ? "—" : formatSeconds(session.seconds)}
      </Td>
      <Td>
        {session.query ? (
          <button
            type="button"
            // Opening the statement is how you find out what it is actually
            // doing — the cell can only ever show its first line.
            onClick={() => onOpenQuery(session.query!)}
            title={session.query}
            className="block w-full truncate text-left font-mono text-text hover:text-accent"
          >
            {session.query.replace(/\s+/g, " ")}
          </button>
        ) : (
          <span className="text-text-muted">—</span>
        )}
      </Td>
      <Td className="text-right">
        {readOnly ? null : confirming ? (
          // Two clicks, and the second one says what it does. There is no
          // statement to show first the way a DROP has one, so the button
          // carries the warning itself.
          <span className="flex justify-end gap-1">
            <button
              type="button"
              onClick={onKill}
              className="rounded bg-danger px-1.5 py-0.5 text-[11px] font-medium text-white"
            >
              End it
            </button>
            <button
              type="button"
              onClick={onCancel}
              className="rounded px-1 py-0.5 text-[11px] text-text-muted hover:text-text"
            >
              No
            </button>
          </span>
        ) : (
          <button
            type="button"
            onClick={onConfirm}
            className="rounded px-1.5 py-0.5 text-[11px] text-text-muted hover:bg-danger/10 hover:text-danger"
            title={
              session.is_self
                ? "This is your own session — ending it disconnects you"
                : "End this session"
            }
          >
            Kill
          </button>
        )}
      </Td>
    </tr>
  );
}

function Td({ children, className }: { children?: React.ReactNode; className?: string }) {
  return <td className={cx("max-w-0 px-2 py-1 align-top", className)}>{children}</td>;
}

/**
 * The same shape the backend uses for stats, applied to the per-row times.
 *
 * Kept here rather than crossing the wire pre-formatted because the column is
 * sorted and compared by the number, and a string that says "4h 0m" cannot be.
 */
function formatSeconds(seconds: number): string {
  if (seconds < 1) return `${Math.round(seconds * 1000)}ms`;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const total = Math.round(seconds);
  const m = Math.floor(total / 60);
  if (m < 60) return `${m}m ${total % 60}s`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

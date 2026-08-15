/**
 * Who exists on this server, and what each of them can reach.
 *
 * Principals on the left, the selected one's grants on the right, because the
 * question is almost always about one account rather than about the list.
 *
 * Privilege names are shown in the engine's own words. `SELECT` means the same
 * thing everywhere, but `BYPASSRLS`, `PROCESS`, and `VIEW SERVER STATE` do not
 * translate, and a mapping between them would be inventing an equivalence that
 * does not exist.
 */

import { useEffect, useMemo, useState } from "react";
import { Banner, Spinner, cx } from "../ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import type { Grant, Principal, Privileges } from "@/lib/types";

export function PrivilegesPanel({
  connectionId,
  quote,
  onOpenScript,
}: {
  connectionId: string;
  /** The engine's identifier quote, for the statements this offers to write. */
  quote: string;
  /** Send a REVOKE to a query tab, where it can be read before it is run. */
  onOpenScript: (title: string, sql: string) => void;
}) {
  const [data, setData] = useState<Privileges | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    ipc
      .privileges(connectionId)
      .then((next) => {
        if (cancelled) return;
        setData(next);
        setSelected((was) => was ?? next.principals[0]?.name ?? null);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError((e as IpcError).message);
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [connectionId]);

  const grants = useMemo(() => {
    if (!data || !selected) return [];
    const needle = filter.trim().toLowerCase();
    return data.grants
      .filter((g) => g.grantee === selected)
      .filter(
        (g) =>
          !needle ||
          g.privilege.toLowerCase().includes(needle) ||
          (g.object ?? "").toLowerCase().includes(needle),
      );
  }, [data, selected, filter]);

  if (loading) {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Spinner className="text-text-muted" />
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex-1 p-3">
        <Banner tone="error">{error}</Banner>
        {/* Reading privileges usually needs privileges of its own, and that is
            the likeliest reason to be here. */}
        <p className="mt-2 text-[11px] text-text-muted">
          Listing users generally requires an administrative account.
        </p>
      </div>
    );
  }

  if (!data) return null;

  const principal = data.principals.find((p) => p.name === selected);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {data.notes.length > 0 && (
        <div className="shrink-0 px-2 pt-2">
          <Banner tone="info">{data.notes.join(" ")}</Banner>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div className="flex w-56 shrink-0 flex-col border-r border-border">
          <div className="shrink-0 border-b border-border bg-surface-1 px-2 py-1 text-[11px] text-text-muted">
            {data.principals.length} principal{data.principals.length === 1 ? "" : "s"}
          </div>
          <ul className="min-h-0 flex-1 overflow-auto">
            {data.principals.map((p) => (
              <li key={p.name}>
                <button
                  type="button"
                  onClick={() => setSelected(p.name)}
                  className={cx(
                    "flex w-full items-center gap-1.5 px-2 py-1 text-left text-[12px]",
                    selected === p.name
                      ? "bg-surface-3 text-text"
                      : "text-text-muted hover:bg-surface-2",
                  )}
                >
                  <PrincipalIcon principal={p} />
                  <span className="min-w-0 flex-1 truncate">{p.name}</span>
                  {p.superuser && (
                    <span className="shrink-0 rounded bg-danger/15 px-1 text-[9.5px] font-medium text-danger">
                      SUPER
                    </span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        </div>

        <div className="flex min-w-0 flex-1 flex-col">
          {principal && (
            <div className="shrink-0 border-b border-border bg-surface-1 px-2 py-1.5">
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="font-medium text-[12px] text-text">{principal.name}</span>
                {!principal.can_login && (
                  // A role that cannot log in is a group in all but name, and
                  // that is the first thing anyone looks for.
                  <Tag>cannot log in</Tag>
                )}
                {principal.attributes.map((a) => (
                  <Tag key={a}>{a}</Tag>
                ))}
              </div>
              {principal.member_of.length > 0 && (
                <p className="mt-1 text-[11px] text-text-muted">
                  Inherits from{" "}
                  <span className="font-mono text-text">{principal.member_of.join(", ")}</span> —
                  those roles&apos; grants apply here too and are not listed below.
                </p>
              )}
            </div>
          )}

          <div className="flex shrink-0 items-center gap-2 border-b border-border px-2 py-1">
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Filter by privilege or object"
              className="h-6 flex-1 rounded border border-border bg-surface-0 px-1.5 text-[11px] outline-none focus:border-accent"
            />
            <span className="tabular-nums text-[11px] text-text-muted">{grants.length}</span>
          </div>

          <div className="min-h-0 flex-1 overflow-auto">
            {grants.length === 0 ? (
              <p className="p-4 text-center text-[11px] text-text-muted">
                {filter ? "Nothing matches that." : "No grants recorded for this principal."}
              </p>
            ) : (
              <table className="w-full border-collapse text-[length:var(--text-data)]">
                <tbody>
                  {grants.map((grant, i) => (
                    <tr
                      key={i}
                      className={cx(
                        "group border-b border-border/50",
                        grant.denied && "bg-danger/5",
                      )}
                    >
                      <td className="w-44 px-2 py-1 align-top">
                        <span
                          className={cx("font-medium", grant.denied ? "text-danger" : "text-text")}
                        >
                          {grant.denied && "DENY "}
                          {grant.privilege}
                        </span>
                      </td>
                      <td className="px-2 py-1 align-top font-mono text-text-muted">
                        {grant.object ?? <span className="italic">server-wide</span>}
                      </td>
                      <td className="w-24 px-2 py-1 align-top text-[10.5px] text-text-muted">
                        {grant.grantable && "may re-grant"}
                      </td>
                      <td className="w-20 px-2 py-1 text-right align-top">
                        <button
                          type="button"
                          // Opened rather than run: revoking is not something
                          // to do by clicking a row in a list.
                          onClick={() =>
                            onOpenScript(`Revoke — ${grant.grantee}`, revokeSql(grant, quote))
                          }
                          className="text-[10.5px] text-text-muted opacity-0 transition-opacity hover:text-danger group-hover:opacity-100"
                        >
                          Revoke…
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded border border-border bg-surface-0 px-1 py-px text-[10px] text-text-muted">
      {children}
    </span>
  );
}

function PrincipalIcon({ principal }: { principal: Principal }) {
  return (
    <span
      className="w-3 shrink-0 text-center text-[10px]"
      title={principal.kind === "role" ? "Role" : "User"}
    >
      {principal.kind === "role" ? "◇" : "○"}
    </span>
  );
}

/**
 * The statement that would take this grant away.
 *
 * Written here rather than fetched because it is a pure function of the row
 * already on screen, and a round trip to produce a line of text the user is
 * about to read anyway would only add a way for it to be out of date.
 */
function revokeSql(grant: Grant, quote: string): string {
  const close = quote === "[" ? "]" : quote;
  const name = `${quote}${grant.grantee.replaceAll(close, close + close)}${close}`;
  const on = grant.object ? ` ON ${grant.object}` : "";
  return `REVOKE ${grant.privilege}${on} FROM ${name};`;
}

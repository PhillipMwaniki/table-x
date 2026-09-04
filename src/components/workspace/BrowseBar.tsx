/**
 * A table tab's WHERE and ORDER BY, as two boxes above its rows.
 *
 * The filter above the grid works on the page that came back, and says so;
 * these go to the server, so they reach every row the table has. The bar is
 * the difference made visible: the two boxes are labelled with the keywords
 * they stand for, and what is typed in them is sent as written, in the
 * engine's own dialect.
 *
 * Typing is local until Enter or Apply. A clause that ran a query on every
 * keystroke would be a clause that ran a hundred queries to reach the one
 * meant, against a table that may be large.
 */

import { useState } from "react";
import { cx } from "../ui/primitives";
import type { Clauses } from "@/lib/browse";

export function BrowseBar({
  where,
  orderBy,
  busy,
  onApply,
}: {
  /** The clauses in force — what the rows on screen were fetched with. */
  where: string;
  orderBy: string;
  busy: boolean;
  onApply: (clauses: Clauses) => void;
}) {
  const [draftWhere, setDraftWhere] = useState(where);
  const [draftOrder, setDraftOrder] = useState(orderBy);
  // The clauses in force can change from outside the boxes — the paging
  // bar's "order by the key" fills ORDER BY — and the drafts follow, or the
  // box would show one thing while the rows were fetched with another.
  const [seen, setSeen] = useState({ where, orderBy });
  if (seen.where !== where || seen.orderBy !== orderBy) {
    setSeen({ where, orderBy });
    setDraftWhere(where);
    setDraftOrder(orderBy);
  }
  const dirty = draftWhere !== where || draftOrder !== orderBy;

  const apply = () => onApply({ where: draftWhere, orderBy: draftOrder });
  const revert = () => {
    setDraftWhere(where);
    setDraftOrder(orderBy);
  };

  return (
    <div
      role="group"
      aria-label="Server-side clauses"
      className="flex h-8 shrink-0 items-center gap-2 border-t border-border bg-surface-1 px-2 text-[11px]"
    >
      <Clause
        keyword="WHERE"
        value={draftWhere}
        inForce={where.trim().length > 0}
        placeholder="country = 'Kenya' AND active"
        onChange={setDraftWhere}
        onEnter={apply}
        onEscape={revert}
        onClear={() => {
          setDraftWhere("");
          onApply({ where: "", orderBy: draftOrder });
        }}
      />
      <Clause
        keyword="ORDER BY"
        value={draftOrder}
        inForce={orderBy.trim().length > 0}
        placeholder="joined DESC"
        onChange={setDraftOrder}
        onEnter={apply}
        onEscape={revert}
        onClear={() => {
          setDraftOrder("");
          onApply({ where: draftWhere, orderBy: "" });
        }}
      />
      {/* Present only while there is something to apply, so the bar at rest
          has no button that does nothing. */}
      {dirty && (
        <button
          onClick={apply}
          disabled={busy}
          className="rounded bg-accent px-2 py-0.5 font-medium text-accent-fg hover:bg-accent/90 disabled:opacity-50"
          title="Fetch the first page again with these clauses (Enter)"
        >
          Apply
        </button>
      )}
      <span
        className="hidden whitespace-nowrap text-text-muted/70 xl:inline"
        title="Sent to the database as part of the statement, so it reaches every row in the table. The filter above the rows only narrows the page that was loaded."
      >
        Runs on the server
      </span>
    </div>
  );
}

function Clause({
  keyword,
  value,
  inForce,
  placeholder,
  onChange,
  onEnter,
  onEscape,
  onClear,
}: {
  keyword: string;
  value: string;
  /** Whether the rows on screen were fetched with a clause here. */
  inForce: boolean;
  placeholder: string;
  onChange: (value: string) => void;
  onEnter: () => void;
  onEscape: () => void;
  onClear: () => void;
}) {
  return (
    <label className="flex min-w-0 flex-1 items-center gap-1">
      <span
        className={cx(
          "shrink-0 rounded px-1.5 py-0.5 font-mono text-[10px] font-medium tracking-wide",
          inForce ? "bg-accent/20 text-accent" : "bg-surface-2 text-text-muted",
        )}
      >
        {keyword}
      </span>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            onEnter();
          } else if (e.key === "Escape") {
            e.preventDefault();
            onEscape();
          }
        }}
        placeholder={placeholder}
        aria-label={keyword}
        spellCheck={false}
        className="h-5 min-w-0 flex-1 rounded border border-border bg-surface-0 px-1.5 font-mono text-[11px] placeholder:text-text-muted/40 focus:border-accent focus:outline-none"
      />
      {(value || inForce) && (
        <button
          onClick={onClear}
          className="rounded px-1 text-text-muted hover:text-text"
          title={`Clear the ${keyword} clause and fetch again`}
          aria-label={`Clear ${keyword}`}
        >
          ✕
        </button>
      )}
    </label>
  );
}

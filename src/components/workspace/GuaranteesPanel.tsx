/**
 * What this application is promising about the result on screen.
 *
 * All of it is already true and none of it is visible, which makes it worth
 * nothing to someone deciding whether to trust the tool. The claims here are
 * specific to the result in front of you on purpose: "we are careful with
 * decimals" is marketing, and "total is carried exactly, rate went through a
 * float because the column is one" is a fact you can check.
 */

import { Dialog } from "../ui/Dialog";
import { Button, cx } from "../ui/primitives";
import type { Guarantees } from "@/lib/guarantees";

export function GuaranteesPanel({
  open,
  onClose,
  guarantees,
  readOnly,
}: {
  open: boolean;
  onClose: () => void;
  guarantees: Guarantees;
  /** Why editing is off, when it is. Absent for an editable result. */
  readOnly?: { reason: string; remedy: string } | undefined;
}) {
  const { exact, approximate, keyColumns, editable, complete } = guarantees;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="About this result"
      description="What is guaranteed about the rows on screen, and what is not."
      footer={
        <div className="flex justify-end">
          <Button onClick={onClose}>Close</Button>
        </div>
      }
    >
      <div className="space-y-3">
        <Claim
          tone={editable ? "ok" : "muted"}
          title={editable ? "Edits are checked before they land" : "This result is read-only"}
        >
          {editable ? (
            <>
              An edit becomes an <code>UPDATE</code> matched on{" "}
              <Names names={keyColumns} />, carrying the value the row{" "}
              <em>had</em> when it was read. If that matches anything other than
              exactly one row, the change is rolled back rather than applied —
              so an edit cannot quietly hit a row someone else changed first.
            </>
          ) : (
            <>
              {readOnly?.reason} {readOnly?.remedy}
            </>
          )}
        </Claim>

        {exact.length > 0 && (
          <Claim tone="ok" title="Exact numbers stay exact">
            <Names names={exact} /> {exact.length === 1 ? "is" : "are"} carried
            as text from the server to this screen, digit for digit. No part of
            the path converts {exact.length === 1 ? "it" : "them"} to a
            floating-point number, so nothing is rounded on the way — including
            values wider than a 64-bit float can hold.
          </Claim>
        )}

        {approximate.length > 0 && (
          <Claim tone="warn" title="Some columns are approximate at the source">
            <Names names={approximate} /> {approximate.length === 1 ? "is" : "are"}{" "}
            a floating-point column, so {approximate.length === 1 ? "its value" : "their values"}{" "}
            may already have been rounded before this application saw{" "}
            {approximate.length === 1 ? "it" : "them"}. That is the column's
            type, not something done here — but it is worth knowing before
            trusting the last digits.
          </Claim>
        )}

        <Claim
          tone={complete ? "ok" : "warn"}
          title={complete ? "Every row is here" : "This is part of the result"}
        >
          {complete ? (
            <>
              The statement returned fewer rows than the page limit, so what is
              shown is everything it produced. Sorting and filtering in the grid
              cover the whole result.
            </>
          ) : (
            <>
              A full page came back, so there is likely more. Sorting and
              filtering in the grid apply to the rows fetched so far, not to the
              whole result — which is why the row count says how many are
              loaded rather than how many exist.
            </>
          )}
        </Claim>
      </div>
    </Dialog>
  );
}

function Claim({
  tone,
  title,
  children,
}: {
  tone: "ok" | "warn" | "muted";
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div
      className={cx(
        "rounded-md border p-2.5",
        tone === "ok" && "border-ok/40 bg-ok/5",
        tone === "warn" && "border-warn/40 bg-warn/5",
        tone === "muted" && "border-border bg-surface-2",
      )}
    >
      <h3
        className={cx(
          "mb-1 text-[12px] font-medium",
          tone === "ok" && "text-ok",
          tone === "warn" && "text-warn",
          tone === "muted" && "text-text",
        )}
      >
        {title}
      </h3>
      <p className="text-[11.5px] leading-relaxed text-text-muted">{children}</p>
    </div>
  );
}

/** A column list that reads as prose rather than as an array. */
function Names({ names }: { names: string[] }) {
  return (
    <>
      {names.map((name, i) => (
        <span key={name}>
          {i > 0 && (i === names.length - 1 ? " and " : ", ")}
          <code className="font-mono text-text">{name}</code>
        </span>
      ))}
    </>
  );
}

/**
 * One name, and nothing else.
 *
 * Creating a database or a schema takes exactly one piece of information, and
 * every engine's extra options — a character set, an owner, a tablespace — have
 * a default that is right far more often than a form full of pickers would be.
 * Anything more particular than that is a `CREATE` statement somebody writes in
 * the editor, which is a better place for it than a dialog trying to model five
 * engines' worth of clauses.
 */

import { useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Button, Field, Input } from "../ui/primitives";

export function NameDialog({
  open,
  title,
  label,
  description,
  onClose,
  onSubmit,
}: {
  open: boolean;
  title: string;
  label: string;
  description?: string | undefined;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const trimmed = name.trim();

  const submit = () => {
    if (!trimmed) return;
    setName("");
    onSubmit(trimmed);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      {...(description ? { description } : {})}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!trimmed} onClick={submit}>
            Create
          </Button>
        </div>
      }
    >
      <Field label={label}>
        <Input
          autoFocus
          value={name}
          spellCheck={false}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
      </Field>
    </Dialog>
  );
}

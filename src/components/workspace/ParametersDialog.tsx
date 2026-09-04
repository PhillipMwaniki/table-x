/**
 * Values for a statement's placeholders, asked for before it runs.
 *
 * One row per placeholder: what kind of value, and the value. The kind
 * matters because the hole is filled with a literal, and `'5'` and `5` are
 * different things to a database. Text is the default because it is the
 * kind that cannot be mistaken for another; NULL and raw SQL are there for
 * the cases a form otherwise cannot express.
 *
 * Values are remembered by name for the session, so re-running the same
 * query asks the same question with the last answer already in the box.
 */

import { useState } from "react";
import { Dialog } from "../ui/Dialog";
import { Button, Field, Input, Select } from "../ui/primitives";
import { literal } from "@/lib/params";
import type { Parameter, ParameterValue, ValueType } from "@/lib/params";

/** Last values given, by parameter name. Session-only, deliberately. */
const remembered = new Map<string, ParameterValue>();

const TYPES: { value: ValueType; label: string }[] = [
  { value: "text", label: "Text" },
  { value: "number", label: "Number" },
  { value: "boolean", label: "Boolean" },
  { value: "null", label: "NULL" },
  { value: "raw", label: "Raw SQL" },
];

export function ParametersDialog({
  open,
  parameters,
  driver,
  onClose,
  onSubmit,
}: {
  open: boolean;
  parameters: Parameter[];
  driver: string;
  onClose: () => void;
  onSubmit: (values: Record<string, ParameterValue>) => void;
}) {
  const [values, setValues] = useState<Record<string, ParameterValue>>(() =>
    Object.fromEntries(
      parameters.map((p) => [p.name, remembered.get(p.name) ?? { type: "text", text: "" }]),
    ),
  );

  const problems = Object.fromEntries(
    parameters.map((p) => {
      const value = values[p.name] ?? { type: "text" as const, text: "" };
      const lit = literal(value, driver);
      return [p.name, lit.ok ? undefined : lit.error];
    }),
  );
  const complete = parameters.every((p) => !problems[p.name]);

  const submit = () => {
    if (!complete) return;
    for (const [name, value] of Object.entries(values)) remembered.set(name, value);
    onSubmit(values);
  };

  const set = (name: string, change: Partial<ParameterValue>) =>
    setValues((was) => ({
      ...was,
      [name]: { ...(was[name] ?? { type: "text", text: "" }), ...change },
    }));

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Fill in the placeholders"
      description="Each is replaced by a literal before the statement runs, so what ran is what the history shows."
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!complete} onClick={submit}>
            Run
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        {parameters.map((p, i) => {
          const value = values[p.name] ?? { type: "text" as const, text: "" };
          return (
            <div key={p.name} className="grid grid-cols-[1fr_7rem] items-end gap-2">
              <Field
                label={p.token}
                error={value.text || value.type === "null" ? problems[p.name] : undefined}
              >
                {value.type === "null" ? (
                  <Input value="NULL" disabled />
                ) : value.type === "boolean" ? (
                  <Select
                    value={value.text === "false" ? "false" : "true"}
                    onChange={(e) => set(p.name, { text: e.target.value })}
                  >
                    <option value="true">true</option>
                    <option value="false">false</option>
                  </Select>
                ) : (
                  <Input
                    autoFocus={i === 0}
                    value={value.text}
                    spellCheck={false}
                    className={value.type === "raw" ? "font-mono" : undefined}
                    onChange={(e) => set(p.name, { text: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") submit();
                    }}
                  />
                )}
              </Field>
              <Field label="As">
                <Select
                  value={value.type}
                  onChange={(e) => {
                    const type = e.target.value as ValueType;
                    set(p.name, {
                      type,
                      // A boolean box has two answers; give it one.
                      ...(type === "boolean" && value.text !== "false" ? { text: "true" } : {}),
                    });
                  }}
                >
                  {TYPES.map((t) => (
                    <option key={t.value} value={t.value}>
                      {t.label}
                    </option>
                  ))}
                </Select>
              </Field>
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}

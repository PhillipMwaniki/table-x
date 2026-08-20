/**
 * Appearance settings.
 *
 * Every control applies immediately and the dialog stays open, so the theme and
 * type can be judged against the real thing behind it rather than a swatch.
 */

import { useEffect, useMemo, useState } from "react";
import { Dialog } from "./ui/Dialog";
import { Button, Checkbox, Field, Input, Select, cx } from "./ui/primitives";
import { useSettings } from "@/store/settings";
import { DATA_FONTS, MAX_FONT_SIZE, MIN_FONT_SIZE, THEMES, UI_FONTS } from "@/lib/settings";

/**
 * Whether a stack's first family is actually installed.
 *
 * `document.fonts.check` needs a size and a probe string; it returns true for a
 * generic fallback, so the generics are filtered out before asking.
 */
function isInstalled(stack: string): boolean {
  const first = stack.split(",")[0]?.trim();
  if (!first) return true;
  if (/^(ui-monospace|monospace|system-ui|sans-serif|serif)$/.test(first)) return true;
  try {
    return document.fonts.check(`12px ${first}`);
  } catch {
    // A browser that will not answer is not grounds for hiding the whole list.
    return true;
  }
}

export function SettingsDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const {
    theme,
    uiFont,
    dataFont,
    dataFontSize,
    stripedRows,
    setStripedRows,
    checkForUpdates,
    setCheckForUpdates,
    setTheme,
    setUiFont,
    setDataFont,
    setDataFontSize,
    reset,
  } = useSettings();

  // Probed once per opening: fonts can be installed while the app runs, but
  // checking on every render would be wasted work.
  const [installed, setInstalled] = useState<{ data: typeof DATA_FONTS; ui: typeof UI_FONTS }>({
    data: DATA_FONTS,
    ui: UI_FONTS,
  });

  useEffect(() => {
    if (!open) return;
    void document.fonts.ready.then(() => {
      setInstalled({
        data: DATA_FONTS.filter((f) => !f.stack || isInstalled(f.stack)),
        ui: UI_FONTS.filter((f) => !f.stack || isInstalled(f.stack)),
      });
    });
  }, [open]);

  // A stack saved on another machine stays selectable here even if the font is
  // missing, rather than silently snapping the dropdown to something else.
  const dataOptions = useMemo(
    () => withCurrent(installed.data, dataFont),
    [installed.data, dataFont],
  );
  const uiOptions = useMemo(() => withCurrent(installed.ui, uiFont), [installed.ui, uiFont]);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Appearance"
      description="Changes apply as you make them."
      footer={
        <div className="flex items-center justify-between">
          <Button variant="ghost" onClick={reset}>
            Reset to defaults
          </Button>
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        </div>
      }
    >
      <section>
        <h3 className="mb-1.5 text-[11px] font-medium text-text-muted">Theme</h3>
        <div className="grid grid-cols-2 gap-1.5">
          <ThemeOption
            name="Follow system"
            note="Light or dark, whichever the OS is using."
            selected={theme === "system"}
            onSelect={() => setTheme("system")}
          />
          {THEMES.map((t) => (
            <ThemeOption
              key={t.id}
              name={t.name}
              note={t.note}
              themeId={t.id}
              selected={theme === t.id}
              onSelect={() => setTheme(t.id)}
            />
          ))}
        </div>
      </section>

      <section className="mt-4 space-y-3">
        <h3 className="text-[11px] font-medium text-text-muted">Type</h3>

        <Field
          label="Data font"
          hint="SQL, cell values, and column types. Only fonts installed on this machine are listed."
        >
          <Select value={dataFont} onChange={(e) => setDataFont(e.target.value)}>
            {dataOptions.map((f) => (
              <option key={f.label} value={f.stack}>
                {f.label}
              </option>
            ))}
          </Select>
        </Field>

        <Field
          label="Data size"
          hint={`${MIN_FONT_SIZE}–${MAX_FONT_SIZE} px. Grid rows grow to match.`}
        >
          <div className="flex items-center gap-2">
            <input
              type="range"
              min={MIN_FONT_SIZE}
              max={MAX_FONT_SIZE}
              value={dataFontSize}
              onChange={(e) => setDataFontSize(Number(e.target.value))}
              className="h-7 flex-1 accent-[var(--color-accent)]"
              aria-label="Data font size"
            />
            <Input
              type="number"
              min={MIN_FONT_SIZE}
              max={MAX_FONT_SIZE}
              value={dataFontSize}
              onChange={(e) => setDataFontSize(Number(e.target.value))}
              className="w-16"
              aria-label="Data font size in pixels"
            />
          </div>
        </Field>

        <Field label="Interface font" hint="Menus, labels, and buttons. Their sizes are fixed.">
          <Select value={uiFont} onChange={(e) => setUiFont(e.target.value)}>
            {uiOptions.map((f) => (
              <option key={f.label} value={f.stack}>
                {f.label}
              </option>
            ))}
          </Select>
        </Field>

        {/* Shows the chosen face at the chosen size, in the tokens of the chosen
            theme — the three settings interact, so they are previewed together. */}
        <div className="rounded-md border border-border bg-surface-0 p-2">
          <p className="mb-1 text-[10px] text-text-muted">Preview</p>
          <pre className="overflow-x-auto font-mono text-[length:var(--text-data)] text-text">
            <span className="text-accent">SELECT</span> id, email, balance
            {"\n"}
            <span className="text-accent">FROM</span> customers{" "}
            <span className="text-accent">WHERE</span> active;
          </pre>
          <p className="mt-1 font-mono text-[length:var(--text-data)] text-text-muted">
            123456789012345678.1234567890 · NULL · 2026-08-14
          </p>
        </div>
      </section>

      <section className="mt-4 space-y-2">
        <h3 className="text-[11px] font-medium text-text-muted">Grid</h3>

        <Checkbox
          label="Alternating row colours"
          hint="Banding stops the eye sliding a line up or down on a wide result."
          checked={stripedRows}
          onChange={setStripedRows}
        />

        <Checkbox
          label="Check for updates"
          hint="Asks the release channel once a day whether a newer version exists, and sends nothing. The only request this app makes that you did not ask for."
          checked={checkForUpdates}
          onChange={setCheckForUpdates}
        />

        {/* Four rows of nothing in particular, banded the way the grid bands
            them. The effect is small by design, so describing it is worse than
            showing it. */}
        <div className="overflow-hidden rounded-md border border-border">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              className={cx(
                "flex gap-4 px-2 font-mono text-[length:var(--text-data)] leading-6 text-text-muted",
                stripedRows && i % 2 === 1 ? "bg-surface-1" : "bg-surface-0",
              )}
            >
              <span className="w-4 text-right text-text-muted/50">{i + 1}</span>
              <span>{["alice", "bob", "carol", "dave"][i]}</span>
              <span className="ml-auto tabular-nums">{[128.5, 4.0, 96.25, 31.75][i]}</span>
            </div>
          ))}
        </div>
      </section>
    </Dialog>
  );
}

/** Keep the active stack in the list even when its font is not installed. */
function withCurrent(
  options: { label: string; stack: string }[],
  current: string,
): { label: string; stack: string }[] {
  if (!current || options.some((o) => o.stack === current)) return options;
  const label = current.split(",")[0]?.replace(/"/g, "").trim() ?? current;
  return [...options, { label: `${label} (not installed)`, stack: current }];
}

function ThemeOption({
  name,
  note,
  themeId,
  selected,
  onSelect,
}: {
  name: string;
  note: string;
  themeId?: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      aria-pressed={selected}
      className={cx(
        "flex items-center gap-2 rounded-md border px-2 py-1.5 text-left",
        selected ? "border-accent bg-surface-2" : "border-border hover:bg-surface-2",
      )}
    >
      {/* The swatch renders in the theme it names, so the list shows what each
          one actually looks like instead of describing it. */}
      <span
        data-theme={themeId}
        aria-hidden
        className="flex size-6 shrink-0 overflow-hidden rounded border border-border bg-surface-0"
      >
        <span className="w-1/3 bg-surface-2" />
        <span className="w-1/3 bg-accent" />
        <span className="w-1/3 bg-surface-0" />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-[12px] text-text">{name}</span>
        <span className="block truncate text-[10px] text-text-muted">{note}</span>
      </span>
    </button>
  );
}

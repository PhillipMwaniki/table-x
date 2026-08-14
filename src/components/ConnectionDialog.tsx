/**
 * Create/edit a connection.
 *
 * The form shape is driven by `DriverInfo.file_based` rather than by a hardcoded
 * list of driver ids, so a new driver gets the right fields without touching
 * this file.
 */

import { useEffect, useMemo, useState } from "react";
import { Dialog } from "./ui/Dialog";
import { SshSection } from "./SshSection";
import { Banner, Button, Checkbox, Field, Input, Select } from "./ui/primitives";
import { ipc, IpcError } from "@/lib/ipc";
import { folderNames, normalizeFolder } from "@/lib/folders";
import { useConnections } from "@/store/connections";
import type { ConnectionConfig, DriverInfo, TlsMode } from "@/lib/types";

/** A sentinel meaning "the stored secret is unchanged". */
const KEEP_EXISTING = Symbol("keep");

const COLORS = [
  { value: "", label: "None" },
  { value: "#e5484d", label: "Red — production" },
  { value: "#f5a524", label: "Amber — staging" },
  { value: "#30a46c", label: "Green — development" },
  { value: "#4d8df5", label: "Blue" },
  { value: "#8e4ec6", label: "Purple" },
];

function blankConfig(driver: DriverInfo): ConnectionConfig {
  return {
    // crypto.randomUUID is available in every webview Tauri v2 supports.
    id: crypto.randomUUID(),
    name: "",
    driver: driver.id,
    host: driver.file_based ? undefined : "localhost",
    port: driver.default_port ?? undefined,
    database: undefined,
    username: undefined,
    file_path: undefined,
    tls: { mode: "prefer" },
    read_only: false,
    options: {},
  };
}

export function ConnectionDialog({
  open,
  onClose,
  drivers,
  editing,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  drivers: DriverInfo[];
  /** `null` creates a new connection. */
  editing: ConnectionConfig | null;
  onSaved: (config: ConnectionConfig, secret?: string, sshSecret?: string) => Promise<void>;
}) {
  // Existing folders come from the saved connections, so the picker offers what
  // is already there instead of asking the user to remember their own names.
  //
  // The selector returns the stored array and the derivation happens in a memo:
  // a selector that builds a new array each call is compared by identity, looks
  // like a change on every render, and takes the component down with "Maximum
  // update depth exceeded".
  const connections = useConnections((s) => s.connections);
  const existingFolders = useMemo(() => folderNames(connections), [connections]);

  const [config, setConfig] = useState<ConnectionConfig | null>(null);
  // `KEEP_EXISTING` distinguishes "the user did not touch the password field"
  // from "the user cleared it", which must delete the keychain entry.
  const [secret, setSecret] = useState<string | typeof KEEP_EXISTING>("");
  const [sshSecret, setSshSecret] = useState<string | typeof KEEP_EXISTING>("");
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ tone: "error" | "success"; text: string } | null>(null);

  // Reset whenever the dialog opens, so a cancelled edit never leaks into the
  // next one.
  useEffect(() => {
    if (!open) return;
    const first = drivers[0];
    if (editing) {
      setConfig({ ...editing });
      setSecret(KEEP_EXISTING);
      setSshSecret(KEEP_EXISTING);
    } else if (first) {
      setConfig(blankConfig(first));
      setSecret("");
      setSshSecret("");
    }
    setResult(null);
  }, [open, editing, drivers]);

  const driver = useMemo(
    () => drivers.find((d) => d.id === config?.driver),
    [drivers, config?.driver],
  );

  if (!config || !driver) return null;

  const patch = (changes: Partial<ConnectionConfig>) =>
    setConfig((c) => (c ? { ...c, ...changes } : c));

  /** Switching driver rewrites the transport fields but keeps the name and id. */
  const changeDriver = (id: string) => {
    const next = drivers.find((d) => d.id === id);
    if (!next) return;
    setConfig((c) =>
      c
        ? {
            ...blankConfig(next),
            id: c.id,
            name: c.name,
            folder: c.folder,
            color: c.color,
            read_only: c.read_only,
          }
        : c,
    );
  };

  const nameError = config.name.trim() === "" ? "A name is required" : undefined;
  const targetError = driver.file_based
    ? !config.file_path?.trim()
      ? "A database file is required"
      : undefined
    : !config.host?.trim()
      ? "A host is required"
      : undefined;
  const invalid = Boolean(nameError || targetError);

  const secretArg = () => (secret === KEEP_EXISTING ? undefined : secret);
  const sshSecretArg = () => (sshSecret === KEEP_EXISTING ? undefined : sshSecret);

  const handleTest = async () => {
    setTesting(true);
    setResult(null);
    try {
      await ipc.testConnection(config, secretArg(), sshSecretArg());
      setResult({ tone: "success", text: "Connected successfully." });
    } catch (e) {
      const err = e as IpcError;
      setResult({
        tone: "error",
        // The category is the actionable part: "auth" means fix the password,
        // "connection" means fix the host.
        text: err.category === "auth" ? `Authentication failed. ${err.message}` : err.message,
      });
    } finally {
      setTesting(false);
    }
  };

  const handleSave = async () => {
    setSaving(true);
    setResult(null);
    try {
      await onSaved({ ...config, name: config.name.trim() }, secretArg(), sshSecretArg());
      onClose();
    } catch (e) {
      // Stay open on failure so the user's input is not thrown away.
      setResult({ tone: "error", text: (e as Error).message });
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={editing ? "Edit connection" : "New connection"}
      description="Credentials are stored in your operating system's keychain, never in the config file."
      footer={
        <div className="flex items-center gap-2">
          <Button onClick={handleTest} busy={testing} disabled={invalid || saving}>
            Test connection
          </Button>
          <div className="flex-1" />
          <Button variant="ghost" onClick={onClose} disabled={saving}>
            Cancel
          </Button>
          <Button variant="primary" onClick={handleSave} busy={saving} disabled={invalid}>
            Save
          </Button>
        </div>
      }
    >
      <div className="space-y-3">
        {result && (
          <Banner tone={result.tone} onDismiss={() => setResult(null)}>
            {result.text}
          </Banner>
        )}

        <div className="grid grid-cols-2 gap-3">
          <Field label="Name" error={config.name ? undefined : nameError}>
            <Input
              value={config.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="Production"
              autoFocus
            />
          </Field>

          <Field label="Driver">
            <Select
              value={config.driver}
              onChange={(e) => changeDriver(e.target.value)}
              // Changing driver on an existing connection would invalidate its
              // stored credential and saved shape.
              disabled={Boolean(editing)}
            >
              {drivers.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.name}
                </option>
              ))}
            </Select>
          </Field>
        </div>

        {driver.file_based ? (
          <Field
            label="Database file"
            hint="Use :memory: for a scratch database that is discarded on close."
            error={config.file_path ? undefined : targetError}
          >
            <Input
              value={config.file_path ?? ""}
              onChange={(e) => patch({ file_path: e.target.value })}
              placeholder="C:\data\app.db"
              spellCheck={false}
            />
          </Field>
        ) : (
          <>
            <div className="grid grid-cols-[1fr_5.5rem] gap-3">
              <Field label="Host" error={config.host ? undefined : targetError}>
                <Input
                  value={config.host ?? ""}
                  onChange={(e) => patch({ host: e.target.value })}
                  placeholder="localhost"
                  spellCheck={false}
                />
              </Field>
              <Field label="Port">
                <Input
                  type="number"
                  value={config.port ?? ""}
                  onChange={(e) =>
                    patch({ port: e.target.value ? Number(e.target.value) : undefined })
                  }
                  placeholder={String(driver.default_port ?? "")}
                />
              </Field>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <Field label="Database">
                <Input
                  value={config.database ?? ""}
                  onChange={(e) => patch({ database: e.target.value || undefined })}
                  spellCheck={false}
                />
              </Field>
              <Field label="Username">
                <Input
                  value={config.username ?? ""}
                  onChange={(e) => patch({ username: e.target.value || undefined })}
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
            </div>

            <Field
              label="Password"
              hint={
                secret === KEEP_EXISTING
                  ? "Leave blank to keep the saved password. Clearing it removes the stored credential."
                  : undefined
              }
            >
              <Input
                type="password"
                value={secret === KEEP_EXISTING ? "" : secret}
                // The stored password is never sent to the frontend, so the field
                // starts empty and only becomes meaningful once typed in.
                placeholder={secret === KEEP_EXISTING ? "••••••••" : ""}
                onChange={(e) => setSecret(e.target.value)}
                autoComplete="new-password"
              />
            </Field>

            <Field
              label="TLS"
              hint={
                config.tls.mode === "prefer"
                  ? "Uses TLS when the server offers a trusted certificate, otherwise connects unencrypted."
                  : config.tls.mode === "verify_full"
                    ? "Requires TLS and verifies the certificate chain and hostname."
                    : "Never encrypts. Only appropriate on a trusted local network."
              }
            >
              <Select
                value={config.tls.mode}
                onChange={(e) =>
                  patch({ tls: { ...config.tls, mode: e.target.value as TlsMode } })
                }
              >
                <option value="prefer">Prefer</option>
                <option value="verify_full">Require and verify</option>
                <option value="disable">Disable</option>
              </Select>
            </Field>
          </>
        )}

        {/* An embedded database is a local file — there is nothing to tunnel to. */}
        {!driver.file_based && (
          <SshSection
            ssh={config.ssh}
            onChange={(ssh) => patch({ ssh })}
            secret={sshSecret === KEEP_EXISTING ? undefined : sshSecret}
            onSecret={setSshSecret}
            secretIsStored={Boolean(editing)}
          />
        )}

        <div className="border-t border-border pt-3">
          <Field
            label="Folder"
            hint="Groups this connection in the sidebar. Type a new name or pick an existing one."
          >
            {/* A text input with suggestions rather than a dropdown: creating a
                folder and choosing one are the same gesture, so there is no
                separate "new folder" step to find. */}
            <Input
              list="connection-folders"
              value={config.folder ?? ""}
              onChange={(e) => patch({ folder: normalizeFolder(e.target.value) })}
              placeholder="None"
            />
            <datalist id="connection-folders">
              {existingFolders.map((f) => (
                <option key={f} value={f} />
              ))}
            </datalist>
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-3 pt-1">
          <Field label="Colour tag" hint="Shown in the sidebar.">
            <Select
              value={config.color ?? ""}
              onChange={(e) => patch({ color: e.target.value || undefined })}
            >
              {COLORS.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </Select>
          </Field>

          <div className="flex items-end pb-1">
            <Checkbox
              label="Read-only"
              hint="Blocks writes from this app regardless of database permissions."
              checked={config.read_only}
              onChange={(read_only) => patch({ read_only })}
            />
          </div>
        </div>
      </div>
    </Dialog>
  );
}

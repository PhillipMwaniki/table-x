/**
 * SSH tunnel configuration.
 *
 * The host key flow is the load-bearing part. The backend refuses to open a
 * tunnel to a host whose fingerprint it does not already know, so this section
 * has to fetch the fingerprint, show it, and get an explicit confirmation before
 * a tunnelled connection can work at all. That is deliberate: silently trusting
 * whatever key appears would defeat the point of tunnelling in the first place.
 */

import { useState } from "react";
import { Banner, Button, Checkbox, Field, Input, Select, cx } from "./ui/primitives";
import { ipc } from "@/lib/ipc";
import type { SshAuth, SshConfig } from "@/lib/types";

function blankSsh(): SshConfig {
  return { host: "", port: 22, username: "", auth: "public_key" };
}

export function SshSection({
  ssh,
  onChange,
  secret,
  onSecret,
  secretIsStored,
}: {
  ssh: SshConfig | undefined;
  onChange: (ssh: SshConfig | undefined) => void;
  /** Empty string means "cleared"; undefined means "untouched". */
  secret: string | undefined;
  onSecret: (value: string) => void;
  secretIsStored: boolean;
}) {
  const [probing, setProbing] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const enabled = ssh !== undefined;
  const patch = (changes: Partial<SshConfig>) => ssh && onChange({ ...ssh, ...changes });

  const verify = async () => {
    if (!ssh) return;
    setProbing(true);
    setError(null);
    setCandidate(null);
    try {
      const fingerprint = await ipc.sshHostFingerprint(ssh);
      if (fingerprint === ssh.host_key_fingerprint) {
        // Already trusted — say so rather than asking again.
        setError(null);
        setCandidate(null);
        patch({ host_key_fingerprint: fingerprint });
      } else {
        setCandidate(fingerprint);
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setProbing(false);
    }
  };

  return (
    <div className="border-t border-border pt-3">
      <Checkbox
        label="Connect through an SSH tunnel"
        hint="For databases reachable only from a bastion host."
        checked={enabled}
        onChange={(on) => onChange(on ? blankSsh() : undefined)}
      />

      {enabled && ssh && (
        <div className="mt-3 space-y-3 border-l-2 border-border pl-3">
          <div className="grid grid-cols-[1fr_5.5rem] gap-3">
            <Field label="SSH host">
              <Input
                value={ssh.host}
                onChange={(e) => patch({ host: e.target.value })}
                placeholder="bastion.example.com"
                spellCheck={false}
              />
            </Field>
            <Field label="Port">
              <Input
                type="number"
                value={ssh.port}
                onChange={(e) => patch({ port: Number(e.target.value) || 22 })}
              />
            </Field>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <Field label="SSH username">
              <Input
                value={ssh.username}
                onChange={(e) => patch({ username: e.target.value })}
                spellCheck={false}
                autoComplete="off"
              />
            </Field>
            <Field label="Authentication">
              <Select
                value={ssh.auth}
                onChange={(e) => patch({ auth: e.target.value as SshAuth })}
              >
                <option value="public_key">Private key</option>
                <option value="agent">SSH agent</option>
                <option value="password">Password</option>
              </Select>
            </Field>
          </div>

          {ssh.auth === "public_key" && (
            <Field label="Private key" hint="The passphrase, if any, goes in the field below.">
              <Input
                value={ssh.key_path ?? ""}
                onChange={(e) => patch({ key_path: e.target.value || undefined })}
                placeholder="C:\Users\you\.ssh\id_ed25519"
                spellCheck={false}
              />
            </Field>
          )}

          {ssh.auth !== "agent" && (
            <Field
              label={ssh.auth === "password" ? "SSH password" : "Key passphrase"}
              hint={
                secretIsStored && secret === undefined
                  ? "Leave blank to keep the saved credential."
                  : undefined
              }
            >
              <Input
                type="password"
                value={secret ?? ""}
                placeholder={secretIsStored && secret === undefined ? "••••••••" : ""}
                onChange={(e) => onSecret(e.target.value)}
                autoComplete="new-password"
              />
            </Field>
          )}

          {ssh.auth === "agent" && (
            <p className="text-[11px] text-text-muted">
              Signing is delegated to your SSH agent, so no private key is loaded into
              this application.
            </p>
          )}

          {/* Host key verification */}
          <div className="rounded-md border border-border bg-surface-2 p-2">
            <div className="flex items-start gap-2">
              <span className="min-w-0 flex-1">
                <span className="block text-[11px] font-medium text-text">Host key</span>
                {ssh.host_key_fingerprint ? (
                  <span
                    className="mt-0.5 block font-mono text-[10px] break-all text-ok"
                    data-selectable
                  >
                    {ssh.host_key_fingerprint}
                  </span>
                ) : (
                  <span className="mt-0.5 block text-[10.5px] text-warn">
                    Not verified. A tunnel will not open until you verify it.
                  </span>
                )}
              </span>
              <Button
                onClick={verify}
                busy={probing}
                disabled={!ssh.host.trim()}
                className="shrink-0"
              >
                {ssh.host_key_fingerprint ? "Re-verify" : "Verify"}
              </Button>
            </div>

            {error && (
              <div className="mt-2">
                <Banner tone="error" onDismiss={() => setError(null)}>
                  {error}
                </Banner>
              </div>
            )}

            {candidate && (
              <div
                className={cx(
                  "mt-2 rounded border p-2",
                  // A *changed* key on a host already trusted is the signature of
                  // an interception attempt, so it is styled and worded far more
                  // sharply than a first-time confirmation.
                  ssh.host_key_fingerprint
                    ? "border-danger/50 bg-danger/10"
                    : "border-border bg-surface-1",
                )}
              >
                {ssh.host_key_fingerprint && (
                  <p className="mb-1 text-[11px] font-semibold text-danger">
                    This host is presenting a different key than the one you trusted.
                    Do not accept it unless you know the server was rebuilt or rekeyed.
                  </p>
                )}
                <p className="text-[10.5px] text-text-muted">
                  {ssh.host_key_fingerprint ? "New fingerprint" : "The server presented"}:
                </p>
                <p className="mt-0.5 font-mono text-[10px] break-all text-text" data-selectable>
                  {candidate}
                </p>
                <p className="mt-1.5 text-[10.5px] text-text-muted">
                  Confirm it matches what the server administrator published before
                  accepting.
                </p>
                <div className="mt-2 flex gap-2">
                  <Button
                    variant={ssh.host_key_fingerprint ? "danger" : "primary"}
                    onClick={() => {
                      patch({ host_key_fingerprint: candidate });
                      setCandidate(null);
                    }}
                  >
                    Accept this key
                  </Button>
                  <Button variant="ghost" onClick={() => setCandidate(null)}>
                    Cancel
                  </Button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

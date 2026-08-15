/**
 * One hop of an SSH chain: where it is, who connects, and which key is trusted.
 *
 * The host key flow is the load-bearing part and it is per hop, because trust
 * does not travel along a chain. Reaching a jump host through a bastion you
 * already trust tells you nothing about the jump host, so each one is verified
 * on its own — and a hop can only be verified once everything in front of it
 * is, which is why they are confirmed front to back.
 */

import { useState } from "react";
import { Banner, Button, Field, Input, Select, cx } from "./ui/primitives";
import { ipc } from "@/lib/ipc";
import type { SshAuth, SshConfig } from "@/lib/types";

export function SshHopFields({
  hop,
  onChange,
  secret,
  onSecret,
  secretIsStored,
  /** Hops in front of this one, needed to reach it at all. */
  via,
  /** Their secrets, in order, for the same reason. */
  viaSecrets,
  compact = false,
}: {
  hop: SshConfig;
  onChange: (hop: SshConfig) => void;
  secret: string | undefined;
  onSecret: (value: string) => void;
  secretIsStored: boolean;
  via: SshConfig[];
  viaSecrets: (string | null)[];
  compact?: boolean;
}) {
  const [probing, setProbing] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const patch = (changes: Partial<SshConfig>) => onChange({ ...hop, ...changes });

  const unreachable = via.some((h) => !h.host_key_fingerprint);

  const verify = async () => {
    setProbing(true);
    setError(null);
    setCandidate(null);
    try {
      const fingerprint = await ipc.sshHostFingerprint({ ...hop, via }, viaSecrets);
      if (fingerprint === hop.host_key_fingerprint) {
        setCandidate(null);
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
    <div className="space-y-3">
      <div className="grid grid-cols-[1fr_5.5rem] gap-3">
        <Field label={compact ? "Jump host" : "SSH host"}>
          <Input
            value={hop.host}
            onChange={(e) => patch({ host: e.target.value })}
            placeholder="bastion.example.com"
            spellCheck={false}
          />
        </Field>
        <Field label="Port">
          <Input
            type="number"
            value={hop.port}
            onChange={(e) => patch({ port: Number(e.target.value) || 22 })}
          />
        </Field>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <Field label="SSH username">
          <Input
            value={hop.username}
            onChange={(e) => patch({ username: e.target.value })}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        <Field label="Authentication">
          <Select value={hop.auth} onChange={(e) => patch({ auth: e.target.value as SshAuth })}>
            <option value="public_key">Private key</option>
            <option value="agent">SSH agent</option>
            <option value="password">Password</option>
          </Select>
        </Field>
      </div>

      {hop.auth === "public_key" && (
        <Field label="Private key" hint="The passphrase, if any, goes in the field below.">
          <Input
            value={hop.key_path ?? ""}
            onChange={(e) => patch({ key_path: e.target.value || undefined })}
            placeholder="C:\Users\you\.ssh\id_ed25519"
            spellCheck={false}
          />
        </Field>
      )}

      {hop.auth !== "agent" && (
        <Field
          label={hop.auth === "password" ? "SSH password" : "Key passphrase"}
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

      {hop.auth === "agent" && (
        <p className="text-[11px] text-text-muted">
          Signing is delegated to your SSH agent, so no private key is loaded into this
          application.
        </p>
      )}

      <div className="rounded-md border border-border bg-surface-2 p-2">
        <div className="flex items-start gap-2">
          <span className="min-w-0 flex-1">
            <span className="block text-[11px] font-medium text-text">Host key</span>
            {hop.host_key_fingerprint ? (
              <span className="mt-0.5 block font-mono text-[10px] break-all text-ok" data-selectable>
                {hop.host_key_fingerprint}
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
            disabled={!hop.host.trim() || unreachable}
            className="shrink-0"
            title={
              unreachable
                ? "Verify the hops in front of this one first — it is reached through them"
                : undefined
            }
          >
            {hop.host_key_fingerprint ? "Re-verify" : "Verify"}
          </Button>
        </div>

        {unreachable && (
          <p className="mt-1.5 text-[10.5px] text-text-muted">
            This host is reached through the hops above it, so those have to be verified
            first.
          </p>
        )}

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
              // A *changed* key on a host already trusted is the signature of an
              // interception attempt, so it is worded far more sharply than a
              // first-time confirmation.
              hop.host_key_fingerprint
                ? "border-danger/50 bg-danger/10"
                : "border-border bg-surface-1",
            )}
          >
            {hop.host_key_fingerprint && (
              <p className="mb-1 text-[11px] font-semibold text-danger">
                This host is presenting a different key than the one you trusted. Do not
                accept it unless you know the server was rebuilt or rekeyed.
              </p>
            )}
            <p className="text-[10.5px] text-text-muted">
              {hop.host_key_fingerprint ? "New fingerprint" : "The server presented"}:
            </p>
            <p className="mt-0.5 font-mono text-[10px] break-all text-text" data-selectable>
              {candidate}
            </p>
            <p className="mt-1.5 text-[10.5px] text-text-muted">
              Confirm it matches what the server administrator published before accepting.
            </p>
            <div className="mt-2 flex gap-2">
              <Button
                variant={hop.host_key_fingerprint ? "danger" : "primary"}
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
  );
}

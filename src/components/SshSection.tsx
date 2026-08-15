/**
 * SSH tunnel configuration, one or more hops deep.
 *
 * The host key flow is the load-bearing part. The backend refuses to open a
 * tunnel to a host whose fingerprint it does not already know, so this section
 * has to fetch each fingerprint, show it, and get an explicit confirmation
 * before a tunnelled connection can work at all. That is deliberate: silently
 * trusting whatever key appears would defeat the point of tunnelling.
 *
 * Hops are listed in the order they are traversed, with the database behind the
 * last one. They are verified in that order too, and not by choice — an
 * internal jump host is exactly the machine that cannot be reached directly, so
 * reading its key means going through everything in front of it.
 */

import { Button, Checkbox } from "./ui/primitives";
import { SshHopFields } from "./SshHopFields";
import type { SshConfig } from "@/lib/types";

function blankSsh(): SshConfig {
  return { host: "", port: 22, username: "", auth: "public_key" };
}

export function SshSection({
  ssh,
  onChange,
  secrets,
  onSecret,
  storedCount,
}: {
  ssh: SshConfig | undefined;
  onChange: (ssh: SshConfig | undefined) => void;
  /** One per hop, in chain order. Empty string clears; undefined is untouched. */
  secrets: (string | undefined)[];
  onSecret: (index: number, value: string) => void;
  /** How many hops already have a credential in the keychain. */
  storedCount: number;
}) {
  const enabled = ssh !== undefined;
  if (!enabled || !ssh) {
    return (
      <div className="border-t border-border pt-3">
        <Checkbox
          label="Connect through an SSH tunnel"
          hint="For databases reachable only from a bastion host."
          checked={false}
          onChange={(on) => onChange(on ? blankSsh() : undefined)}
        />
      </div>
    );
  }

  const via = ssh.via ?? [];
  // The chain as the tunnel sees it: jump hosts first, the bastion the database
  // sits behind last.
  const hops = [...via, ssh];

  const setHop = (index: number, next: SshConfig) => {
    if (index === hops.length - 1) {
      onChange({ ...next, via });
      return;
    }
    const nextVia = [...via];
    nextVia[index] = next;
    onChange({ ...ssh, via: nextVia });
  };

  const addJumpHost = () => onChange({ ...ssh, via: [...via, blankSsh()] });

  const removeJumpHost = (index: number) =>
    onChange({ ...ssh, via: via.filter((_, i) => i !== index) });

  return (
    <div className="border-t border-border pt-3">
      <Checkbox
        label="Connect through an SSH tunnel"
        hint="For databases reachable only from a bastion host."
        checked
        onChange={(on) => onChange(on ? blankSsh() : undefined)}
      />

      <div className="mt-3 space-y-3 border-l-2 border-border pl-3">
        {hops.map((hop, index) => {
          const isLast = index === hops.length - 1;
          return (
            <div key={index} className={index > 0 ? "border-t border-border/60 pt-3" : undefined}>
              <div className="mb-2 flex items-center gap-2">
                <span className="text-[11px] font-medium text-text">
                  {isLast
                    ? hops.length > 1
                      ? `${index + 1}. Bastion — the database is reachable from here`
                      : "Bastion"
                    : `${index + 1}. Jump host`}
                </span>
                <div className="flex-1" />
                {!isLast && (
                  <Button variant="ghost" className="h-5" onClick={() => removeJumpHost(index)}>
                    Remove
                  </Button>
                )}
              </div>

              <SshHopFields
                hop={hop}
                onChange={(next) => setHop(index, next)}
                secret={secrets[index]}
                onSecret={(value) => onSecret(index, value)}
                secretIsStored={index < storedCount}
                // Everything in front of this hop, which is what it is reached
                // through — and what has to be trusted before it can be.
                via={hops.slice(0, index)}
                viaSecrets={secrets.slice(0, index).map((s) => s ?? null)}
                compact={!isLast}
              />
            </div>
          );
        })}

        <div className="flex items-center gap-2 border-t border-border/60 pt-2">
          <Button variant="ghost" className="h-6" onClick={addJumpHost}>
            Add a jump host
          </Button>
          <span className="text-[10.5px] text-text-muted">
            Passed through before the bastion, in the order listed.
          </span>
        </div>
      </div>
    </div>
  );
}

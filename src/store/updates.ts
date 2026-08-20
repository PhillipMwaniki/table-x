/**
 * Whether a newer release exists, asked at most once a day.
 *
 * Startup is the right moment to ask and the wrong frequency to ask at: someone
 * who opens the app twenty times a day would make twenty identical requests for
 * information that changes every few weeks. So the trigger is launch and the
 * gate is a timestamp, persisted beside the appearance settings.
 *
 * Nothing here surfaces an error. A failed check leaves the badge off, which is
 * the same as having no news — deliberately not the same as reporting that you
 * are up to date, which is a claim this code cannot make when it did not hear
 * back, and is how somebody misses a security fix.
 */

import { create } from "zustand";
import { load as loadStore } from "@tauri-apps/plugin-store";
import type { Store } from "@tauri-apps/plugin-store";
import { ipc } from "@/lib/ipc";
import type { UpdateInfo } from "@/lib/types";

const FILE = "settings.json";
const KEY = "updates";

/** The floor, used until the endpoint says otherwise. */
const DEFAULT_INTERVAL_SECONDS = 86_400;

interface Persisted {
  /** Epoch milliseconds of the last completed check, successful or not. */
  lastCheckedAt: number;
  /** Seconds to wait, as the endpoint last asked. */
  interval: number;
  /** A version the user has already been told about and dismissed. */
  dismissed: string;
}

const EMPTY: Persisted = { lastCheckedAt: 0, interval: DEFAULT_INTERVAL_SECONDS, dismissed: "" };

interface UpdateState {
  /** The new release, once one is known and not dismissed. */
  available: UpdateInfo | null;

  /** Called at launch. Does nothing if switched off or checked recently. */
  check: (enabled: boolean) => Promise<void>;
  /** Stop showing this version. A later one will still be announced. */
  dismiss: () => Promise<void>;
}

let store: Store | null = null;
async function handle(): Promise<Store> {
  store ??= await loadStore(FILE, { autoSave: false });
  return store;
}

async function read(): Promise<Persisted> {
  try {
    const raw = await (await handle()).get<Partial<Persisted>>(KEY);
    return {
      lastCheckedAt: Number(raw?.lastCheckedAt) || 0,
      interval: Number(raw?.interval) || DEFAULT_INTERVAL_SECONDS,
      dismissed: typeof raw?.dismissed === "string" ? raw.dismissed : "",
    };
  } catch {
    // An unreadable file costs the throttle, not the feature.
    return { ...EMPTY };
  }
}

async function write(next: Persisted): Promise<void> {
  try {
    const s = await handle();
    await s.set(KEY, next);
    await s.save();
  } catch {
    // Failing to record the check means asking again next launch, which is a
    // far smaller problem than surfacing a write error over a version number.
  }
}

export const useUpdates = create<UpdateState>((set) => ({
  available: null,

  check: async (enabled) => {
    if (!enabled) return;

    const state = await read();
    const due = Date.now() - state.lastCheckedAt >= state.interval * 1000;
    if (!due) return;

    // Silent by design on every failure path. See the note at the top.
    const info: UpdateInfo | null = await ipc.checkForUpdate().catch(() => null);

    // Recorded even when the check failed, so an endpoint that is down is asked
    // once a day rather than on every launch until it returns.
    await write({
      lastCheckedAt: Date.now(),
      interval: info?.check_again_in ?? state.interval,
      dismissed: state.dismissed,
    });

    if (info && info.latest !== state.dismissed) set({ available: info });
  },

  dismiss: async () => {
    const current = useUpdates.getState().available;
    if (!current) return;
    const state = await read();
    await write({ ...state, dismissed: current.latest });
    set({ available: null });
  },
}));

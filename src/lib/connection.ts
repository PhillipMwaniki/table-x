/**
 * Moving a half-filled connection form between drivers.
 *
 * Picking the driver is the first control on the form and the one people get
 * wrong, so it is changed *after* the rest has been filled in as often as
 * before. What the user typed has to survive that: a host, a database name and
 * a whole SSH chain are the same facts whichever engine is listening on the
 * other end.
 */

import type { ConnectionConfig, DriverInfo } from "./types";

/**
 * The driver a new connection or design starts on.
 *
 * Not simply the first registered one, which is whichever module happens to be
 * listed first in the registry — today ClickHouse, which is nobody's likely
 * answer to "New connection". Drivers sit behind Cargo features, so a build
 * without this one falls back to the first it does have rather than offering
 * nothing at all.
 */
export const DEFAULT_DRIVER = "mysql";

export function defaultDriver(drivers: DriverInfo[]): DriverInfo | undefined {
  return drivers.find((d) => d.id === DEFAULT_DRIVER) ?? drivers[0];
}

/**
 * Point a config at another driver, keeping everything that still means
 * something.
 *
 * The port is the one field with a real decision in it. Left at the old
 * driver's default it is not a choice at all — it is what the form filled in —
 * so it moves to the new driver's default; MySQL's 3306 carried over to
 * PostgreSQL would be a connection that cannot work. Typed by hand it *is* a
 * choice, and it is kept.
 *
 * The driver's own options go, because they belong to the driver being left:
 * they are named per engine, and carrying them across would send one engine's
 * settings to another that has never heard of them.
 */
export function retargetConfig(
  config: ConnectionConfig,
  from: DriverInfo | undefined,
  to: DriverInfo,
): ConnectionConfig {
  const typedByHand = config.port != null && config.port !== from?.default_port;

  return {
    ...config,
    driver: to.id,
    // A file-based driver shows neither of these, so they are left untouched
    // rather than cleared: switching to SQLite to look at it and back again
    // should not cost the host somebody typed. `forDriver` drops them on the
    // way to being saved, so nothing irrelevant reaches the file.
    host: to.file_based ? config.host : (config.host ?? "localhost"),
    port: to.file_based ? config.port : typedByHand ? config.port : (to.default_port ?? undefined),
    options: {},
  };
}

/**
 * Drop the fields the chosen driver has no use for.
 *
 * Applied on the way out of the form rather than while it is being edited. The
 * two want opposite things: an edit in progress should lose nothing, and a
 * saved connection should not carry a database file for a MySQL server or a
 * port for SQLite — fields nothing reads, that suggest a setting exists where
 * none does.
 */
/**
 * Whether a write against `database` on this connection touches production.
 *
 * The same rule the backend applies, so the badge and the gate agree: the
 * whole connection, or a list of names compared without regard to case,
 * since two of the engines fold them and a list typed by hand should not have
 * to know which. A connection saved before the setting existed is not
 * production.
 */
export function inProduction(config: ConnectionConfig, database: string | null): boolean {
  const scope = config.production ?? { scope: "none" };
  switch (scope.scope) {
    case "none":
      return false;
    case "all":
      return true;
    case "databases":
      return (
        database !== null &&
        scope.names.some((n) => n.trim().toLowerCase() === database.toLowerCase())
      );
  }
}

/** Whether any part of this connection is production, for the sidebar's badge. */
export function hasProduction(config: ConnectionConfig): boolean {
  const scope = config.production ?? { scope: "none" };
  return scope.scope === "all" || (scope.scope === "databases" && scope.names.length > 0);
}

export function forDriver(config: ConnectionConfig, driver: DriverInfo): ConnectionConfig {
  if (driver.file_based) {
    return {
      ...config,
      host: undefined,
      port: undefined,
      database: undefined,
      username: undefined,
    };
  }
  return { ...config, file_path: undefined };
}

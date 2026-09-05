import { describe, expect, it } from "vitest";
import {
  defaultDriver,
  forDriver,
  hasProduction,
  inProduction,
  retargetConfig,
} from "./connection";
import type { ConnectionConfig, DriverInfo } from "./types";

function driver(id: string, overrides: Partial<DriverInfo> = {}): DriverInfo {
  return {
    id,
    name: id,
    default_port: null,
    file_based: false,
    capabilities: {} as DriverInfo["capabilities"],
    ...overrides,
  };
}

const mysql = driver("mysql", { default_port: 3306 });
const postgres = driver("postgres", { default_port: 5432 });
const sqlite = driver("sqlite", { file_based: true });

function config(overrides: Partial<ConnectionConfig> = {}): ConnectionConfig {
  return {
    id: "abc",
    name: "Production",
    driver: "mysql",
    host: "db.internal",
    port: 3306,
    database: "app",
    username: "reader",
    tls: { mode: "verify_full" },
    read_only: true,
    options: {},
    ...overrides,
  };
}

describe("retargetConfig", () => {
  it("keeps everything the user typed", () => {
    // The bug this exists to prevent: filling in the form, noticing the driver
    // is wrong, and having the form emptied as the price of fixing it.
    const next = retargetConfig(config(), mysql, postgres);

    expect(next.driver).toBe("postgres");
    expect(next.name).toBe("Production");
    expect(next.host).toBe("db.internal");
    expect(next.database).toBe("app");
    expect(next.username).toBe("reader");
    expect(next.tls.mode).toBe("verify_full");
    expect(next.read_only).toBe(true);
    expect(next.id).toBe("abc");
  });

  it("keeps an SSH chain, which is the most expensive thing on the form", () => {
    const ssh = {
      host: "bastion",
      port: 22,
      username: "deploy",
      auth: "public_key" as const,
      key_path: "~/.ssh/id_ed25519",
    };
    expect(retargetConfig(config({ ssh }), mysql, postgres).ssh).toEqual(ssh);
  });

  it("moves a port that was only ever the old default", () => {
    // 3306 was filled in by the form, not chosen. Carried to PostgreSQL it is
    // a connection that cannot succeed.
    expect(retargetConfig(config({ port: 3306 }), mysql, postgres).port).toBe(5432);
  });

  it("keeps a port that was typed by hand", () => {
    expect(retargetConfig(config({ port: 3307 }), mysql, postgres).port).toBe(3307);
  });

  it("fills in the new default when there was no port at all", () => {
    expect(retargetConfig(config({ port: undefined }), sqlite, mysql).port).toBe(3306);
  });

  it("gives a host back when coming from a driver that had none", () => {
    const from = config({ driver: "sqlite", host: undefined, port: undefined });
    expect(retargetConfig(from, sqlite, mysql).host).toBe("localhost");
  });

  it("leaves the host and port alone on the way to a file", () => {
    // Neither is shown for SQLite, so clearing them would quietly cost the
    // host on the way back — which is the same bug in a longer form.
    const next = retargetConfig(config({ port: 3307 }), mysql, sqlite);
    expect(next.host).toBe("db.internal");
    expect(next.port).toBe(3307);
  });

  it("drops the previous driver's own options", () => {
    // Named per engine: what MySQL calls a setting, PostgreSQL has never heard
    // of.
    const next = retargetConfig(config({ options: { allow_cleartext: "true" } }), mysql, postgres);
    expect(next.options).toEqual({});
  });
});

describe("forDriver", () => {
  it("saves a file-based connection without a server on it", () => {
    const next = forDriver(config({ file_path: "/data/app.db" }), sqlite);
    expect(next.file_path).toBe("/data/app.db");
    expect(next.host).toBeUndefined();
    expect(next.port).toBeUndefined();
    expect(next.database).toBeUndefined();
    expect(next.username).toBeUndefined();
  });

  it("saves a server connection without a file on it", () => {
    const next = forDriver(config({ file_path: "/data/app.db" }), mysql);
    expect(next.file_path).toBeUndefined();
    expect(next.host).toBe("db.internal");
  });
});

describe("defaultDriver", () => {
  it("starts on MySQL rather than on whichever driver sorts first", () => {
    // The registry's order is an accident of how the modules are listed, and
    // the first one is ClickHouse -- nobody's likely answer to "new design".
    const clickhouse = driver("clickhouse");
    expect(defaultDriver([clickhouse, postgres, mysql])?.id).toBe("mysql");
  });

  it("falls back to the first driver a build actually has", () => {
    // Drivers sit behind Cargo features, so a build without MySQL still has to
    // open the form on something.
    expect(defaultDriver([postgres, sqlite])?.id).toBe("postgres");
  });

  it("has nothing to offer a build with no drivers at all", () => {
    expect(defaultDriver([])).toBeUndefined();
  });
});

describe("inProduction", () => {
  it("is nothing for a connection that never said", () => {
    expect(inProduction(config(), "app")).toBe(false);
    expect(hasProduction(config())).toBe(false);
  });

  it("covers every database when the whole connection is production", () => {
    const c = config({ production: { scope: "all" } });
    expect(inProduction(c, "anything")).toBe(true);
    // A file database has no name to check and is still production.
    expect(inProduction(c, null)).toBe(true);
    expect(hasProduction(c)).toBe(true);
  });

  it("matches listed databases by name, without regard to case", () => {
    const c = config({ production: { scope: "databases", names: ["App_Prod", " billing "] } });
    expect(inProduction(c, "app_prod")).toBe(true);
    expect(inProduction(c, "billing")).toBe(true);
    expect(inProduction(c, "app_staging")).toBe(false);
    expect(inProduction(c, null)).toBe(false);
    expect(hasProduction(c)).toBe(true);
    expect(hasProduction(config({ production: { scope: "databases", names: [] } }))).toBe(false);
  });
});

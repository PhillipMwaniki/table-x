import { describe, expect, it } from "vitest";
import { relativeTime } from "./HistoryPanel";

/** Fixed "now" so these never depend on when the suite runs. */
const NOW = Date.parse("2026-03-01T12:00:00Z");

function ago(ms: number): string {
  return new Date(NOW - ms).toISOString();
}

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

describe("relativeTime", () => {
  it("collapses the last minute to 'just now'", () => {
    expect(relativeTime(ago(20 * SECOND), NOW)).toBe("just now");
  });

  it("counts minutes, hours, and days", () => {
    expect(relativeTime(ago(5 * MINUTE), NOW)).toBe("5m ago");
    expect(relativeTime(ago(3 * HOUR), NOW)).toBe("3h ago");
    expect(relativeTime(ago(2 * DAY), NOW)).toBe("2d ago");
  });

  it("falls back to a date once the elapsed time stops being the useful part", () => {
    // Past a week, "12d ago" is harder to place than the date itself.
    const formatted = relativeTime(ago(12 * DAY), NOW);
    expect(formatted).not.toContain("ago");
    expect(formatted).toBe(new Date(NOW - 12 * DAY).toLocaleDateString());
  });

  it("never renders a future timestamp as a negative age", () => {
    // Clock skew between the machine that wrote the entry and this one must not
    // produce "-3m ago".
    expect(relativeTime(ago(-3 * MINUTE), NOW)).toBe("just now");
  });

  it("returns an unparseable timestamp verbatim rather than 'Invalid Date'", () => {
    expect(relativeTime("not a date", NOW)).toBe("not a date");
  });
});

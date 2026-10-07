import { describe, expect, it } from "vitest";
import { sameStatus } from "./serverStatus";

describe("sameStatus", () => {
  it("treats identical statuses as equal", () => {
    expect(sameStatus({ type: "stopped" }, { type: "stopped" })).toBe(true);
    expect(sameStatus({ type: "starting" }, { type: "starting" })).toBe(true);
    expect(
      sameStatus(
        { type: "running", port: 8080, pid: 42, ready: false },
        { type: "running", port: 8080, pid: 42, ready: false },
      ),
    ).toBe(true);
    expect(
      sameStatus(
        { type: "error", message: "boom" },
        { type: "error", message: "boom" },
      ),
    ).toBe(true);
  });

  it("detects every field change", () => {
    const run = (port: number, pid: number, ready: boolean) =>
      ({ type: "running" as const, port, pid, ready });
    expect(sameStatus({ type: "stopped" }, { type: "starting" })).toBe(false);
    expect(sameStatus(run(8080, 42, false), run(8080, 42, true))).toBe(false);
    expect(sameStatus(run(8080, 42, true), run(8081, 42, true))).toBe(false);
    expect(sameStatus(run(8080, 42, true), run(8080, 43, true))).toBe(false);
    expect(
      sameStatus(
        { type: "error", message: "a" },
        { type: "error", message: "b" },
      ),
    ).toBe(false);
    expect(sameStatus({ type: "running", port: 1, pid: 1, ready: true }, { type: "stopped" })).toBe(
      false,
    );
  });
});

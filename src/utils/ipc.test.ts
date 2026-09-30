import { describe, expect, it } from "vitest";
import { fmtGB, fmtMB } from "./ipc";

describe("fmtMB", () => {
  it("rounds megabytes to one decimal", () => {
    expect(fmtMB(593.74609375)).toBe("593.7 MB");
    expect(fmtMB(0)).toBe("0.0 MB");
  });

  it("switches to gigabytes at 1024", () => {
    expect(fmtMB(1024)).toBe("1.0 GB");
    expect(fmtMB(404.5)).toBe("404.5 MB");
  });
});

describe("fmtGB", () => {
  it("formats bytes as gigabytes", () => {
    expect(fmtGB(1024 ** 3)).toBe("1.0 GB");
  });
});

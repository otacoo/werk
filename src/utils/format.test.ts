import { describe, expect, it } from "vitest";
import {
  formatSize,
  isDsparkFile,
  isImatrixFile,
  isMmprojFile,
  mmprojSaveName,
  parseSplitSuffix,
  quantFromName,
  quantSortKey,
} from "./format";

describe("formatSize", () => {
  it("formats bytes", () => {
    expect(formatSize(1024 ** 3)).toBe("1.0 GB");
    expect(formatSize(512 * 1024 ** 2)).toBe("512.00 MB");
  });
});

describe("quantSortKey", () => {
  it("orders by precision", () => {
    expect(quantSortKey("Q8_0")).toBeLessThan(quantSortKey("Q4_K_M"));
    expect(quantSortKey("Q4_K_M")).toBeLessThan(quantSortKey("Q2_K"));
    expect(quantSortKey(null)).toBe(999);
  });
});

describe("quantFromName", () => {
  it("extracts trailing quant tags", () => {
    expect(quantFromName("model-Q4_K_M.gguf")).toBe("Q4_K_M");
    expect(quantFromName("model-f16.gguf")).toBe("F16");
    expect(quantFromName("plain.gguf")).toBeNull();
  });
});

describe("parseSplitSuffix", () => {
  it("parses part notation", () => {
    expect(parseSplitSuffix("m-00001-of-00005.gguf")).toEqual({ base: "m", index: 1, total: 5 });
    expect(parseSplitSuffix("single.gguf")).toBeNull();
    expect(parseSplitSuffix("m-00000-of-00005.gguf")).toBeNull();
  });
});

describe("companions", () => {
  it("detects sidecar files", () => {
    expect(isMmprojFile("model-mmproj-f16.gguf")).toBe(true);
    expect(isDsparkFile("model-dspark.gguf")).toBe(true);
    expect(isImatrixFile("model.imatrix")).toBe(true);
    expect(isMmprojFile("model-Q4_K_M.gguf")).toBe(false);
  });

  it("prefixes sidecar names with the model stem", () => {
    expect(mmprojSaveName("Qwen.gguf", "mmproj-f16.gguf")).toBe("Qwen-mmproj-f16.gguf");
  });
});

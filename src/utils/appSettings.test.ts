import { describe, expect, it } from "vitest";
import { getQuickBench, setQuickBench, subscribeQuickBench } from "./appSettings";

describe("quick bench store", () => {
  it("starts disabled", () => {
    expect(getQuickBench()).toBe(false);
  });

  it("notifies subscribers only when the value changes", () => {
    const start = getQuickBench();
    const other = !start;
    const seen: boolean[] = [];
    const unsub = subscribeQuickBench((v) => seen.push(v));
    setQuickBench(other);
    setQuickBench(other);
    unsub();
    setQuickBench(start);
    expect(seen).toEqual([other]);
    expect(getQuickBench()).toBe(start);
  });
});

import { describe, expect, it } from "vitest";
import { bytesToBase64, encodeWav } from "./recorder";

describe("encodeWav", () => {
  it("writes a PCM16 mono header", () => {
    const samples = new Float32Array([0, 1, -1]);
    const bytes = encodeWav(samples, 16000);
    const view = new DataView(bytes.buffer);
    const str = (o: number, n: number) =>
      String.fromCharCode(...bytes.subarray(o, o + n));
    expect(bytes.length).toBe(44 + 6);
    expect(str(0, 4)).toBe("RIFF");
    expect(str(8, 4)).toBe("WAVE");
    expect(str(36, 4)).toBe("data");
    expect(view.getUint32(4, true)).toBe(36 + 6);
    expect(view.getUint16(20, true)).toBe(1);
    expect(view.getUint16(22, true)).toBe(1);
    expect(view.getUint32(24, true)).toBe(16000);
    expect(view.getUint16(34, true)).toBe(16);
    expect(view.getUint32(40, true)).toBe(6);
    expect(view.getInt16(44, true)).toBe(0);
    expect(view.getInt16(46, true)).toBe(0x7fff);
    expect(view.getInt16(48, true)).toBe(-0x8000);
  });

  it("clamps out-of-range samples", () => {
    const bytes = encodeWav(new Float32Array([2, -2]), 8000);
    const view = new DataView(bytes.buffer);
    expect(view.getInt16(44, true)).toBe(0x7fff);
    expect(view.getInt16(46, true)).toBe(-0x8000);
  });
});

describe("bytesToBase64", () => {
  it("round-trips across chunk boundaries", () => {
    const bytes = new Uint8Array(0x8000 + 5);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i % 251;
    const b64 = bytesToBase64(bytes);
    const back = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    expect(back).toEqual(bytes);
  });
});

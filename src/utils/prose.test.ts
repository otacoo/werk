import { describe, expect, it } from "vitest";
import {
  closePartialEmphasis,
  collapseSpeakerRepeats,
  dialogueSegments,
  speakerHue,
  speakerPrefixOf,
} from "./prose";

describe("dialogueSegments", () => {
  it("marks quoted runs and leaves prose alone", () => {
    const segs = dialogueSegments('She nods. "Hello there," she says.');
    expect(segs.map((s) => [s.dialogue, s.text])).toEqual([
      [false, "She nods. "],
      [true, '"Hello there,"'],
      [false, " she says."],
    ]);
  });

  it("does not treat inch marks as dialogue", () => {
    const segs = dialogueSegments('He is 5ft8" tall and said "hi".');
    const dialogue = segs.filter((s) => s.dialogue).map((s) => s.text);
    expect(dialogue).toEqual(['"hi"']);
  });

  it("handles curly quotes and unterminated openers", () => {
    expect(dialogueSegments("\u201cHi\u201d there")[0]).toEqual({ dialogue: true, text: "\u201cHi\u201d" });
    // Raw when final, wrapped to end-of-line while streaming.
    const final = dialogueSegments('He said "hello');
    expect(final.some((s) => s.dialogue)).toBe(false);
    const live = dialogueSegments('He said "hello', true);
    expect(live[live.length - 1]).toEqual({ dialogue: true, text: '"hello' });
  });
});

describe("closePartialEmphasis", () => {
  it("closes an unterminated marker with content after it", () => {
    expect(closePartialEmphasis("*she wav")).toBe("*she wav*");
    expect(closePartialEmphasis("**bold")).toBe("**bold**");
  });

  it("leaves balanced and bare markers alone", () => {
    expect(closePartialEmphasis("*done*")).toBe("*done*");
    // A lone trailing star has no content to italicize; it stays literal.
    expect(closePartialEmphasis("a *")).toBe("a *");
    expect(closePartialEmphasis("a * ")).toBe("a * ");
  });
});

describe("speakerPrefixOf", () => {
  const names = ["Mia", "Ivan"];

  it("strips known prefixes in every shape", () => {
    expect(speakerPrefixOf("Mia: hello", names)).toEqual({ name: "Mia", text: "hello" });
    expect(speakerPrefixOf("**Mia:** hello", names)).toEqual({ name: "Mia", text: "hello" });
    expect(speakerPrefixOf("*Mia*: hello", names)).toEqual({ name: "Mia", text: "hello" });
    expect(speakerPrefixOf("Narrator: the room", names)).toEqual({
      name: "Narrator",
      text: "the room",
    });
  });

  it("keeps an action's opening star", () => {
    expect(speakerPrefixOf("Mia: *waves*", names)).toEqual({ name: "Mia", text: "*waves*" });
  });

  it("ignores unknown names and missing prefixes", () => {
    expect(speakerPrefixOf("Zoe: hi", names)).toBeNull();
    expect(speakerPrefixOf("just prose", names)).toBeNull();
  });
});

describe("collapseSpeakerRepeats", () => {
  it("drops repeats for the current speaker only", () => {
    const names = ["Mia", "Ivan"];
    expect(collapseSpeakerRepeats("Mia: hi\nMia: again\nIvan: yo\nNarrator: then", names)).toBe(
      "Mia: hi\nagain\nIvan: yo\nNarrator: then",
    );
  });

  it("leaves unknown names untouched", () => {
    expect(collapseSpeakerRepeats("Zoe: hi\nZoe: ho", ["Mia"])).toBe("Zoe: hi\nZoe: ho");
  });
});

describe("speakerHue", () => {
  it("is stable and name-specific", () => {
    expect(speakerHue("Mia")).toBe(speakerHue("Mia"));
    expect(speakerHue("Mia")).not.toBe(speakerHue("Ivan"));
    expect(speakerHue("Mia")).toBeGreaterThanOrEqual(0);
    expect(speakerHue("Mia")).toBeLessThan(360);
  });
});

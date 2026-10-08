//! Roleplay prose helpers: dialogue spans, streaming emphasis, and speaker
//! prefixes. Pure functions so the display and the tests share one behavior.

export interface DialogueSegment {
  dialogue: boolean;
  text: string;
}

const WORD = /[\p{L}\p{N}]/u;

/// Does a straight quote at `i` open dialogue? Not after a letter or digit
/// (`5ft8"`, `rock"in'`) and not before whitespace or another quote, or a
/// stray inch mark would pair with the next real quote and eat the text
/// between. Curly quotes always open.
function opensDialogue(line: string, i: number): "straight" | "curly" | null {
  const ch = line[i];
  if (ch === "\u201c") return "curly";
  if (ch !== '"') return null;
  const prev = line[i - 1] ?? "";
  const next = line[i + 1] ?? " ";
  if (WORD.test(prev)) return null;
  if (next === "" || /[\s"\u201c]/.test(next)) return null;
  return "straight";
}

/// Split one line into plain and dialogue runs. Unterminated quotes stay raw
/// unless `closeOpen` (live streaming), where an opener wraps to end-of-line
/// so the partial sentence colors as it arrives; the final render shows the
/// raw truth.
function segmentsOfLine(line: string, closeOpen: boolean): DialogueSegment[] {
  const out: DialogueSegment[] = [];
  const push = (dialogue: boolean, text: string) => {
    if (!text) return;
    const last = out[out.length - 1];
    if (last && last.dialogue === dialogue) last.text += text;
    else out.push({ dialogue, text });
  };
  let i = 0;
  while (i < line.length) {
    const opener = opensDialogue(line, i);
    if (!opener) {
      push(false, line[i]);
      i += 1;
      continue;
    }
    const closer = opener === "curly" ? "\u201d" : '"';
    const end = line.indexOf(closer, i + 1);
    if (end === -1) {
      if (closeOpen) push(true, line.slice(i));
      else push(false, line[i]);
      i = closeOpen ? line.length : i + 1;
      continue;
    }
    push(true, line.slice(i, end + 1));
    i = end + 1;
  }
  return out;
}

/// Dialogue segments for a whole block, newlines preserved. Fenced code is
/// the caller's concern (the markdown pass skips code nodes).
export function dialogueSegments(text: string, closeOpen = false): DialogueSegment[] {
  const out: DialogueSegment[] = [];
  text.split("\n").forEach((line, index) => {
    if (index > 0) out.push({ dialogue: false, text: "\n" });
    for (const seg of segmentsOfLine(line, closeOpen)) {
      const last = out[out.length - 1];
      if (last && last.dialogue === seg.dialogue) last.text += seg.text;
      else out.push(seg);
    }
  });
  return out;
}

/// Streaming only: tentatively close an unterminated `*`/`**` so a partial
/// reply formats as it grows (`*she wav` renders italic immediately). The
/// final render uses the raw text, so an unclosed marker snaps back off.
export function closePartialEmphasis(text: string): string {
  const scan = text.replace(/\\./g, "x");
  let out = text;
  if (
    (scan.match(/\*\*/g) ?? []).length % 2 === 1 &&
    /[^\s*]/.test(scan.slice(scan.lastIndexOf("**") + 2))
  ) {
    out += "**";
  }
  const rest = scan.replace(/\*\*/g, "");
  if ((rest.match(/\*/g) ?? []).length % 2 === 1 && /[^\s*]/.test(rest.slice(rest.lastIndexOf("*") + 1))) {
    out += "*";
  }
  return out;
}

/// `{ name, text }` when the message starts with a speaker prefix for a known
/// name (`Name:`, `**Name:**`, `*Name*:`) or `Narrator:`; the prefix is
/// stripped from `text`. Unknown names return null and stay plain text.
export function speakerPrefixOf(
  text: string,
  names: string[],
): { name: string; text: string } | null {
  const trimmed = text.replace(/^\s+/, "");
  const body = trimmed.replace(/^\*+/, "");
  const colon = body.indexOf(":");
  if (colon < 0) return null;
  const candidate = body
    .slice(0, colon)
    .trim()
    .replace(/\*+$/, "")
    .trim();
  if (!candidate) return null;
  const known =
    candidate.toLowerCase() === "narrator"
      ? "Narrator"
      : names.find((n) => n.trim().toLowerCase() === candidate.toLowerCase());
  if (!known) return null;
  let rest = body.slice(colon + 1).replace(/^\s+/, "");
  const afterStars = rest.replace(/^\*+/, "");
  if (afterStars.length < rest.length && (afterStars === "" || /^\s/.test(afterStars))) {
    rest = afterStars.replace(/^\s+/, "");
  }
  return { name: known.trim(), text: rest };
}

/// Drop speaker prefixes repeated inside the same speaker's stretch
/// ("Mia: … Mia: …") for display; the model sees the same cleaned form.
export function collapseSpeakerRepeats(text: string, names: string[]): string {
  if (!text || names.length === 0) return text;
  let current = "";
  const out: string[] = [];
  for (const line of text.split("\n")) {
    const split = speakerPrefixOf(line, names);
    if (split && split.name === current) out.push(split.text);
    else {
      if (split) current = split.name;
      out.push(line);
    }
  }
  return out.join("\n");
}

/// Deterministic per-speaker hue (0-359) for name labels.
export function speakerHue(name: string): number {
  let h = 0;
  for (const ch of name) h = (h * 31 + (ch.codePointAt(0) ?? 0)) % 360;
  return h;
}

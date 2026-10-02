//! UI-only appearance prefs (localStorage): theme, accent, font, corners.
//! Applied as data attributes; components use semantic tokens.

export type ThemeName = "system" | "day" | "night" | "werk" | "gemma" | "migu" | "nerv";
export type AccentName = "blue" | "violet" | "red" | "green" | "amber";
export type CornersMode = "auto" | "sharp" | "round";
export type FontSizeName = "compact" | "default" | "large" | "xlarge";
export type BubbleAlign = "left" | "right";

export const ACCENTS: Record<AccentName, { label: string; rgb: string }> = {
  blue: { label: "Blue", rgb: "0 120 215" },
  violet: { label: "Violet", rgb: "124 108 255" },
  red: { label: "Red", rgb: "229 72 77" },
  green: { label: "Green", rgb: "70 167 88" },
  amber: { label: "Amber", rgb: "232 161 60" },
};

/// Earthy counterparts used by the sand-toned werk theme.
export const EARTHY_ACCENTS: Record<AccentName, { label: string; rgb: string; soft: string }> = {
  blue: { label: "Eucalyptus", rgb: "70 101 95", soft: "54 82 77" },
  violet: { label: "Plum", rgb: "110 90 115", soft: "88 70 93" },
  red: { label: "Terracotta", rgb: "169 85 63", soft: "140 66 47" },
  green: { label: "Moss", rgb: "95 127 60", soft: "74 107 45" },
  amber: { label: "Ochre", rgb: "188 138 60", soft: "158 113 44" },
};

/// Character palette for the Gemma theme: sky blue, white, starlight gold.
export const GEMMA_ACCENTS: Record<AccentName, { label: string; rgb: string; soft: string }> = {
  blue: { label: "Sky", rgb: "47 143 216", soft: "31 112 175" },
  violet: { label: "Periwinkle", rgb: "106 118 224", soft: "82 92 190" },
  red: { label: "Rose", rgb: "214 88 110", soft: "178 62 84" },
  green: { label: "Teal", rgb: "42 158 138", soft: "30 128 110" },
  amber: { label: "Star", rgb: "214 158 46", soft: "176 126 26" },
};

/// Character palette for the Migu theme: teal and silver with neon pink
/// highlights (each primary pairs with the opposite hue as its soft accent).
export const MIGU_ACCENTS: Record<AccentName, { label: string; rgb: string; soft: string }> = {
  blue: { label: "Teal", rgb: "57 197 187", soft: "244 114 182" },
  violet: { label: "Iris", rgb: "138 148 219", soft: "244 114 182" },
  red: { label: "Neon Pink", rgb: "244 114 182", soft: "94 221 211" },
  green: { label: "Mint", rgb: "110 226 178", soft: "244 114 182" },
  amber: { label: "Silver", rgb: "185 198 210", soft: "244 114 182" },
};

/// Root size for the rem-based type scale; all text scales with it.
export const FONT_SIZES: Record<FontSizeName, { label: string; px: number }> = {
  compact: { label: "Compact", px: 14 },
  default: { label: "Default", px: 16 },
  large: { label: "Large", px: 18 },
  xlarge: { label: "Extra large", px: 20 },
};

const THEME_KEY = "werk.theme";
const ACCENT_KEY = "werk.accent";
const CORNERS_KEY = "werk.corners";
const FONT_KEY = "werk.font";
const SNIPPETS_KEY = "werk.tool_snippets";
const ALIGN_KEY = "werk.chat.align";
const RUN_CHANGES_KEY = "werk.chat.runChanges";

let theme: ThemeName = "system";
let accent: AccentName = "blue";
let corners: CornersMode = "auto";
let font: FontSizeName = "default";
let showToolSnippets = true;
const snippetListeners = new Set<(v: boolean) => void>();
/// Where the user's bubbles sit; the model takes the other side.
let bubbleAlign: BubbleAlign = "right";
const alignListeners = new Set<(v: BubbleAlign) => void>();
/// Changed-files summary card after a run.
let showRunChanges = true;
const runChangesListeners = new Set<(v: boolean) => void>();
let autoCorners: "sharp" | "round" = "sharp";
const listeners = new Set<() => void>();

function read<T extends string>(key: string, fallback: T, valid: readonly string[]): T {
  try {
    const raw = localStorage.getItem(key);
    if (raw && (valid as readonly string[]).includes(raw)) return raw as T;
  } catch {}
  return fallback;
}

function emit() {
  listeners.forEach((l) => l());
}

export function getTheme(): ThemeName {
  return theme;
}

export function getAccent(): AccentName {
  return accent;
}

export function getCorners(): CornersMode {
  return corners;
}

export function getFont(): FontSizeName {
  return font;
}

export function setTheme(v: ThemeName): void {
  if (v === theme) return;
  theme = v;
  try {
    localStorage.setItem(THEME_KEY, v);
  } catch {}
  applyAppearance();
  emit();
}

export function setAccent(v: AccentName): void {
  if (v === accent) return;
  accent = v;
  try {
    localStorage.setItem(ACCENT_KEY, v);
  } catch {}
  applyAppearance();
  emit();
}

export function setCorners(v: CornersMode): void {
  if (v === corners) return;
  corners = v;
  try {
    localStorage.setItem(CORNERS_KEY, v);
  } catch {}
  applyAppearance();
  emit();
}

export function setFont(v: FontSizeName): void {
  if (v === font) return;
  font = v;
  try {
    localStorage.setItem(FONT_KEY, v);
  } catch {}
  applyAppearance();
  emit();
}

export function subscribeAppearance(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function getShowToolSnippets(): boolean {
  return showToolSnippets;
}

export function setShowToolSnippets(v: boolean): void {
  if (v === showToolSnippets) return;
  showToolSnippets = v;
  try {
    localStorage.setItem(SNIPPETS_KEY, v ? "1" : "0");
  } catch {}
  emit();
  snippetListeners.forEach((l) => l(v));
}

export function subscribeShowToolSnippets(cb: (v: boolean) => void): () => void {
  snippetListeners.add(cb);
  return () => {
    snippetListeners.delete(cb);
  };
}

export function getBubbleAlign(): BubbleAlign {
  return bubbleAlign;
}

export function setBubbleAlign(v: BubbleAlign): void {
  if (v === bubbleAlign) return;
  bubbleAlign = v;
  try {
    localStorage.setItem(ALIGN_KEY, v);
  } catch {}
  emit();
  alignListeners.forEach((l) => l(v));
}

export function subscribeBubbleAlign(cb: (v: BubbleAlign) => void): () => void {
  alignListeners.add(cb);
  return () => {
    alignListeners.delete(cb);
  };
}

export function getShowRunChanges(): boolean {
  return showRunChanges;
}

export function setShowRunChanges(v: boolean): void {
  if (v === showRunChanges) return;
  showRunChanges = v;
  try {
    localStorage.setItem(RUN_CHANGES_KEY, v ? "1" : "0");
  } catch {}
  emit();
  runChangesListeners.forEach((l) => l(v));
}

export function subscribeShowRunChanges(cb: (v: boolean) => void): () => void {
  runChangesListeners.add(cb);
  return () => {
    runChangesListeners.delete(cb);
  };
}

function resolvedTheme(): string {
  if (theme === "system") {
    try {
      return window.matchMedia("(prefers-color-scheme: light)").matches ? "day" : "night";
    } catch {
      return "night";
    }
  }
  return theme;
}

/// Accent swatches for the currently painted theme.
export function accentPalette(): Record<AccentName, { label: string; rgb: string }> {
  const resolved = resolvedTheme();
  if (resolved === "gemma") return GEMMA_ACCENTS;
  if (resolved === "migu") return MIGU_ACCENTS;
  if (resolved === "werk") return EARTHY_ACCENTS;
  return ACCENTS;
}

function applyAppearance(): void {
  const root = document.documentElement;
  const resolved = resolvedTheme();
  root.setAttribute("data-theme", resolved);
  // Themes with a signature palette own their accent variants.
  if (resolved === "gemma") {
    root.style.setProperty("--accent", GEMMA_ACCENTS[accent].rgb);
    root.style.setProperty("--accent-soft", GEMMA_ACCENTS[accent].soft);
  } else if (resolved === "migu") {
    root.style.setProperty("--accent", MIGU_ACCENTS[accent].rgb);
    root.style.setProperty("--accent-soft", MIGU_ACCENTS[accent].soft);
  } else if (resolved === "werk") {
    root.style.setProperty("--accent", EARTHY_ACCENTS[accent].rgb);
    root.style.setProperty("--accent-soft", EARTHY_ACCENTS[accent].soft);
  } else {
    root.style.setProperty("--accent", ACCENTS[accent].rgb);
    root.style.removeProperty("--accent-soft");
  }
  root.style.fontSize = `${FONT_SIZES[font].px}px`;
  const sharp = corners === "sharp" || (corners === "auto" && autoCorners === "sharp");
  root.setAttribute("data-corners", sharp ? "sharp" : "round");
}

export function loadAppearance(autoDetectedCorners?: "sharp" | "round"): void {
  theme = read(THEME_KEY, "system", ["system", "day", "night", "werk", "gemma", "migu", "nerv"]);
  accent = read(ACCENT_KEY, "blue", Object.keys(ACCENTS));
  corners = read(CORNERS_KEY, "auto", ["auto", "sharp", "round"]);
  font = read(FONT_KEY, "default", Object.keys(FONT_SIZES));
  bubbleAlign = read(ALIGN_KEY, "right", ["left", "right"]);
  try {
    showToolSnippets = localStorage.getItem(SNIPPETS_KEY) !== "0";
  } catch {}
  try {
    showRunChanges = localStorage.getItem(RUN_CHANGES_KEY) !== "0";
  } catch {}
  if (autoDetectedCorners) autoCorners = autoDetectedCorners;
  applyAppearance();
}

export function setAutoCorners(v: "sharp" | "round"): void {
  if (v === autoCorners) return;
  autoCorners = v;
  applyAppearance();
  emit();
}

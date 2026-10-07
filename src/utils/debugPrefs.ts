//! Debug-only preferences (localStorage): the webview devtools toggle and the
//! theme-animation override.

const DEVTOOLS_KEY = "werk.debug.devtools";
const THEME_ANIMATIONS_KEY = "werk.debug.themeAnimations";

export function getDevtools(): boolean {
  try {
    return localStorage.getItem(DEVTOOLS_KEY) === "1";
  } catch {
    return false;
  }
}

export function setDevtools(on: boolean): void {
  try {
    localStorage.setItem(DEVTOOLS_KEY, on ? "1" : "0");
  } catch {}
}

/// Force theme animations even when software rendering turns them off.
export function getThemeAnimations(): boolean {
  try {
    return localStorage.getItem(THEME_ANIMATIONS_KEY) === "1";
  } catch {
    return false;
  }
}

const listeners = new Set<() => void>();

export function setThemeAnimations(on: boolean): void {
  try {
    localStorage.setItem(THEME_ANIMATIONS_KEY, on ? "1" : "0");
  } catch {}
  for (const fn of listeners) fn();
}

export function subscribeThemeAnimations(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

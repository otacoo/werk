//! Debug-only preferences (localStorage): the webview devtools toggle.

const DEVTOOLS_KEY = "werk.debug.devtools";

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

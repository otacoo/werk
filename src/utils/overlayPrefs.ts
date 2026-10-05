//! Overlay-only visual preferences, stored with the appearance settings so
//! the overlay window re-reads them live over `appearance_changed`.

import { emit as emitTauri } from "@tauri-apps/api/event";

const OPACITY_KEY = "werk.overlay.opacity";
const ANIMATIONS_KEY = "werk.overlay.animations";
/// Never let the slider make the pill effectively invisible.
export const OVERLAY_OPACITY_MIN = 20;

export function getOverlayOpacity(): number {
  try {
    const v = Number(localStorage.getItem(OPACITY_KEY));
    if (Number.isFinite(v) && v >= OVERLAY_OPACITY_MIN && v <= 100) return v;
  } catch {}
  return 100;
}

export function setOverlayOpacity(value: number): void {
  const clamped = Math.min(100, Math.max(OVERLAY_OPACITY_MIN, Math.round(value)));
  try {
    localStorage.setItem(OPACITY_KEY, String(clamped));
  } catch {}
  // Other windows re-read on the signal; the overlay must not rebroadcast.
  if (!window.location.search.includes("overlay")) {
    emitTauri("appearance_changed").catch(() => {});
  }
}

/// Decorative overlay motion (sleeping z's, the sweat drop).
export function getOverlayAnimations(): boolean {
  try {
    return localStorage.getItem(ANIMATIONS_KEY) !== "0";
  } catch {}
  return true;
}

export function setOverlayAnimations(on: boolean): void {
  try {
    localStorage.setItem(ANIMATIONS_KEY, on ? "1" : "0");
  } catch {}
  if (!window.location.search.includes("overlay")) {
    emitTauri("appearance_changed").catch(() => {});
  }
}

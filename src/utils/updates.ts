//! Update check shared between the Settings card and the gear badge: one
//! in-flight request, one cached result, subscribers for both.
import { check, type Update } from "@tauri-apps/plugin-updater";

let pending: Update | null = null;
let checked = false;
let inFlight: Promise<Update | null> | null = null;
const listeners = new Set<(update: Update | null, checked: boolean) => void>();

export function subscribeUpdate(
  fn: (update: Update | null, checked: boolean) => void,
): () => void {
  listeners.add(fn);
  fn(pending, checked);
  return () => {
    listeners.delete(fn);
  };
}

/// Check once at a time; concurrent callers share the request. Errors
/// propagate to the caller and leave the cached result untouched.
export async function checkForUpdate(): Promise<Update | null> {
  if (inFlight) return inFlight;
  inFlight = check()
    .then((update) => {
      pending = update;
      checked = true;
      for (const fn of listeners) fn(pending, checked);
      return update;
    })
    .finally(() => {
      inFlight = null;
    });
  return inFlight;
}

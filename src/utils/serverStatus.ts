//! One shared server-status poller: pages subscribe instead of each polling.
import { commands } from "../bindings";
import type { ServerStatus } from "../bindings";
import { call } from "./ipc";

let status: ServerStatus = { type: "stopped" };
const listeners = new Set<(s: ServerStatus) => void>();
let started = false;

export function getServerStatus(): ServerStatus {
  return status;
}

export function subscribeServerStatus(cb: (s: ServerStatus) => void): () => void {
  listeners.add(cb);
  cb(status);
  return () => {
    listeners.delete(cb);
  };
}

/// Statuses are small and stable; notifying on every poll re-renders every
/// mounted page (including off-tab ones) for nothing.
export function sameStatus(a: ServerStatus, b: ServerStatus): boolean {
  if (a.type !== b.type) return false;
  if (a.type === "running" && b.type === "running") {
    return a.port === b.port && a.pid === b.pid && a.ready === b.ready;
  }
  if (a.type === "error" && b.type === "error") {
    return a.message === b.message;
  }
  return true;
}

export function startServerStatusPolling(): void {
  if (started) return;
  started = true;
  const poll = () => {
    // A hidden window (tray, minimized, covered) needs no updates.
    if (document.visibilityState === "hidden") return;
    call(commands.getServerStatus())
      .then((s) => {
        if (sameStatus(status, s)) return;
        status = s;
        listeners.forEach((l) => l(s));
      })
      .catch(() => {});
  };
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") poll();
  });
  poll();
  setInterval(poll, 2000);
}

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

export function startServerStatusPolling(): void {
  if (started) return;
  started = true;
  const poll = () =>
    call(commands.getServerStatus())
      .then((s) => {
        status = s;
        listeners.forEach((l) => l(s));
      })
      .catch(() => {});
  poll();
  setInterval(poll, 2000);
}

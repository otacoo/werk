import finishedUrl from "../../assets/sounds/finished.mp3?url";
import askUrl from "../../assets/sounds/ask.mp3?url";
import errorUrl from "../../assets/sounds/error.mp3?url";
import { commands } from "../bindings";
import { call } from "./ipc";

export type NotificationKind = "agent" | "permissions" | "errors";

const FILES: Record<NotificationKind, string> = {
  agent: finishedUrl,
  permissions: askUrl,
  errors: errorUrl,
};

// Nudge the autoplay policy on first interaction so later programmatic
// plays (agent finish, approvals) are allowed. Failures stay silent.
let unlocked = false;
function unlock() {
  if (unlocked) return;
  unlocked = true;
  try {
    const a = new Audio(FILES.agent);
    a.muted = true;
    void a.play().catch(() => {});
  } catch {}
}
if (typeof window !== "undefined") {
  window.addEventListener("pointerdown", unlock, { once: true });
  window.addEventListener("keydown", unlock, { once: true });
}

/** Play a notification sound if its Settings toggle is on. Never throws. */
export async function playNotificationSound(kind: NotificationKind): Promise<void> {
  try {
    const cfg = await call(commands.getConfig());
    const enabled =
      kind === "agent"
        ? (cfg.sound_agent ?? true)
        : kind === "permissions"
          ? (cfg.sound_permissions ?? true)
          : (cfg.sound_errors ?? true);
    if (!enabled) return;
    const audio = new Audio(FILES[kind]);
    audio.volume = 0.8;
    await audio.play();
  } catch {}
}

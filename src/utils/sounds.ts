import finishedUrl from "../../assets/sounds/finished.wav?url";
import askUrl from "../../assets/sounds/ask.wav?url";
import errorUrl from "../../assets/sounds/error.wav?url";
import { commands } from "../bindings";
import { call } from "./ipc";
import { subscribeConfigChanged } from "./appSettings";

export type NotificationKind = "agent" | "permissions" | "errors";

// PCM WAV: decodable everywhere the webview can play audio, with no codec
// plugins required (WebKitGTK goes through GStreamer).
const FILES: Record<NotificationKind, string> = {
  agent: finishedUrl,
  permissions: askUrl,
  errors: errorUrl,
};

// Cached sound toggles: playback must not wait on an IPC round trip, or
// WebKit drops the user activation and refuses to play.
let prefs: Record<NotificationKind, boolean> | null = null;

async function soundPrefs(): Promise<Record<NotificationKind, boolean>> {
  if (prefs) return prefs;
  const cfg = await call(commands.getConfig());
  prefs = {
    agent: cfg.sound_agent ?? true,
    permissions: cfg.sound_permissions ?? true,
    errors: cfg.sound_errors ?? true,
  };
  return prefs;
}

// Settings toggles invalidate the cache.
subscribeConfigChanged(() => {
  prefs = null;
});

// One reusable element per sound: a muted play during the first user gesture
// unlocks the element for later programmatic playback (WebKitGTK/WKWebView).
const elements: Partial<Record<NotificationKind, HTMLAudioElement>> = {};

function elementFor(kind: NotificationKind): HTMLAudioElement {
  let el = elements[kind];
  if (!el) {
    el = new Audio(FILES[kind]);
    el.volume = 0.8;
    elements[kind] = el;
  }
  return el;
}

let unlocked = false;
function unlock() {
  if (unlocked) return;
  unlocked = true;
  void soundPrefs().catch(() => {});
  for (const kind of ["agent", "permissions", "errors"] as NotificationKind[]) {
    try {
      const el = elementFor(kind);
      el.muted = true;
      void el
        .play()
        .then(() => {
          el.pause();
          el.currentTime = 0;
          el.muted = false;
        })
        .catch(() => {
          el.muted = false;
        });
    } catch {}
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("pointerdown", unlock, { once: true });
  window.addEventListener("keydown", unlock, { once: true });
}

/** Play a notification sound if its Settings toggle is on. Never throws. */
export async function playNotificationSound(kind: NotificationKind): Promise<void> {
  try {
    const enabled = (await soundPrefs())[kind];
    if (!enabled) return;
    const el = elementFor(kind);
    el.currentTime = 0;
    await el.play();
  } catch {}
}

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

// Play from in-memory blob URLs: WebKitGTK's media player requests byte
// ranges, which the app's asset protocol does not serve, so pointing an
// audio element at the asset URL fails with NotSupportedError on Linux.
const sources: Partial<Record<NotificationKind, string>> = {};

async function sourceFor(kind: NotificationKind): Promise<string> {
  const existing = sources[kind];
  if (existing) return existing;
  const res = await fetch(FILES[kind]);
  if (!res.ok) throw new Error(`Cannot load the ${kind} sound (${res.status})`);
  const url = URL.createObjectURL(await res.blob());
  sources[kind] = url;
  return url;
}

// One reusable element per sound: a muted play during the first user gesture
// unlocks the element for later programmatic playback (WebKitGTK/WKWebView).
const elements: Partial<Record<NotificationKind, HTMLAudioElement>> = {};

async function elementFor(kind: NotificationKind): Promise<HTMLAudioElement> {
  const src = await sourceFor(kind);
  let el = elements[kind];
  if (!el) {
    el = new Audio();
    el.volume = 0.8;
    elements[kind] = el;
  }
  if (el.src !== src) el.src = src;
  return el;
}

let unlocked = false;
function unlock() {
  if (unlocked) return;
  unlocked = true;
  void soundPrefs().catch(() => {});
  void (async () => {
    for (const kind of ["agent", "permissions", "errors"] as NotificationKind[]) {
      try {
        const el = await elementFor(kind);
        el.muted = true;
        await el.play();
        el.pause();
        el.currentTime = 0;
        el.muted = false;
      } catch {
        const el = elements[kind];
        if (el) el.muted = false;
      }
    }
  })();
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
    const el = await elementFor(kind);
    el.currentTime = 0;
    await el.play();
  } catch {}
}

/// Play a sound regardless of the toggle and surface failures, for the
/// Debug page's test buttons (Linux audio issues are otherwise invisible).
export async function testNotificationSound(kind: NotificationKind): Promise<void> {
  const el = await elementFor(kind);
  el.muted = false;
  el.volume = 0.8;
  el.currentTime = 0;
  await el.play();
}

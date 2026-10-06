import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { ArrowUp, Droplets, Mic, RefreshCw, Sparkles, Square } from "lucide-react";
import { commands } from "../bindings";
import { call } from "../utils/ipc";
import { loadAppearance } from "../utils/appearance";
import { getOverlayAnimations, getOverlayOpacity } from "../utils/overlayPrefs";
import { VoiceRecorder } from "../utils/recorder";
import { SpeechQueue } from "../utils/speechQueue";

const COLLAPSED = 84;
const EXPANDED_W = 404;
const EXPANDED_H = 84;
/// Wider pill while a run is starting/working, for the status label.
const WORKING_W = 176;
const POS_KEY = "werk.overlay.pos";

/// Always-on-top assistant overlay: a pulsing circle that expands into a
/// floating input. Enter sends to the assistant; Esc collapses.
export default function Overlay() {
  const [expanded, setExpanded] = useState(false);
  const [input, setInput] = useState("");
  const [state, setState] = useState<"idle" | "working" | "sent" | "attention" | "error">("idle");
  const [avatar, setAvatar] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /// Generation events are flowing: the only thing that lights the glow.
  const [generating, setGenerating] = useState(false);
  /// Quiet for a while: the avatar dozes off with floating z's.
  const [sleeping, setSleeping] = useState(false);
  /// User-chosen pill opacity (appearance preference).
  const [opacity, setOpacity] = useState(getOverlayOpacity);
  /// Decorative motion: sleeping z's and the sweat drop.
  const [animations, setAnimations] = useState(getOverlayAnimations);
  /// Wayland forbids client-side positioning: bounds and position memory off.
  const waylandRef = useRef(false);
  const stateRef = useRef(state);
  stateRef.current = state;
  const inputRef = useRef<HTMLInputElement>(null);
  const expandedRef = useRef(false);
  const sentTimer = useRef<number | null>(null);
  const sleepTimer = useRef<number | null>(null);
  /// Click opens; a press that moves past the threshold drags the window.
  const dragging = useRef(false);
  const pressAt = useRef<{ x: number; y: number } | null>(null);
  /// Expanded state at press time: the input blurs before the click lands.
  const wasExpanded = useRef(false);
  /// A press on the pill body: keep the input's blur from collapsing it.
  const pillPress = useRef(false);
  /// A send is in flight: key repeat must not fire it twice.
  const sending = useRef(false);
  /// The growing reply text and its narration queue (sentences synthesize
  /// while the model is still writing).
  const reply = useRef("");
  const speech = useRef(new SpeechQueue());
  const narrate = useRef(false);
  /// Voice input: dictation through the configured Qwen3-ASR model.
  const [sttEnabled, setSttEnabled] = useState(false);
  const [micReady, setMicReady] = useState(false);
  /// Assistant name for the input suggestion ("Ask Ada…").
  const [assistantName, setAssistantName] = useState("");
  /// The server is loading the model for this run ("Waking up…").
  const [loadingModel, setLoadingModel] = useState(false);
  /// Narration audio is still playing; keep the glow going.
  const [speaking, setSpeaking] = useState(false);
  const speakingRef = useRef(false);
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [level, setLevel] = useState(0);
  const [sweat, setSweat] = useState(false);
  const recorder = useRef(new VoiceRecorder());
  const sweatTimer = useRef<number | null>(null);

  const beginPress = (e: ReactPointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    // A press anywhere on a handle must keep the input's blur from collapsing
    // the pill while the user is dragging (avatar or pill body).
    pillPress.current = true;
    wake();
    dragging.current = false;
    wasExpanded.current = expandedRef.current;
    pressAt.current = { x: e.screenX, y: e.screenY };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPressDown = (e: ReactPointerEvent<HTMLButtonElement>) => {
    beginPress(e);
  };

  /// The pill body drags too; controls keep their own behavior.
  const onPillDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).closest("input, button, textarea, select, a")) return;
    beginPress(e);
  };

  const onPressMove = async (e: ReactPointerEvent<HTMLElement>) => {
    const from = pressAt.current;
    if (!from || dragging.current) return;
    if (Math.hypot(e.screenX - from.x, e.screenY - from.y) < 4) return;
    dragging.current = true;
    pressAt.current = null;
    try {
      await getCurrentWindow().startDragging();
    } catch {
      // Best effort; a plain click still opens the input.
    }
  };

  const onPressUp = (e: ReactPointerEvent<HTMLElement>) => {
    pillPress.current = false;
    pressAt.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // Capture may already be gone after an OS drag.
    }
  };

  const onPressClick = () => {
    if (dragging.current) {
      dragging.current = false;
      return;
    }
    if (wasExpanded.current) void collapse();
    else void expand();
  };

  /// Clicking the pill body (never a control) collapses an empty pill.
  const onPillClick = (e: ReactMouseEvent<HTMLDivElement>) => {
    pillPress.current = false;
    if ((e.target as HTMLElement).closest("input, button, textarea, select, a")) return;
    if (dragging.current) {
      dragging.current = false;
      return;
    }
    if (expandedRef.current && !input.trim()) void collapse();
  };

  const wake = () => {
    if (sleepTimer.current) window.clearTimeout(sleepTimer.current);
    sleepTimer.current = null;
    setSleeping(false);
    // Re-arm: a drag or nudge restarts the countdown instead of ending it.
    if (stateRef.current === "idle" && !expandedRef.current) {
      sleepTimer.current = window.setTimeout(() => setSleeping(true), 30000);
    }
  };

  // Doze off after a quiet spell; any activity restarts the countdown.
  useEffect(() => {
    wake();
    return () => {
      if (sleepTimer.current) window.clearTimeout(sleepTimer.current);
      sleepTimer.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, expanded, input]);

  // Transparent page: the pill owns all visible pixels. The theme is applied
  // here too: the overlay renders outside App, which normally loads it.
  useEffect(() => {
    loadAppearance();
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    document.body.style.overflow = "hidden";
  }, []);

  // Theme/accent/font changes in the main window re-apply here live; the
  // overlay opacity is an appearance preference too.
  useEffect(() => {
    const unlisten = listen("appearance_changed", () => {
      loadAppearance();
      setOpacity(getOverlayOpacity());
      setAnimations(getOverlayAnimations());
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const anchor = async (w: number, h: number, initial = false) => {
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    if (initial) {
      const pw = Math.round(w * scale);
      const ph = Math.round(h * scale);
      const margin = Math.round(16 * scale);
      const raise = Math.round(16 * scale);
      // Prefer the work area so the pill never sits over the taskbar.
      const area = await commands.overlayWorkArea().catch(() => null);
      // A dragged position is remembered; clamp it into the work area.
      const saved = localStorage.getItem(POS_KEY);
      if (saved) {
        try {
          const pos = JSON.parse(saved) as { x: number; y: number };
          const x = area
            ? Math.min(Math.max(pos.x, area.left), area.right - pw)
            : pos.x;
          const y = area
            ? Math.min(Math.max(pos.y, area.top), area.bottom - ph)
            : pos.y;
          await win.setSize(new PhysicalSize(pw, ph));
          await win.setPosition(new PhysicalPosition(x, y));
          await win.setAlwaysOnTop(true);
          return;
        } catch {
          // Corrupt entry: fall through to the default corner.
        }
      }
      if (area) {
        await win.setSize(new PhysicalSize(pw, ph));
        await win.setPosition(
          new PhysicalPosition(area.right - pw - margin, area.bottom - ph - margin - raise),
        );
        await win.setAlwaysOnTop(true);
        return;
      }
      const mon = await currentMonitor();
      if (mon) {
        await win.setSize(new PhysicalSize(pw, ph));
        await win.setPosition(
          new PhysicalPosition(
            mon.position.x + mon.size.width - pw - margin,
            mon.position.y + mon.size.height - ph - margin - raise,
          ),
        );
        await win.setAlwaysOnTop(true);
        return;
      }
    }
    // Keep the bottom-right corner anchored across resizes (and drags).
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    const pw = Math.round(w * scale);
    const ph = Math.round(h * scale);
    await win.setSize(new PhysicalSize(pw, ph));
    await win.setPosition(new PhysicalPosition(pos.x + size.width - pw, pos.y + size.height - ph));
    await win.setAlwaysOnTop(true);
  };

  useEffect(() => {
    void anchor(COLLAPSED, COLLAPSED, true).then(() => enforceBounds());
    call(commands.assistantAvatar())
      .then(setAvatar)
      .catch(() => {});
    call(commands.sessionKind())
      .then((k) => {
        waylandRef.current = k === "wayland";
      })
      .catch(() => {});
    refreshMic();
    // Narration drives the glow while audio plays, and reports failures.
    speech.current.onPlayback = (playing) => {
      speakingRef.current = playing;
      setSpeaking(playing);
    };
    speech.current.onError = (message) => {
      setError(`Narration failed: ${message}`);
      setState("error");
      if (sentTimer.current) window.clearTimeout(sentTimer.current);
      sentTimer.current = window.setTimeout(() => setState("idle"), 4000);
    };
  }, []);

  /// More than half off-screen: sweat, then bounce fully back in. Wayland
  /// compositors place windows themselves, so there is nothing to enforce.
  const enforceBounds = async () => {
    if (waylandRef.current) return;
    const win = getCurrentWindow();
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    let bounds: { left: number; top: number; right: number; bottom: number } | null = null;
    const area = await commands.overlayWorkArea().catch(() => null);
    if (area) {
      bounds = { left: area.left, top: area.top, right: area.right, bottom: area.bottom };
    } else {
      const mon = await currentMonitor();
      if (mon) {
        bounds = {
          left: mon.position.x,
          top: mon.position.y,
          right: mon.position.x + mon.size.width,
          bottom: mon.position.y + mon.size.height,
        };
      }
    }
    if (!bounds) return;
    const visibleW = Math.min(pos.x + size.width, bounds.right) - Math.max(pos.x, bounds.left);
    const visibleH = Math.min(pos.y + size.height, bounds.bottom) - Math.max(pos.y, bounds.top);
    const visible = Math.max(0, visibleW) * Math.max(0, visibleH);
    if (visible * 2 >= size.width * size.height) return;
    const margin = Math.round(12 * (await win.scaleFactor()));
    const target = {
      x: Math.min(Math.max(pos.x, bounds.left + margin), bounds.right - size.width - margin),
      y: Math.min(Math.max(pos.y, bounds.top + margin), bounds.bottom - size.height - margin),
    };
    setSweat(true);
    // Wayland ignores client positioning, so the bounce is X11/Windows-only;
    // the sweat still flags that the pill is mostly off-screen.
    if (!waylandRef.current) {
      const steps = 8;
      for (let i = 1; i <= steps; i++) {
        const t = 1 - Math.pow(1 - i / steps, 3);
        await win.setPosition(
          new PhysicalPosition(
            Math.round(pos.x + (target.x - pos.x) * t),
            Math.round(pos.y + (target.y - pos.y) * t),
          ),
        );
        await new Promise((r) => window.setTimeout(r, 16));
      }
    }
    if (sweatTimer.current) window.clearTimeout(sweatTimer.current);
    sweatTimer.current = window.setTimeout(() => setSweat(false), 900);
  };

  /// Voice input is ready when the Voice tab has an STT model enabled.
  const refreshMic = () => {
    call(commands.getConfig())
      .then((c) => {
        const enabled = !!c.assistant?.stt_enabled;
        setSttEnabled(enabled);
        setMicReady(enabled && !!c.assistant?.stt_model);
        setAssistantName((c.assistant?.name ?? "").trim());
      })
      .catch(() => {});
  };

  const toggleDictation = async () => {
    if (transcribing) return;
    setError(null);
    if (recording) {
      setRecording(false);
      setTranscribing(true);
      try {
        const audio = await recorder.current.stop();
        if (audio) {
          const res = await call(commands.assistantSttTranscribe(audio));
          if (res.text) {
            setInput((prev) => (prev ? `${prev} ${res.text}` : res.text));
            inputRef.current?.focus();
          }
        }
      } catch (e) {
        setError(String(e));
      } finally {
        setTranscribing(false);
      }
    } else {
      try {
        await recorder.current.start();
        setRecording(true);
      } catch (e) {
        setError(`Microphone unavailable: ${e}`);
      }
    }
  };

  // Remember where the pill was dragged to (debounced; moves are frequent).
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: number | null = null;
    const unlisten = win.onMoved(({ payload }) => {
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        dragging.current = false;
        // Wayland ignores setPosition, so a saved spot could never be restored.
        if (!waylandRef.current) {
          localStorage.setItem(POS_KEY, JSON.stringify({ x: payload.x, y: payload.y }));
        }
        void enforceBounds();
      }, 250);
    });
    return () => {
      unlisten.then((f) => f());
      if (timer) window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const expand = async () => {
    if (expandedRef.current) {
      inputRef.current?.focus();
      return;
    }
    refreshMic();
    expandedRef.current = true;
    setExpanded(true);
    await anchor(EXPANDED_W, EXPANDED_H);
    const win = getCurrentWindow();
    await win.setFocus();
    inputRef.current?.focus();
  };

  const collapse = async () => {
    if (!expandedRef.current) return;
    expandedRef.current = false;
    setExpanded(false);
    setInput("");
    setError(null);
    // A recording without a pill has nowhere to land: stop and drop it.
    if (recorder.current.active) void recorder.current.stop();
    setRecording(false);
    await anchor(
      stateRef.current === "working" || speakingRef.current ? WORKING_W : COLLAPSED,
      COLLAPSED,
    );
  };

  useEffect(() => {
    expandedRef.current = expanded;
  }, [expanded]);

  // The assistant bus drives the pulse and quick status.
  useEffect(() => {
    const unlisten = listen<Record<string, unknown>>("assistant_event", (event) => {
      const ev = event.payload;
      switch (ev.type) {
        case "content": {
          reply.current += (ev.text as string) ?? "";
          setState("working");
          setGenerating(true);
          setLoadingModel(false);
          if (sending.current && narrate.current) speech.current.push(reply.current);
          break;
        }
        case "reasoning_delta":
        case "tool_call":
        case "tool_result":
          setState("working");
          setGenerating(true);
          if (ev.type === "tool_call") speech.current.abort();
          break;
        case "done":
          setGenerating(false);
          break;
        case "notice":
          if (typeof ev.text === "string" && /failed/i.test(ev.text)) {
            setError(ev.text);
            setState("error");
          }
          break;
        case "reminder":
          setState("attention");
          if (sentTimer.current) window.clearTimeout(sentTimer.current);
          sentTimer.current = window.setTimeout(() => setState("idle"), 6000);
          break;
        default:
          break;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Global hotkey (or tray): expand and focus.
  useEffect(() => {
    const unlisten = listen("assistant_focus", () => {
      void expand();
      refreshMic();
      call(commands.assistantAvatar())
        .then(setAvatar)
        .catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The server reports model loads on server_log; during a starting run that
  // is the "waking up" phase.
  useEffect(() => {
    const unlisten = listen<string>("server_log", (e) => {
      if (/loading model|ensure_model|waiting until model/i.test(e.payload ?? "")) {
        setLoadingModel(true);
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Grow the pill while working or speaking so the status label has room.
  useEffect(() => {
    if (expandedRef.current) return;
    void anchor(state === "working" || speaking ? WORKING_W : COLLAPSED, COLLAPSED);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, speaking]);

  // Dictation hotkey: open the pill and start or stop recording.
  useEffect(() => {
    const unlisten = listen("assistant_dictate", () => {
      void (async () => {
        if (!expandedRef.current) await expand();
        await toggleDictation();
      })();
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Live input level while recording, so the pill visibly listens. After
  // speech stops, a few quiet seconds end the take and transcribe it.
  useEffect(() => {
    if (!recording) {
      setLevel(0);
      return;
    }
    let raf = 0;
    let last = 0;
    let lastVoice = performance.now();
    let hadVoice = false;
    const tick = (t: number) => {
      const lv = recorder.current.level();
      if (t - last > 60) {
        last = t;
        setLevel(lv);
      }
      if (lv > 0.08) {
        hadVoice = true;
        lastVoice = t;
      }
      if (hadVoice && t - lastVoice > 3500) {
        void toggleDictation();
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording]);

  const send = async () => {
    const text = input.trim();
    if (!text || sending.current) return;
    if (recording) {
      void recorder.current.stop();
      setRecording(false);
    }
    sending.current = true;
    setError(null);
    setState("working");
    setGenerating(false);
    setLoadingModel(false);
    setInput("");
    reply.current = "";
    speech.current.reset();
    try {
      // The local server may be stopped; start it like the Chat composer does.
      const cfg = call(commands.getConfig()).catch(() => null);
      await call(commands.ensureServer());
      const config = await cfg;
      narrate.current = !!(config?.assistant?.tts_enabled && config?.assistant?.tts_autoplay);
      const res = await call(commands.assistantSend(text, null, null));
      setState("sent");
      if (sentTimer.current) window.clearTimeout(sentTimer.current);
      sentTimer.current = window.setTimeout(() => setState("idle"), 2500);
      // Narrate when voice narration is on; failures stay silent.
      if (narrate.current && res.text) void speech.current.finish(res.text);
    } catch (e) {
      const msg = String(e);
      setGenerating(false);
      speech.current.abort();
      if (!msg.includes("aborted")) {
        setError(msg);
        setState("error");
        if (sentTimer.current) window.clearTimeout(sentTimer.current);
        sentTimer.current = window.setTimeout(() => setState("idle"), 4000);
      } else {
        setState("idle");
      }
    } finally {
      sending.current = false;
    }
  };

  const dot =
    state === "working"
      ? "bg-accent animate-pulse"
      : state === "sent"
        ? "bg-accent-green"
        : state === "attention"
          ? "bg-accent-yellow animate-pulse"
          : state === "error"
            ? "bg-accent-red"
            : "bg-accent/80 animate-[pulse_3s_ease-in-out_infinite]";
  /// Busy covers the whole span: server start, model load, generation, speech.
  const busy = state === "working" || speaking;
  const statusLabel = generating
    ? "Thinking…"
    : speaking
      ? "Speaking…"
      : loadingModel
        ? "Loading model…"
        : "Waking up…";

  return (
    <div
      className="h-screen w-screen flex items-end justify-end p-2 select-none"
      style={opacity < 100 ? { opacity: opacity / 100 } : undefined}
    >
      <div
        className={`relative flex items-center gap-2 rounded-full border border-border bg-surface-2/95 shadow-lg transition-all ${
          expanded ? "pl-2 pr-1.5 py-1.5" : "p-1.5"
        }`}
        onPointerDown={onPillDown}
        onPointerMove={(e) => void onPressMove(e)}
        onPointerUp={onPressUp}
        onPointerCancel={onPressUp}
        onClick={onPillClick}
      >
        {expanded && busy && (
          <span className="overlay-ring" aria-hidden>
            <span className="overlay-ring-rotor" />
          </span>
        )}
        <button
          className="relative w-11 h-11 rounded-full bg-surface-3 flex items-center justify-center shrink-0 overflow-hidden cursor-pointer"
          onPointerDown={onPressDown}
          onPointerMove={(e) => void onPressMove(e)}
          onPointerUp={onPressUp}
          onPointerCancel={onPressUp}
          onClick={onPressClick}
          title={expanded ? "Collapse" : `Ask ${assistantName || "the assistant"} (drag to move)`}
        >
          <span className={`absolute inset-0 rounded-full opacity-30 ${dot}`} />
          {avatar ? (
            <img
              src={avatar}
              alt=""
              draggable={false}
              onDragStart={(e) => e.preventDefault()}
              className="relative w-full h-full object-cover"
            />
          ) : (
            <Sparkles size={16} className="relative text-ink" />
          )}
          {!expanded && busy && (
            <span className="overlay-ring" aria-hidden>
              <span className="overlay-ring-rotor" />
            </span>
          )}
        </button>
        {!expanded && busy && (
          <span className="text-xs text-dim whitespace-nowrap pr-2" aria-live="polite">
            {statusLabel}
          </span>
        )}
        {sleeping && !expanded && animations && (
          <span className="overlay-zzz" aria-hidden>
            <span>z</span>
            <span>z</span>
            <span>z</span>
          </span>
        )}
        {sweat && animations && (
          <span className="overlay-sweat" aria-hidden>
            <Droplets size={12} fill="currentColor" />
          </span>
        )}
        {expanded && (
          <>
            <input
              ref={inputRef}
              className="input flex-1 min-w-0 bg-transparent border-0 text-sm focus:outline-none"
              placeholder={
                recording
                  ? "Listening…"
                  : busy
                    ? statusLabel
                    : assistantName
                      ? `Ask ${assistantName}…`
                      : "Ask the assistant…"
              }
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void send();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  void collapse();
                }
              }}
              onBlur={() => {
                if (!pillPress.current && !input.trim()) void collapse();
              }}
            />
            {recording && (
              <span className="flex items-end gap-0.5 h-4 shrink-0" aria-hidden>
                {[0.45, 0.8, 0.6, 1].map((k, i) => (
                  <span
                    key={i}
                    className="w-0.5 rounded-full bg-accent-red transition-[height] duration-75"
                    style={{ height: `${Math.max(12, Math.min(100, level * 100 * k))}%` }}
                  />
                ))}
              </span>
            )}
            {sttEnabled && (
              <button
                className={`shrink-0 w-8 h-8 rounded-full flex items-center justify-center transition-colors ${
                  recording
                    ? "bg-accent-red text-white animate-pulse"
                    : micReady
                      ? "bg-surface-3 text-ink hover:bg-accent/20"
                      : "bg-surface-3 text-faint opacity-40 cursor-not-allowed"
                }`}
                disabled={!micReady || transcribing}
                onPointerDown={() => {
                  // Keep the input's blur from collapsing the pill mid-press.
                  pillPress.current = true;
                }}
                onPointerUp={() => {
                  pillPress.current = false;
                }}
                onClick={() => void toggleDictation()}
                title={
                  !micReady
                    ? "Set up voice input on the Assistant Voice tab"
                    : transcribing
                      ? "Transcribing…"
                      : recording
                        ? "Stop and transcribe"
                        : "Dictate"
                }
              >
                {transcribing ? (
                  <RefreshCw size={13} className="animate-spin" />
                ) : recording ? (
                  <Square size={13} />
                ) : (
                  <Mic size={13} />
                )}
              </button>
            )}
            <button
              className="btn-primary shrink-0 w-8 h-8 rounded-full flex items-center justify-center"
              onClick={() => void send()}
              disabled={!input.trim() || state === "working"}
              title="Send"
            >
              <ArrowUp size={13} />
            </button>
          </>
        )}
      </div>
      {expanded && error && (
        <p className="absolute right-4 bottom-20 max-w-[22rem] text-[0.625rem] text-accent-red bg-surface-2/95 border border-border rounded px-2 py-1">
          {error}
        </p>
      )}
    </div>
  );
}

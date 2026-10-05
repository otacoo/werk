import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { ArrowUp, Mic, Sparkles } from "lucide-react";
import { commands } from "../bindings";
import { call } from "../utils/ipc";
import { loadAppearance } from "../utils/appearance";

const COLLAPSED = 84;
const EXPANDED_W = 404;
const EXPANDED_H = 84;
const POS_KEY = "werk.overlay.pos";

/// Always-on-top assistant overlay: a pulsing circle that expands into a
/// floating input. Enter sends to the assistant; Esc collapses.
export default function Overlay() {
  const [expanded, setExpanded] = useState(false);
  const [input, setInput] = useState("");
  const [state, setState] = useState<"idle" | "working" | "sent" | "attention" | "error">("idle");
  const [avatar, setAvatar] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const expandedRef = useRef(false);
  const sentTimer = useRef<number | null>(null);
  /// Click opens; a press that moves past the threshold drags the window.
  const dragging = useRef(false);
  const pressAt = useRef<{ x: number; y: number } | null>(null);
  /// Expanded state at press time: the input blurs before the click lands.
  const wasExpanded = useRef(false);

  const onPressDown = (e: ReactPointerEvent<HTMLButtonElement>) => {
    if (e.button !== 0) return;
    dragging.current = false;
    wasExpanded.current = expandedRef.current;
    pressAt.current = { x: e.screenX, y: e.screenY };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPressMove = async (e: ReactPointerEvent<HTMLButtonElement>) => {
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

  const onPressUp = (e: ReactPointerEvent<HTMLButtonElement>) => {
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

  // Transparent page: the pill owns all visible pixels. The theme is applied
  // here too: the overlay renders outside App, which normally loads it.
  useEffect(() => {
    loadAppearance();
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    document.body.style.overflow = "hidden";
  }, []);

  // Theme/accent/font changes in the main window re-apply here live.
  useEffect(() => {
    const unlisten = listen("appearance_changed", () => loadAppearance());
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
    void anchor(COLLAPSED, COLLAPSED, true);
    call(commands.assistantAvatar())
      .then(setAvatar)
      .catch(() => {});
  }, []);

  // Remember where the pill was dragged to (debounced; moves are frequent).
  useEffect(() => {
    const win = getCurrentWindow();
    let timer: number | null = null;
    const unlisten = win.onMoved(({ payload }) => {
      if (timer) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        localStorage.setItem(POS_KEY, JSON.stringify({ x: payload.x, y: payload.y }));
      }, 250);
    });
    return () => {
      unlisten.then((f) => f());
      if (timer) window.clearTimeout(timer);
    };
  }, []);

  const expand = async () => {
    if (expandedRef.current) {
      inputRef.current?.focus();
      return;
    }
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
    await anchor(COLLAPSED, COLLAPSED);
  };

  useEffect(() => {
    expandedRef.current = expanded;
  }, [expanded]);

  // The assistant bus drives the pulse and quick status.
  useEffect(() => {
    const unlisten = listen<Record<string, unknown>>("assistant_event", (event) => {
      const ev = event.payload;
      switch (ev.type) {
        case "content":
        case "reasoning_delta":
        case "tool_call":
        case "tool_result":
          setState("working");
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
      call(commands.assistantAvatar())
        .then(setAvatar)
        .catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const send = async () => {
    const text = input.trim();
    if (!text || state === "working") return;
    setError(null);
    setState("working");
    setInput("");
    try {
      // The local server may be stopped; start it like the Chat composer does.
      await call(commands.ensureServer());
      await call(commands.assistantSend(text, null, null));
      setState("sent");
      if (sentTimer.current) window.clearTimeout(sentTimer.current);
      sentTimer.current = window.setTimeout(() => setState("idle"), 2500);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("aborted")) {
        setError(msg);
        setState("error");
        if (sentTimer.current) window.clearTimeout(sentTimer.current);
        sentTimer.current = window.setTimeout(() => setState("idle"), 4000);
      } else {
        setState("idle");
      }
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

  return (
    <div className="h-screen w-screen flex items-end justify-end p-2 select-none">
      <div
        data-tauri-drag-region
        className={`flex items-center gap-2 rounded-full border border-border bg-surface-2/95 shadow-lg backdrop-blur transition-all ${
          state === "idle" && !expanded ? "opacity-70 hover:opacity-100" : "opacity-100"
        } ${expanded ? "pl-2 pr-1.5 py-1.5" : "p-1.5"}`}
      >
        <button
          className="relative w-11 h-11 rounded-full bg-surface-3 flex items-center justify-center shrink-0 overflow-hidden cursor-pointer"
          onPointerDown={onPressDown}
          onPointerMove={(e) => void onPressMove(e)}
          onPointerUp={onPressUp}
          onPointerCancel={onPressUp}
          onClick={onPressClick}
          title={expanded ? "Collapse" : "Ask the assistant (drag to move)"}
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
        </button>
        {expanded && (
          <>
            <input
              ref={inputRef}
              className="input flex-1 min-w-0 bg-transparent border-0 text-sm focus:outline-none"
              placeholder="Ask the assistant…"
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
                if (!input.trim()) void collapse();
              }}
            />
            <button
              className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center bg-surface-3 text-faint opacity-40 cursor-not-allowed"
              disabled
              title="Voice input — coming soon (hotkey TBD)"
            >
              <Mic size={13} />
            </button>
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

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { currentMonitor, getCurrentWindow, PhysicalPosition, PhysicalSize } from "@tauri-apps/api/window";
import { ArrowUp, Sparkles } from "lucide-react";
import { commands } from "../bindings";
import { call } from "../utils/ipc";

const COLLAPSED = 64;
const EXPANDED_W = 380;
const EXPANDED_H = 64;

/// Always-on-top assistant overlay: a pulsing circle that expands into a
/// floating input. Enter sends to the assistant; Esc collapses.
export default function Overlay() {
  const [expanded, setExpanded] = useState(false);
  const [input, setInput] = useState("");
  const [state, setState] = useState<"idle" | "working" | "sent" | "attention" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const expandedRef = useRef(false);
  const sentTimer = useRef<number | null>(null);

  // Transparent page: the pill owns all visible pixels.
  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    document.body.style.overflow = "hidden";
  }, []);

  const anchor = async (w: number, h: number, initial = false) => {
    const win = getCurrentWindow();
    const scale = await win.scaleFactor();
    if (initial) {
      const mon = await currentMonitor();
      if (mon) {
        const pw = Math.round(w * scale);
        const ph = Math.round(h * scale);
        await win.setSize(new PhysicalSize(pw, ph));
        await win.setPosition(
          new PhysicalPosition(
            mon.position.x + mon.size.width - pw - Math.round(16 * scale),
            mon.position.y + mon.size.height - ph - Math.round(16 * scale),
          ),
        );
        return;
      }
    }
    // Keep the bottom-right corner anchored across resizes (and drags).
    const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
    const pw = Math.round(w * scale);
    const ph = Math.round(h * scale);
    await win.setSize(new PhysicalSize(pw, ph));
    await win.setPosition(new PhysicalPosition(pos.x + size.width - pw, pos.y + size.height - ph));
  };

  useEffect(() => {
    void anchor(COLLAPSED, COLLAPSED, true);
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
          expanded ? "pl-2 pr-1.5 py-1.5" : "p-1.5"
        }`}
      >
        <button
          className="relative w-11 h-11 rounded-full bg-surface-3 flex items-center justify-center shrink-0"
          onClick={() => (expanded ? void collapse() : void expand())}
          title={expanded ? "Collapse" : "Ask the assistant"}
        >
          <span className={`absolute inset-0 rounded-full opacity-30 ${dot}`} />
          <Sparkles size={16} className="relative text-ink" />
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

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { CornerDownLeft, Square, Trash2, X } from "lucide-react";
import { commands } from "../../bindings";
import { call } from "../../utils/ipc";

/** Width limits for the panel; persisted in localStorage. */
const TERM_MIN = 260;
const TERM_MAX = 720;
const TERM_DEFAULT = 420;

type Line = { kind: "cmd" | "out" | "err" | "note"; text: string };

/// Embedded project terminal: commands run in the project folder with
/// streamed output. User-driven — the agent's sandbox does not apply.
export function TerminalPanel({ onClose }: { onClose: () => void }) {
  const [lines, setLines] = useState<Line[]>([]);
  const [input, setInput] = useState("");
  const [cwd, setCwd] = useState("");
  const [running, setRunning] = useState(false);
  const history = useRef<string[]>([]);
  const histIdx = useRef<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [width, setWidth] = useState(() => {
    const w = Number(localStorage.getItem("werk.chat.terminal.w"));
    return w >= TERM_MIN && w <= TERM_MAX ? w : TERM_DEFAULT;
  });

  const push = (line: Line) => setLines((prev) => [...prev.slice(-400), line]);

  useEffect(() => {
    let alive = true;
    call(commands.terminalCwd(true))
      .then((dir) => {
        if (!alive) return;
        setCwd(dir);
        push({ kind: "note", text: `Project terminal — commands run in ${dir}` });
      })
      .catch((e) => push({ kind: "err", text: String(e) }));
    inputRef.current?.focus();
    const out = listen<{ seq: number; line: string; err?: boolean }>("terminal_output", (ev) => {
      push({ kind: ev.payload.err ? "err" : "out", text: ev.payload.line });
    });
    const done = listen<{ seq: number; code: number | null }>("terminal_done", (ev) => {
      setRunning(false);
      const code = ev.payload.code;
      if (code !== null && code !== 0) push({ kind: "note", text: `exit ${code}` });
    });
    return () => {
      alive = false;
      out.then((f) => f());
      done.then((f) => f());
      // Closing the panel stops the shell, like closing a terminal window.
      call(commands.terminalKill()).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [lines]);

  const shortCwd = cwd.split(/[\\/]/).filter(Boolean).pop() ?? cwd;

  const run = async () => {
    const cmd = input.trim();
    if (!cmd || running) return;
    push({ kind: "cmd", text: `${shortCwd}> ${cmd}` });
    history.current.push(cmd);
    histIdx.current = null;
    setInput("");
    setRunning(true);
    try {
      await call(commands.terminalExec(cmd));
      // `cd` updates the tracked folder without spawning anything.
      if (/^cd(\s|$)/.test(cmd)) {
        const dir = await call(commands.terminalCwd(false));
        setCwd(dir);
      }
    } catch (e) {
      push({ kind: "err", text: String(e) });
      setRunning(false);
    }
  };

  const stop = async () => {
    try {
      await call(commands.terminalKill());
    } catch {}
    setRunning(false);
  };

  const startDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    let cur = startW;
    const move = (ev: MouseEvent) => {
      cur = Math.min(TERM_MAX, Math.max(TERM_MIN, startW - (ev.clientX - startX)));
      setWidth(cur);
    };
    const up = () => {
      localStorage.setItem("werk.chat.terminal.w", String(cur));
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      void run();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (history.current.length === 0) return;
      const idx =
        histIdx.current === null
          ? history.current.length - 1
          : Math.max(0, histIdx.current - 1);
      histIdx.current = idx;
      setInput(history.current[idx]);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (histIdx.current === null) return;
      const idx = histIdx.current + 1;
      if (idx >= history.current.length) {
        histIdx.current = null;
        setInput("");
      } else {
        histIdx.current = idx;
        setInput(history.current[idx]);
      }
    }
  };

  const color = (k: Line["kind"]) =>
    k === "cmd"
      ? "text-ink"
      : k === "err"
        ? "text-accent-yellow"
        : k === "note"
          ? "text-faint"
          : "text-dim";

  return (
    <aside
      className="shrink-0 border-l border-border bg-surface-1 flex flex-col min-h-0 relative"
      style={{ width }}
    >
      <div
        className="absolute top-0 left-0 w-1 h-full cursor-col-resize hover:bg-accent/40 transition-colors z-10"
        onMouseDown={startDrag}
        title="Drag to resize"
      />
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-border shrink-0">
        <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">
          Terminal
        </span>
        <div className="flex items-center gap-1.5">
          <span className="text-[0.625rem] text-faint font-mono truncate max-w-[140px]" title={cwd}>
            {shortCwd}
          </span>
          <button
            className="text-dim hover:text-ink transition-colors"
            onClick={() => setLines([])}
            title="Clear the output"
          >
            <Trash2 size={12} />
          </button>
          <button
            className="text-dim hover:text-ink transition-colors"
            onClick={onClose}
            title="Hide terminal"
          >
            <X size={13} />
          </button>
        </div>
      </div>
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-3 py-1.5 font-mono text-[0.6875rem] leading-snug select-text"
      >
        {lines.length === 0 ? (
          <p className="text-faint">
            Run commands in the project folder. `cd` is tracked; Up/Down recalls history.
          </p>
        ) : (
          lines.map((l, i) => (
            <pre key={i} className={`whitespace-pre-wrap break-words ${color(l.kind)}`}>
              {l.text}
            </pre>
          ))
        )}
      </div>
      <div className="flex items-center gap-1.5 px-3 py-1.5 border-t border-border shrink-0">
        <span className="text-[0.6875rem] text-faint font-mono shrink-0">{shortCwd} &gt;</span>
        <input
          ref={inputRef}
          className="input flex-1 min-w-0 py-0.5 px-1.5 text-xs font-mono"
          placeholder={running ? "running…" : "command"}
          value={input}
          disabled={running}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
        />
        {running ? (
          <button
            className="btn-secondary py-0.5 px-1.5 shrink-0"
            onClick={stop}
            title="Stop the command"
          >
            <Square size={11} />
          </button>
        ) : (
          <button
            className="btn-secondary py-0.5 px-1.5 shrink-0"
            onClick={() => void run()}
            disabled={!input.trim()}
            title="Run (Enter)"
          >
            <CornerDownLeft size={11} />
          </button>
        )}
      </div>
    </aside>
  );
}

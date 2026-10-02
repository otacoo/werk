import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArrowUp,
  Brain,
  Check,
  Eye,
  FileWarning,
  FolderOpen,
  Paperclip,
  RefreshCw,
  Square,
  Cloud,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  HarnessCapabilities,
  ReasoningOptions,
  ContextStats,
  RunChange,
  ServerStatus,
  TodoDto,
} from "../bindings";
import { call } from "../utils/ipc";
import { getBubbleAlign, getShowRunChanges, subscribeBubbleAlign, subscribeShowRunChanges } from "../utils/appearance";
import { subscribeConfigChanged } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { playNotificationSound } from "../utils/sounds";
import ProjectTree from "../components/ProjectTree";
import { Markdown } from "./chat/markdown";
import { ChatSidebar } from "./chat/sidebar";
import { TerminalPanel } from "./chat/terminal";
import { ExternalChatControls, EFFORT_LABELS } from "./chat/external-controls";
import {
  ChangesCard,
  ContextRing,
  CopyButton,
  ReasoningBlock,
  ResponseFooter,
  SLASH_COMMANDS,
  SysNotice,
  TodoStrip,
  ToolCard,
  fmtTok,
} from "./chat/components";
import type { Tab } from "../App";

// ── Items (messages + tool activity) ────────────────────────────────────────

type Item =
  | {
      kind: "msg";
      role: "user" | "assistant";
      content: string;
      model?: string;
      tokps?: number | null;
      time?: number;
      elapsedMs?: number;
      tokens?: number;
      reasoning?: string;
      attachments?: string[];
      /** Image data URLs for multimodal turns (restored sessions, live sends). */
      images?: string[];
      /** Local-only entries (/help) — never in backend history. */
      local?: boolean;
    }
  | {
      kind: "tool";
      callId: string;
      tool: string;
      args: string;
      output?: { ok: boolean; text: string };
      /** Model a subagent spawn card or subagent tool call ran on. */
      subagentModel?: string;
    }
  | { kind: "reasoning"; text: string }
  | { kind: "sys"; text: string }
  | { kind: "changes"; files: RunChange[]; added: number; removed: number }
  | {
      kind: "approval";
      seq: number;
      tool: string;
      command: string | null;
      args: string;
      resolved?: "denied" | "once" | "session" | "project";
    };

interface ChatAttachment {
  name: string;
  kind: "image" | "text";
  path: string;
  preview?: string;
  text?: string;
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** Model that ran a subagent tool call, resolved from its `sub{seq}:...` id. */
function subagentModelFor(callId: string, items: Item[]): string | undefined {
  const m = /^(sub\d+):/.exec(callId);
  if (!m) return undefined;
  const spawn = items.find((it) => it.kind === "tool" && it.callId === m[1]);
  return spawn && spawn.kind === "tool" ? spawn.subagentModel : undefined;
}

export default function Chat({ go, active = true }: { go: (t: Tab) => void; active?: boolean }) {
  const [status, setStatus] = useState<ServerStatus>(getServerStatus());
  const [items, setItems] = useState<Item[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const [streamText, setStreamText] = useState<string | null>(null);
  const [reasoningText, setReasoningText] = useState<string | null>(null);
  const [reasoningOpen, setReasoningOpen] = useState(false);
  // Reasoning deltas are still arriving (drives the "Thinking…" block label).
  const [reasoningLive, setReasoningLive] = useState(false);
  const [reasoningEffort, setReasoningEffort] = useState("");
  // Reasoning levels come from the model's template — others 500. Hidden for
  // non-reasoning models.
  const [reasoningOpts, setReasoningOpts] = useState<ReasoningOptions | null>(null);
  const [caps, setCaps] = useState<HarnessCapabilities | null>(null);
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [contextUsed, setContextUsed] = useState<number | null>(null);
  const [todos, setTodos] = useState<TodoDto[]>([]);
  // Live slot context (refreshed with the status poll); falls back to the
  // last run's usage + GGUF length when the server can't report it.
  const [slotCtx, setSlotCtx] = useState<ContextStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  /// Right-hand panel: the project file tree or the embedded terminal.
  const [panel, setPanelState] = useState<"files" | "terminal" | null>(() => {
    try {
      return localStorage.getItem("werk.chat.files.open") === "0" ? null : "files";
    } catch {
      return "files";
    }
  });
  const [runChangesShown, setRunChangesShown] = useState(getShowRunChanges());
  useEffect(() => subscribeShowRunChanges(setRunChangesShown), []);
  const setPanel = (v: "files" | "terminal" | null) => {
    try {
      localStorage.setItem("werk.chat.files.open", v === "files" ? "1" : "0");
    } catch {}
    setPanelState(v);
  };
  const [activeProject, setActiveProject] = useState<string | null>(null);
  /// External API mode: chat works server-less through the configured provider.
  const [externalMode, setExternalMode] = useState(false);
  const [autoStart, setAutoStart] = useState(false);
  const [externalTarget, setExternalTarget] = useState("");
  const [favorites, setFavorites] = useState<string[]>([]);
  const [runStatus, setRunStatus] = useState<"thinking" | "loading" | "working" | null>(null);
  // Live run stats: deltas ≈ tokens (fallback), server usage when reported.
  const [liveDeltas, setLiveDeltas] = useState(0);
  const [liveUsage, setLiveUsage] = useState<{ prompt: number; gen: number } | null>(null);
  const runStartRef = useRef(0);
  const runGenRef = useRef(0);
  // Elapsed counts from the first generated token (load time excluded).
  const firstTokenRef = useRef(0);
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!streaming) return;
    const id = setInterval(() => setTick((t) => t + 1), 500);
    return () => clearInterval(id);
  }, [streaming]);
  const liveElapsed = streaming && firstTokenRef.current
    ? Math.max(0, Math.round((Date.now() - firstTokenRef.current) / 1000))
    : null;
  const liveTps = streaming && firstTokenRef.current
    ? (() => {
        const secs = (Date.now() - firstTokenRef.current) / 1000;
        if (secs <= 0) return null;
        // Run average: every turn's reported tokens over the run so far.
        return (runGenRef.current > 0 ? runGenRef.current : liveDeltas) / secs;
      })()
    : null;
  // Live subagent count (ref: updated from stream events, no re-render needed
  // beyond the status line it drives).
  const liveSubs = useRef(0);
  const markSubSpawned = () => {
    liveSubs.current += 1;
    setRunStatus("working");
  };
  const markSubFinished = () => {
    liveSubs.current = Math.max(0, liveSubs.current - 1);
    setRunStatus(liveSubs.current > 0 ? "working" : "thinking");
  };
  // Live transcript accumulators (the global bus outlives any one send
  // call, so stream state lives in refs, not send() closures).
  const accRef = useRef("");
  const reasoningAccRef = useRef("");
  const contentStartedRef = useRef(false);
  // Narration streamed before a tool call belongs to the turn that is now
  // ending — pin it as its own bubble so it survives instead of vanishing.
  const flushNarration = () => {
    if (accRef.current.trim()) {
      const flushed = accRef.current;
      accRef.current = "";
      setItems((prev) => [...prev, { kind: "msg", role: "assistant", content: flushed }]);
    }
  };
  const approvalSeq = useRef(0);
  // Pending ask_user question (the loop parks until the user answers).
  const [pendingQuestion, setPendingQuestion] = useState<{
    question: string;
    options: { title: string; description?: string }[];
  } | null>(null);
  const [questionDraft, setQuestionDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // Auto-grow the input with content, capped at a third of the view
  // (scrollbar past that). Measured and applied directly on the DOM node —
  // going through state would skip re-applying an unchanged height after the
  // direct "0px" reset (React diffs against its own last render).
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "0px";
    const cap = Math.max(120, Math.floor(window.innerHeight / 3));
    el.style.height = `${Math.min(el.scrollHeight + 2, cap)}px`;
  }, [input]);
  // Chat text scale (zoom on the transcript; persists in localStorage).
  const [chatZoom, setChatZoom] = useState(() => {
    const z = Number(localStorage.getItem("werk.chat.zoom"));
    return z >= 0.85 && z <= 1.4 ? z : 1;
  });
  const [bubbleAlign, setBubbleAlign] = useState(getBubbleAlign());
  useEffect(() => subscribeBubbleAlign(setBubbleAlign), []);
  const stepZoom = (d: number) =>
    setChatZoom((z) => {
      const next = Math.min(1.4, Math.max(0.85, Math.round((z + d) * 100) / 100));
      localStorage.setItem("werk.chat.zoom", String(next));
      return next;
    });

  const refreshActiveProject = async () => {
    try {
      const c = await call(commands.getConfig());
      applyConfigFlags(c);
    } catch {}
  };

  // External API mode chats without the local server; the composer and stats
  // must stay live in that case.
  const applyConfigFlags = (c: AppConfig) => {
    setActiveProject(c.harness_active_project ?? null);
    const external = c.server_mode === "external";
    setExternalMode(external);
    setExternalTarget(external ? (c.external_target ?? "").trim() : "");
    setAutoStart(c.server_auto_start ?? false);
    if (external) setFavorites(c.provider_favorites ?? []);
  };

  // Local server: start it on demand when auto-start is enabled.
  const ensureServerReady = async () => {
    if (externalMode) return true;
    try {
      await call(commands.ensureServer());
      return true;
    } catch (e) {
      setItems((prev) => [
        ...prev,
        { kind: "sys", text: `Could not start the server: ${e}` },
      ]);
      return false;
    }
  };

  const selectTarget = async (t: string) => {
    setExternalTarget(t);
    try {
      await call(commands.setExternalTarget(t));
    } catch (e) {
      setError(String(e));
    }
  };

  // Hidden tabs keep their stream state but pause the stats polling.
  const activeRef = useRef(active);
  // Mirrors `streaming` for the server-log listener (registered once).
  const streamingRef = useRef(streaming);

  useEffect(() => {
    const refreshCaps = () => {
      call(commands.harnessAgentCapabilities()).then(setCaps).catch(() => {});
      call(commands.harnessReasoningOptions()).then(setReasoningOpts).catch(() => {});
    };
    const refreshConfig = () => {
      call(commands.getConfig()).then(applyConfigFlags).catch(() => {});
    };
    // Model-dependent info goes stale when the server (re)starts with a
    // different model — refresh on the transition into running.
    let wasRunning = getServerStatus().type === "running";
    const onStatus = (s: ServerStatus) => {
      const running = s.type === "running";
      if (running && !wasRunning) {
        refreshCaps();
      }
      wasRunning = running;
      setStatus(s);
    };
    const poll = () => {
      // The stats hit the local server; skip while the tab is hidden.
      if (!activeRef.current) return;
      call(commands.harnessContextStats()).then(setSlotCtx).catch(() => {});
    };
    refreshCaps();
    refreshConfig();
    refreshActiveProject();
    // The transcript lives in the backend — restore it so chats are
    // consultable even when the server is stopped.
    restoreFromBackend();
    poll();
    const id = setInterval(poll, 2000);
    // Status and mode/target changes signal instead of polling.
    const unsubStatus = subscribeServerStatus(onStatus);
    const unsubConfig = subscribeConfigChanged(refreshConfig);
    return () => {
      clearInterval(id);
      unsubStatus();
      unsubConfig();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    activeRef.current = active;
    if (active) call(commands.harnessContextStats()).then(setSlotCtx).catch(() => {});
  }, [active]);

  useEffect(() => {
    streamingRef.current = streaming;
  }, [streaming]);

  // Router children announce their loads in the server log; reflect them while
  // a run waits so the status reads "Loading model…" instead of "Thinking…".
  useEffect(() => {
    const unlisten = listen<string>("server_log", (e) => {
      const line = e.payload ?? "";
      if (/loading model|ensure_model|waiting until model/i.test(line)) {
        if (streamingRef.current) setRunStatus("loading");
      } else if (/model loaded|listening on/i.test(line)) {
        if (streamingRef.current) setRunStatus("thinking");
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const restoreFromBackend = async () => {
    try {
      const res = await call(commands.harnessAgentHistory());
      setTodos(res.todos ?? []);
      const metaByIndex = new Map((res.meta ?? []).map((m) => [m.index, m.meta]));
      const cap = (s: string, n: number) =>
        s.length > n ? `${s.slice(0, n)}\n[…truncated]` : s;
      const restored: Item[] = [];
      res.messages.forEach((m, i) => {
        if (m.role === "user" && (m.content || (m.images && m.images.length > 0))) {
          // Compaction summaries read as notices, not user bubbles.
          if ((m.content ?? "").startsWith("[Compacted context")) {
            restored.push({ kind: "sys", text: m.content ?? "" });
            return;
          }
          restored.push({
            kind: "msg",
            role: "user",
            content: m.content || "(attachments only)",
            images: m.images ?? undefined,
          });
        } else if (m.role === "assistant" && m.content && !m.tool_calls) {
          const meta = metaByIndex.get(i);
          restored.push({
            kind: "msg",
            role: "assistant",
            content: m.content,
            model: meta?.model ?? undefined,
            tokps: meta?.tokens_per_sec ?? null,
            tokens: meta?.gen_tokens ?? undefined,
            elapsedMs: meta?.elapsed_ms ?? undefined,
            reasoning: meta?.reasoning ?? undefined,
            images: m.images ?? undefined,
          });
          const changes = meta?.changes;
          if (changes && changes.files.length > 0) {
            restored.push({
              kind: "changes",
              files: changes.files,
              added: changes.added,
              removed: changes.removed,
            });
          }
        } else if (m.role === "assistant" && m.tool_calls) {
          // Narration accompanying tool calls is its own bubble (mirrors
          // the live flush on tool_call so restores match live transcripts).
          if (m.content) {
            restored.push({
              kind: "msg",
              role: "assistant",
              content: cap(m.content, 4000),
            });
          }
          for (const tc of m.tool_calls) {
            let args = tc.arguments ?? "";
            try {
              args = JSON.stringify(JSON.parse(args), null, 1);
            } catch {}
            restored.push({
              kind: "tool",
              callId: tc.id ?? "",
              tool: tc.name ?? "?",
              args: cap(args, 2000),
            });
          }
        } else if (m.role === "tool" && typeof m.content === "string" && m.content) {
          for (let k = restored.length - 1; k >= 0; k--) {
            const it = restored[k];
            if (it.kind === "tool" && it.callId === m.tool_call_id && it.output === undefined) {
              it.output = { ok: true, text: cap(m.content, 4000) };
              break;
            }
          }
        }
      });
      setItems(restored);
      setError(null);
    } catch {}
  };

  const avgTokps = (() => {
    const vals = items.flatMap((it) =>
      it.kind === "msg" && it.role === "assistant" && it.tokps != null && it.tokps > 0 ? [it.tokps as number] : [],
    );
    return vals.length > 0 ? vals.reduce((a, b) => a + b, 0) / vals.length : null;
  })();

  // Live prompt rate for this run: prompt tokens over the wait for the first
  // token. The server's /metrics gauges are lifetime averages, not this run.
  const livePromptTps =
    streaming && liveUsage && firstTokenRef.current > runStartRef.current
      ? liveUsage.prompt / Math.max(0.001, (firstTokenRef.current - runStartRef.current) / 1000)
      : null;
  const lastTokps = (() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "msg" && it.role === "assistant" && it.tokps != null && it.tokps > 0) {
        return it.tokps as number;
      }
    }
    return null;
  })();

  const lastAssistant = (() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "msg" && it.role === "assistant") return it;
    }
    return null;
  })();

  // Approval prompts arrive via a global event while the send invoke is still
  // pending (the loop parks until the user decides).
  useEffect(() => {
    const unlisten = listen<{
      type: string;
      tool: string;
      command: string | null;
      args: string;
    }>("harness_approval", (event) => {
      const p = event.payload;
      approvalSeq.current += 1;
      const seq = approvalSeq.current;
      void playNotificationSound("permissions");
      setItems((prev) => [
        ...prev,
        { kind: "approval", seq, tool: p.tool, command: p.command, args: p.args },
      ]);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // ask_user questions arrive via a global event while the send invoke is
  // still pending (the loop parks until the user picks or types an answer).
  useEffect(() => {
    const unlisten = listen<{
      type: string;
      question: string;
      options: { title: string; description?: string }[];
    }>("harness_question", (event) => {
      const p = event.payload;
      void playNotificationSound("permissions");
      setQuestionDraft("");
      setPendingQuestion({ question: p.question, options: p.options ?? [] });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Stay pinned to the newest message: layout effect (post-DOM, pre-paint)
  // plus a rAF pass for late layout (images, code blocks) shifting height.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    const raf = requestAnimationFrame(() => {
      if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    });
    return () => cancelAnimationFrame(raf);
  }, [items, streamText, reasoningText]);

  // Live transcript over the global bus; runs are exclusive so no filtering.
  useEffect(() => {
    const unlisten = listen<Record<string, unknown>>("harness_event", (event) => {
      const ev = event.payload;
      switch (ev.type) {
        case "content":
          accRef.current += (ev.text as string) ?? "";
          setStreamText(accRef.current);
          setLiveDeltas((d) => d + 1);
          if (!firstTokenRef.current) firstTokenRef.current = Date.now();
          if (!contentStartedRef.current) {
            contentStartedRef.current = true;
            setReasoningOpen(false);
            setReasoningLive(false);
          }
          setRunStatus("thinking");
          break;
        case "reasoning_delta":
          reasoningAccRef.current += (ev.text as string) ?? "";
          setReasoningText(reasoningAccRef.current);
          setReasoningLive(true);
          setLiveDeltas((d) => d + 1);
          if (!firstTokenRef.current) firstTokenRef.current = Date.now();
          break;
        case "usage":
          runGenRef.current += Number(ev.completion_tokens ?? 0);
          setLiveUsage({
            prompt: Number(ev.prompt_tokens ?? 0),
            gen: Number(ev.completion_tokens ?? 0),
          });
          break;
        case "tool_call": {
          // Reasoning that preceded the call belongs here, not in the final
          // answer's footer — freeze it as its own block first.
          const preCallReasoning = reasoningAccRef.current;
          reasoningAccRef.current = "";
          flushNarration();
          setStreamText(null);
          setReasoningText(null);
          setReasoningOpen(false);
          setReasoningLive(false);
          setRunStatus("thinking");
          setItems((prev) => {
            const extra: Item[] =
              preCallReasoning.trim().length > 0
                ? [{ kind: "reasoning", text: preCallReasoning }]
                : [];
            if (prev.some((it) => it.kind === "tool" && it.callId === ev.call_id)) {
              return extra.length > 0 ? [...prev, ...extra] : prev;
            }
            return [
              ...prev,
              ...extra,
              {
                kind: "tool",
                callId: (ev.call_id as string) ?? "",
                tool: (ev.tool as string) ?? "?",
                args: (ev.args as string) ?? "",
              },
            ];
          });
          break;
        }
        case "tool_result": {
          setItems((prev) =>
            prev.map((it) =>
              it.kind === "tool" && it.callId === ev.call_id && it.output === undefined
                ? { ...it, output: { ok: !!ev.ok, text: String(ev.output ?? "") } }
                : it,
            ),
          );
          break;
        }
        case "subagent_spawned": {
          const preCallReasoning = reasoningAccRef.current;
          reasoningAccRef.current = "";
          flushNarration();
          setStreamText(null);
          setReasoningText(null);
          setReasoningLive(false);
          markSubSpawned();
          setItems((prev) => {
            const extra: Item[] =
              preCallReasoning.trim().length > 0
                ? [{ kind: "reasoning", text: preCallReasoning }]
                : [];
            if (prev.some((it) => it.kind === "tool" && it.callId === ev.call_id)) {
              return extra.length > 0 ? [...prev, ...extra] : prev;
            }
            return [
              ...prev,
              ...extra,
              {
                kind: "tool",
                callId: (ev.call_id as string) ?? "",
                tool: `⟳ ${(ev.kind as string) ?? "worker"} subagent`,
                args: (ev.goal as string) ?? "",
                subagentModel: typeof ev.model === "string" ? ev.model : undefined,
              },
            ];
          });
          break;
        }
        case "todos_changed":
          setTodos((ev.todos as TodoDto[]) ?? []);
          break;
        case "subagent_finished":
          markSubFinished();
          setItems((prev) =>
            prev.map((it) =>
              it.kind === "tool" && it.callId === ev.call_id && it.output === undefined
                ? { ...it, output: { ok: true, text: String(ev.summary ?? "") } }
                : it,
            ),
          );
          break;
        case "compacted":
          setItems((prev) => [
            ...prev,
            {
              kind: "sys",
              text: `Context compacted - ${Number(ev.removed ?? 0)} older messages summarized to stay within the context window.`,
            },
          ]);
          break;
        case "distilled":
          if (typeof ev.text === "string") {
            setItems((prev) => [...prev, { kind: "sys", text: ev.text as string }]);
          }
          break;
        case "notice":
          if (typeof ev.text === "string" && /loading/i.test(ev.text)) {
            setRunStatus("loading");
          }
          break;
        default:
          break;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Safety net: if live tool events back up, pull the backend transcript every
  // couple of seconds mid-run and merge tool cards by call id (never replaces
  // local items, so approval prompts and notices survive).
  useEffect(() => {
    if (!streaming) return;
    const merge = async () => {
      try {
        const res = await call(commands.harnessAgentHistory());
        setItems((prev) => {
          const known = new Set(
            prev.flatMap((it) => (it.kind === "tool" ? [it.callId] : [])),
          );
          const results = new Map<string, { ok: boolean; text: string }>();
          for (const m of res.messages) {
            if (m.role === "tool" && typeof m.content === "string" && m.tool_call_id) {
              results.set(m.tool_call_id, {
                ok: !m.content.startsWith("error:"),
                text: m.content.length > 4000 ? `${m.content.slice(0, 4000)}\n[…truncated]` : m.content,
              });
            }
          }
          const added: Item[] = [];
          for (const m of res.messages) {
            if (m.role !== "assistant" || !m.tool_calls) continue;
            for (const tc of m.tool_calls) {
              const callId = tc.id ?? "";
              if (!callId || known.has(callId)) continue;
              known.add(callId);
              let args = tc.arguments ?? "";
              try {
                args = JSON.stringify(JSON.parse(args), null, 1);
              } catch {}
              added.push({
                kind: "tool",
                callId,
                tool: tc.name ?? "?",
                args: args.length > 2000 ? `${args.slice(0, 2000)}\n[…truncated]` : args,
                output: results.get(callId),
              });
            }
          }
          const filled = prev.map((it) => {
            if (it.kind === "tool" && it.output === undefined) {
              const output = results.get(it.callId);
              if (output) return { ...it, output };
            }
            return it;
          });
          return added.length > 0 ? [...filled, ...added] : filled;
        });
      } catch {}
    };
    merge();
    const id = setInterval(merge, 2000);
    return () => clearInterval(id);
  }, [streaming]);

  const slashMatches = !slashDismissed && input.startsWith("/") && !/\s/.test(input)
    ? SLASH_COMMANDS.filter((c) => c.name.startsWith(input.toLowerCase()))
    : [];

  const send = async () => {
    const text = input.trim();
    // While a run is in flight, Send steers it instead: the message queues
    // into the loop and joins history as a normal user turn (slash commands
    // and attachments stay send-only).
    if (streaming) {
      if (!text || !canSend) return;
      await steer(text);
      return;
    }
    if (!text && attachments.length === 0) return;
    // Slash commands run locally — never sent to the model, never persisted.
    if (text.startsWith("/")) {
      const cmd = text.split(/\s+/)[0].toLowerCase();
      if (cmd === "/help") {
        setInput("");
        setItems((prev) => [
          ...prev,
          {
            kind: "msg",
            role: "assistant",
            content:
              "Chat commands:\n" +
              SLASH_COMMANDS.map((c) => `- ${c.name} — ${c.hint}`).join("\n"),
            time: Date.now(),
            local: true,
          },
        ]);
        return;
      }
      if (cmd === "/new" || cmd === "/reset" || cmd === "/clear") {
        setInput("");
        await newChat();
        return;
      }
      if (cmd === "/compact") {
        setInput("");
        if (!(await ensureServerReady())) return;
        try {
          await call(commands.harnessAgentCompact());
        } catch (e) {
          setError(String(e));
        }
        restoreFromBackend();
        return;
      }
      if (cmd === "/distill") {
        setInput("");
        if (!(await ensureServerReady())) return;
        setRunStatus("thinking");
        try {
          const report = await call(commands.harnessDistill());
          // Restore first: the backend cleared the session, then keep the report.
          await restoreFromBackend();
          setItems((prev) => [...prev, { kind: "sys", text: report }]);
        } catch (e) {
          setError(String(e));
        } finally {
          setRunStatus(null);
        }
        return;
      }
      setInput("");
      setError(`Unknown command "${cmd}". Type /help for the list.`);
      return;
    }
    if (!(await ensureServerReady())) return;
    setItems((prev) => [
      ...prev,
      {
        kind: "msg",
        role: "user",
        content: text || "(attachments only)",
        time: Date.now(),
        attachments: attachments.map((a) => a.name),
        images: attachments
          .filter((a) => a.kind === "image" && a.preview)
          .map((a) => a.preview as string),
      },
    ]);
    setInput("");
    setStreaming(true);
    setStreamText("");
    setReasoningText(null);
    setReasoningOpen(false);
    setReasoningLive(false);
    setLiveDeltas(0);
    setLiveUsage(null);
    runStartRef.current = Date.now();
    runGenRef.current = 0;
    firstTokenRef.current = 0;
    liveSubs.current = 0;
    accRef.current = "";
    reasoningAccRef.current = "";
    contentStartedRef.current = false;
    setRunStatus("thinking");
    setError(null);

    try {
      const res = await call(
        commands.harnessAgentSend(
          text,
          reasoningEffort || null,
          attachments.map((a) => ({
            name: a.name,
            kind: a.kind,
            path: a.path,
            data_base64:
              a.kind === "image" && a.preview ? a.preview.split(",", 2)[1] ?? null : null,
            text: a.kind === "text" ? (a.text ?? "") : null,
          })),
        ),
      );
      setContextUsed(res.prompt_tokens ?? null);
      setItems((prev) => [
        ...prev,
        {
          kind: "msg",
          role: "assistant",
          content: res.text || accRef.current || "(no response)",
          model: res.model ?? undefined,
          tokps: res.tokens_per_sec ?? null,
          elapsedMs: res.elapsed_ms,
          tokens: res.gen_tokens,
          reasoning: res.reasoning ?? (reasoningAccRef.current || undefined),
        },
      ]);
      const changes = res.changes;
      if (changes && changes.files.length > 0 && getShowRunChanges()) {
        setItems((prev) => [
          ...prev,
          { kind: "changes", files: changes.files, added: changes.added, removed: changes.removed },
        ]);
      }
      setAttachments([]);
      void playNotificationSound("agent");
    } catch (e) {
      const msg = String(e);
      const aborted = msg.includes("aborted");
      if (accRef.current) {
        const leftover = accRef.current;
        accRef.current = "";
        setItems((prev) => [
          ...prev,
          { kind: "msg", role: "assistant", content: leftover + (aborted ? "  (stopped)" : "") },
        ]);
      }
      if (!aborted) {
        setError(msg);
        void playNotificationSound("errors");
      }
    } finally {
      setStreaming(false);
      setStreamText(null);
      setReasoningText(null);
      setReasoningLive(false);
      setAttachments([]);
      setPendingQuestion(null);
      setQuestionDraft("");
      liveSubs.current = 0;
      setRunStatus(null);
      // The active model may have changed (roles, router switches) — keep the
      // context size and capability badges fresh.
      call(commands.harnessAgentCapabilities()).then(setCaps).catch(() => {});
    }
  };

  // ── Attachments (+ button): images for vision models, text inline ──────────

  const clearTodos = async () => {
    setTodos([]);
    try {
      await call(commands.harnessTodosClear());
    } catch {}
  };

  const attachFiles = async () => {
    let picked: string | string[] | null = null;
    try {
      picked = await openDialog({ multiple: true, directory: false });
    } catch {
      return;
    }
    const paths: string[] = Array.isArray(picked) ? picked : picked ? [picked] : [];
    for (const path of paths) {
      try {
        const read = await call(commands.harnessReadAttachment(path));
        if (read.kind === "image" && read.data_base64) {
          const ext = (read.name.split(".").pop() ?? "png").toLowerCase();
          const mime =
            ext === "jpg" || ext === "jpeg"
              ? "image/jpeg"
              : ext === "webp"
                ? "image/webp"
                : "image/png";
          setAttachments((prev) => [
            ...prev,
            { name: read.name, kind: "image", path, preview: `data:${mime};base64,${read.data_base64}` },
          ]);
        } else if (read.text != null) {
          setAttachments((prev) => [...prev, { name: read.name, kind: "text", path, text: read.text ?? undefined }]);
        }
      } catch (e) {
        setError(String(e));
      }
    }
  };

  const removeAttachment = (idx: number) => {
    setAttachments((prev) => prev.filter((_, i) => i !== idx));
  };

  const decide = async (seq: number, grant: "once" | "session" | "project" | null) => {
    setItems((prev) =>
      prev.map((it) =>
        it.kind === "approval" && it.seq === seq && !it.resolved
          ? { ...it, resolved: grant ?? "denied" }
          : it,
      ),
    );
    try {
      await call(commands.harnessAgentDecide(grant));
    } catch {}
  };

  const answerQuestion = async (answer: string) => {
    const text = answer.trim();
    if (!text || !pendingQuestion) return;
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.harnessQuestionAnswer(text));
    } catch {}
  };

  const steer = async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    setInput("");
    const bubble = { kind: "msg", role: "user", content: trimmed, time: Date.now() } as Item;
    setItems((prev) => [...prev, bubble]);
    try {
      await call(commands.harnessAgentSteer(trimmed));
    } catch {
      // Run ended mid-steer: drop the optimistic bubble, the model never saw it.
      setItems((prev) => prev.filter((it) => it !== bubble));
    }
    inputRef.current?.focus();
  };

  const newChat = async () => {
    if (streaming) return;
    try {
      await call(commands.harnessAgentReset());
    } catch {}
    setItems([]);
    setError(null);
    liveSubs.current = 0;
    setRunStatus(null);
  };

  // Delete = rewind: drop the response and the user turn that produced it.
  // Local-only bubbles (/help) are skipped — they are not backend history.
  const lastAssistantIdx = (() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "msg" && it.role === "assistant" && !it.local) return i;
    }
    return -1;
  })();
  const lastUserIdx = (() => {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "msg" && it.role === "user") return i;
    }
    return -1;
  })();

  const deleteResponse = async (idx: number) => {
    if (idx !== lastAssistantIdx || streaming) return;
    try {
      await call(commands.harnessAgentRewind());
      setItems((prev) => prev.slice(0, lastUserIdx));
      setInput((prev) => {
        const removed = items[lastUserIdx];
        return removed && removed.kind === "msg" ? removed.content : prev;
      });
    } catch {}
  };

  // The view is always available: chats are consultable without the server,
  // and sending requires a running server + an active project directory.
  const serverRunning = status.type === "running";
  const canSend =
    (serverRunning || (externalMode && externalTarget !== "") || (autoStart && !externalMode)) &&
    activeProject != null;
  // Bubble alignment: user on the right by default; the model takes the other side.
  const userSide = bubbleAlign === "right";
  const userJustify = userSide ? "justify-end" : "justify-start";
  const modelJustify = userSide ? "justify-start" : "justify-end";

  return (
    <div className="flex h-full min-h-0">
      {sidebarOpen && (
        <ChatSidebar
          visible={active}
          panel={panel}
          onPanel={setPanel}
          onProjectChanged={() => {
            refreshActiveProject();
            restoreFromBackend();
          }}
          onSessionPicked={() => {
            restoreFromBackend();
          }}
        />
      )}

      <div className="flex-1 flex flex-col min-h-0 min-w-0">
        <div className="flex items-center justify-between px-4 py-1.5 border-b border-border">
          <div className="flex items-center gap-3">
            <button
              className="inline-flex items-center gap-1.5 text-xs text-dim hover:text-ink"
              onClick={() => setSidebarOpen((v) => !v)}
              title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
            >
              {sidebarOpen ? (
                <svg width="7" height="9" viewBox="0 0 8 10" className="fill-current shrink-0">
                  <path d="M7 1 L1 5 L7 9 Z" />
                </svg>
              ) : (
                <svg width="7" height="9" viewBox="0 0 8 10" className="fill-current shrink-0">
                  <path d="M1 1 L7 5 L1 9 Z" />
                </svg>
              )}
              <span>Sidebar</span>
            </button>
          </div>
          <div className="flex items-center gap-5">
            <div className="flex items-center gap-1.5 select-none" title="Chat text size">
              <button
                className="text-dim hover:text-ink leading-none"
                onClick={() => stepZoom(0.05)}
              >
                <span className="text-sm font-semibold">A</span>
              </button>
              <span className="text-faint text-xs leading-none">/</span>
              <button
                className="text-dim hover:text-ink leading-none"
                onClick={() => stepZoom(-0.05)}
              >
                <span className="text-[0.6875rem] font-semibold">a</span>
              </button>
            </div>
            <button
              className="text-xs text-dim hover:text-ink"
              onClick={newChat}
              disabled={streaming}
              title="Start a new conversation"
            >
              New chat
            </button>
          </div>
        </div>

        {!serverRunning && (
          <div className="flex items-center gap-2 px-4 py-1.5 border-b border-border text-[0.6875rem] text-dim">
            {externalMode ? (
              <>
                <Cloud size={11} className="text-accent-soft" />
                <span>
                  {externalTarget ? (
                    <>
                      External API mode — chatting through{" "}
                      <span className="font-mono">{externalTarget}</span>.
                    </>
                  ) : (
                    "External API mode — no model selected yet."
                  )}
                </span>
                {!externalTarget && (
                  <button className="ml-auto text-accent hover:underline" onClick={() => go("mode")}>
                    Go to Mode
                  </button>
                )}
              </>
            ) : (
              <>
                <RefreshCw size={11} className={status.type === "starting" ? "animate-spin" : ""} />
                <span>
                  {status.type === "starting"
                    ? "Server is starting — the model is loading."
                    : "Server is not running — your chats stay available below."}
                </span>
                {status.type !== "starting" && (
                  <button
                    className="ml-auto text-accent hover:underline"
                    onClick={() => go("run")}
                  >
                    Go to Run
                  </button>
                )}
              </>
            )}
          </div>
        )}
        {serverRunning && !activeProject && (
          <div className="flex items-center gap-2 px-4 py-1.5 border-b border-border text-[0.6875rem] text-dim">
            <FolderOpen size={11} />
            Select or add a working directory (project) in the sidebar to start chatting.
          </div>
        )}

        {/* Messages — select-text re-enables selection (body disables it for the title bar) */}
        <div ref={scrollRef} className="flex-1 overflow-y-auto px-6 py-4 space-y-3 select-text" style={{ zoom: chatZoom }}>
          {items.length === 0 && streamText === null && <EmptyState />}
          {items.map((it, i) => {
            if (it.kind === "msg") {
              const isUser = it.role === "user";
              const isLastAssistant = it.role === "assistant" && i === lastAssistantIdx;
              return (
                <div key={i} className={`flex ${isUser ? userJustify : modelJustify}`}>
                  <div className="max-w-[80%] xl:max-w-[75%]">
                    {isUser && (
                      <div
                        className={`text-[0.625rem] text-faint mb-0.5 select-text ${
                          userSide ? "text-right" : "text-left"
                        }`}
                      >
                        {it.time ? formatTime(it.time) : ""}
                      </div>
                    )}
                    {it.reasoning && <ReasoningBlock text={it.reasoning} />}
                    <div
                      className={`rounded px-3 py-2 ${
                        isUser ? "bg-accent/20 text-ink text-sm whitespace-pre-wrap break-words" : "bg-surface-2 text-ink"
                      }`}
                    >
                      {isUser ? it.content : <Markdown content={it.content} />}
                    </div>
                    {it.images && it.images.length > 0 && (
                      <div className={`flex flex-wrap gap-1.5 mt-1.5 ${isUser ? userJustify : ""}`}>
                        {it.images.map((src, k) => (
                          <img key={k} src={src} alt="attached image" className="max-h-40 rounded border border-border object-contain" />
                        ))}
                      </div>
                    )}
                    {isUser && it.attachments && it.attachments.length > 0 && (
                      <div className={`flex flex-wrap gap-1 mt-1 ${userJustify}`}>
                        {it.attachments.map((name) => (
                          <span key={name} className="text-[0.625rem] text-dim font-mono">📎 {name}</span>
                        ))}
                      </div>
                    )}
                    {!isUser ? (
                      <ResponseFooter
                        model={it.model}
                        tokps={externalMode ? null : it.tokps}
                        elapsedMs={it.elapsedMs}
                        tokens={it.tokens}
                        onCopy={() => navigator.clipboard.writeText(it.content).catch(() => {})}
                        onDelete={isLastAssistant && !streaming ? () => deleteResponse(i) : undefined}
                      />
                    ) : (
                      <div className={`flex items-center mt-1 px-1 text-[0.625rem] text-faint ${userJustify}`}>
                        <CopyButton text={it.content} label="Copy" />
                      </div>
                    )}
                  </div>
                </div>
              );
            }
            if (it.kind === "tool") {
              return (
                <ToolCard
                  key={it.callId || `tool-${i}`}
                  tool={it.tool}
                  args={it.args}
                  output={it.output}
                  subagentModel={it.subagentModel ?? subagentModelFor(it.callId, items)}
                />
              );
            }
            if (it.kind === "reasoning") {
              return (
                <div key={`reasoning-${i}`} className={`flex ${modelJustify}`}>
                  <div className="max-w-[80%] xl:max-w-[75%]">
                    <ReasoningBlock text={it.text} />
                  </div>
                </div>
              );
            }
            if (it.kind === "sys") {
              return <SysNotice key={`sys-${i}`} text={it.text} />;
            }
            if (it.kind === "changes") {
              if (!runChangesShown) return null;
              return (
                <ChangesCard
                  key={`changes-${i}`}
                  files={it.files}
                  added={it.added}
                  removed={it.removed}
                />
              );
            }
            return (
              <div key={i} className={`flex ${modelJustify}`}>
                <div className="max-w-[85%] xl:max-w-[75%] rounded border border-accent-yellow/40 bg-accent-yellow/5 px-3 py-2 text-xs">
                  <p className="text-ink flex items-center gap-1.5 mb-1">
                    <FileWarning size={11} className="text-accent-yellow shrink-0" />
                    Approval requested: <span className="font-medium">{it.tool}</span>
                    {it.command && <span className="badge-gray text-[0.625rem]">{it.command}</span>}
                  </p>
                  <pre className="whitespace-pre-wrap break-words text-[0.6875rem] text-dim mb-2 max-h-40 overflow-y-auto">
                    {it.args}
                  </pre>
                  {!it.resolved ? (
                    <div className="flex flex-wrap items-center gap-2">
                      <button className="btn-secondary py-1 px-2" onClick={() => decide(it.seq, "once")}>
                        <Check size={11} /> Allow once
                      </button>
                      <button className="btn-secondary py-1 px-2" onClick={() => decide(it.seq, "session")}>
                        <Check size={11} /> Allow session
                      </button>
                      <button className="btn-secondary py-1 px-2" onClick={() => decide(it.seq, "project")} title="Remember for this project">
                        <Check size={11} /> Allow always
                      </button>
                      <button className="btn-ghost py-1 px-2 text-accent-red" onClick={() => decide(it.seq, null)}>
                        <X size={11} /> Deny
                      </button>
                    </div>
                  ) : (
                    <p className="text-faint">
                      {it.resolved === "denied"
                        ? "Denied"
                        : it.resolved === "project"
                          ? "Allowed (always)"
                          : `Allowed (${it.resolved})`}
                    </p>
                  )}
                </div>
              </div>
            );
          })}
          {reasoningText !== null && (
            <div className={`flex ${modelJustify}`}>
              <div className="max-w-[80%] w-full xl:max-w-[75%]">
                <ReasoningBlock
                  text={reasoningText}
                  streaming={reasoningLive}
                  open={reasoningOpen}
                  onToggle={() => setReasoningOpen((v) => !v)}
                />
              </div>
            </div>
          )}
          {streamText !== null && (
            <div className={`flex ${modelJustify}`}>
              <div className="max-w-[80%] xl:max-w-[75%] rounded px-3 py-2 bg-surface-2 text-ink select-text">
                <Markdown content={streamText} />
                {streaming && <span className="ml-0.5 inline-block w-2 h-4 bg-dim animate-pulse align-middle" />}
              </div>
            </div>
          )}
          {streaming && streamText === null && items.length > 0 && (
            <div className={`flex ${modelJustify}`}>
              <RefreshCw size={13} className="animate-spin text-dim" />
            </div>
          )}
        </div>

          {error && (
            <div className="px-6 pb-2 flex items-start gap-2">
              <p className="flex-1 text-xs text-accent-red break-words select-text">{error}</p>
            </div>
          )}

          {streaming && (
            <div className="px-6 pb-1 flex items-center gap-1.5 text-[0.6875rem] text-dim select-none">
              {runStatus === "loading" ? (
                <>
                  <RefreshCw size={11} className="animate-spin" />
                  <span>Loading model…</span>
                </>
              ) : runStatus === "working" ? (
                <>
                  <span className="w-1.5 h-1.5 rounded-full bg-orange-400 animate-pulse" />
                  <span>Working…</span>
                </>
              ) : (
                <>
                  <span className="w-1.5 h-1.5 rounded-full bg-accent-soft animate-pulse" />
                  <span>Thinking…</span>
                </>
              )}
              {!externalMode && liveTps !== null && <span className="tabular-nums">· {liveTps.toFixed(1)} t/s</span>}
              {liveElapsed !== null && <span className="tabular-nums">· {liveElapsed}s</span>}
              {liveUsage?.prompt != null && liveUsage.prompt > 0 && (
                <span className="tabular-nums">· ctx {fmtTok(liveUsage.prompt)}</span>
              )}
            </div>
          )}

        <div className="border-t border-border p-3">
          {slashMatches.length > 0 && (
            <div className="mb-2 rounded border border-border bg-surface-2 py-1">
              {slashMatches.map((c) => (
                <button
                  key={c.name}
                  className="w-full flex items-center gap-2 px-2.5 py-1 text-left text-xs hover:bg-surface-3"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => { setInput(c.name); setSlashDismissed(true); }}
                  title={c.hint}
                >
                  <span className="font-mono text-ink">{c.name}</span>
                  <span className="text-dim truncate">{c.hint}</span>
                </button>
              ))}
            </div>
          )}
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-1.5 mb-2">
              {attachments.map((a, i) => (
                <div
                  key={`${a.name}-${i}`}
                  className="flex items-center gap-1.5 rounded border border-border bg-surface-2 pl-1 pr-1.5 py-0.5 text-[0.6875rem] text-ink"
                  title={a.path}
                >
                  {a.kind === "image" && a.preview ? (
                    <img src={a.preview} alt={a.name} className="w-6 h-6 rounded object-cover" />
                  ) : (
                    <span className="text-dim font-mono">📄</span>
                  )}
                  <span className="max-w-[160px] truncate font-mono">{a.name}</span>
                  <button
                    className="text-faint hover:text-accent-red"
                    onClick={() => removeAttachment(i)}
                    title="Remove attachment"
                  >
                    <X size={11} />
                  </button>
                </div>
              ))}
            </div>
          )}
          {pendingQuestion && (
            <div className="mb-2 rounded border border-accent/60 bg-accent/10 px-3 py-2">
              <p className="text-xs text-ink mb-1.5">
                <span className="text-accent-soft font-medium">Question: </span>
                {pendingQuestion.question}
              </p>
              <div className="flex flex-col items-stretch gap-1.5 mb-1.5">
                {pendingQuestion.options.map((o, i) => (
                  <button
                    key={`${o.title}-${i}`}
                    className="btn-secondary py-1.5 px-2.5 text-xs text-left"
                    onClick={() => answerQuestion(o.title)}
                  >
                    <span className="block text-ink">{o.title}</span>
                    {o.description && (
                      <span className="block text-faint text-[0.6875rem] leading-snug mt-0.5">
                        {o.description}
                      </span>
                    )}
                  </button>
                ))}
              </div>
              <div className="flex items-center gap-1.5">
                <input
                  className="input flex-1 py-1 px-2 text-xs"
                  placeholder="Or type your own answer…"
                  value={questionDraft}
                  onChange={(e) => setQuestionDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      answerQuestion(questionDraft);
                    }
                  }}
                />
                <button
                  className="btn-primary py-1 px-2 text-xs"
                  disabled={!questionDraft.trim()}
                  onClick={() => answerQuestion(questionDraft)}
                >
                  Send
                </button>
              </div>
            </div>
          )}
          {todos.length > 0 && (
            <div className="mb-1.5">
              <TodoStrip todos={todos} onClear={clearTodos} />
            </div>
          )}
          <div className="flex items-end gap-2">
            <ContextRing
              used={slotCtx?.used ?? contextUsed}
              total={slotCtx?.total ?? null}
              avgTokps={externalMode ? null : avgTokps}
              model={lastAssistant?.model}
              genTokens={lastAssistant?.tokens}
              liveGenTps={externalMode ? null : (liveTps ?? lastTokps)}
              livePromptTps={externalMode ? null : livePromptTps}
            />
            <button
              className="btn-secondary shrink-0 py-2 px-2.5 mb-0.5"
              onClick={attachFiles}
              disabled={streaming}
              title="Attach files or images"
            >
              <Paperclip size={14} />
            </button>
            <textarea
              ref={inputRef}
              className="input flex-1 resize-none text-sm overflow-y-auto"
              style={{ minHeight: "2.5rem", maxHeight: `${Math.floor(window.innerHeight / 3)}px` }}
              placeholder={
                !canSend
                  ? activeProject == null
                    ? "Select a project to start chatting…"
                    : externalMode
                      ? "Pick a model on the Mode tab to start chatting…"
                      : autoStart
                        ? "The server starts when you send…"
                        : "Start the server to start chatting…"
                  : streaming
                    ? "Steer the running agent… (Enter to send)"
                    : "Send a message…"
              }
              value={input}
              disabled={!canSend}
              onChange={(e) => { setInput(e.target.value); setSlashDismissed(false); }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send();
                } else if (e.key === "Tab" && slashMatches.length > 0) {
                  e.preventDefault();
                  setInput(slashMatches[0].name);
                } else if (e.key === "Escape" && slashMatches.length > 0) {
                  e.preventDefault();
                  setSlashDismissed(true);
                }
              }}
            />
            {/* Reasoning effort offers only levels the model's template accepts.
                External providers use the controls under the composer. */}
            <div className="flex flex-col items-start gap-1 shrink-0 pb-0.5">
              <div className="flex items-center gap-1.5 h-4" title="Model capabilities">
                {caps?.vision && (
                  <span title="Model supports vision (image input)">
                    <Eye size={13} className="text-[#3B82F6]" />
                  </span>
                )}
                {caps?.reasoning && (
                  <span title="Model supports reasoning (thinking)">
                    <Brain size={13} className="text-[#E5484D]" />
                  </span>
                )}
              </div>
              {!externalMode && reasoningOpts?.supported && reasoningOpts.levels.length > 0 && (
                <select
                  className="input py-1 px-1 text-[0.625rem] w-20"
                  value={reasoningOpts.levels.includes(reasoningEffort) || reasoningEffort === "" ? reasoningEffort : ""}
                  onChange={(e) => setReasoningEffort(e.target.value)}
                  title="Reasoning effort (levels from the model's chat template)"
                  disabled={streaming}
                >
                  <option value="">Default</option>
                  {reasoningOpts.levels.map((level) => (
                    <option key={level} value={level}>
                      {EFFORT_LABELS[level] ?? level}
                    </option>
                  ))}
                </select>
              )}
            </div>
            {streaming && (
              <button className="btn-danger shrink-0" onClick={() => { liveSubs.current = 0; setRunStatus(null); setPendingQuestion(null); setQuestionDraft(""); call(commands.harnessAgentAbort()).catch(() => {}); }} title="Stop">
                <Square size={13} />
                Stop
              </button>
            )}
            <button
              className="btn-primary shrink-0"
              onClick={send}
              disabled={(streaming ? !input.trim() : (!input.trim() && attachments.length === 0)) || !canSend}
              title={
                !canSend
                  ? externalMode
                    ? "Pick a model on the Mode tab to start chatting"
                    : "Needs a running server and an active project"
                  : streaming
                    ? "Steer the running agent"
                    : "Send"
              }
            >
              <ArrowUp size={14} />
            </button>
          </div>
          {externalMode && <ExternalChatControls
            target={externalTarget}
            favorites={favorites}
            effort={reasoningEffort}
            onTarget={selectTarget}
            onEffort={setReasoningEffort}
            disabled={streaming}
          />}
        </div>
      </div>
      {panel === "terminal" && <TerminalPanel onClose={() => setPanel(null)} />}
      {panel === "files" && (
        <ProjectTree
          activeProject={activeProject}
          streaming={streaming}
          onClose={() => setPanel(null)}
        />
      )}
    </div>
  );
}

function EmptyState() {
  return (
    <div className="h-full flex flex-col items-center justify-center gap-4 text-center px-6">
      <p className="text-8xl font-bold tracking-tight select-none">
        werk<span className="text-accent text-[1.1em]">.</span>
      </p>
      <p className="text-sm text-dim">Send a message to start. Use / for commands.</p>
    </div>
  );
}

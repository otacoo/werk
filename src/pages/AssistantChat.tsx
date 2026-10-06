import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  ArrowUp,
  Brain,
  Check,
  FileWarning,
  Paperclip,
  RefreshCw,
  RotateCcw,
  Scissors,
  Skull,
  Sparkles,
  Square,
  Undo2,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type { AppConfig, ContextStats, RunResult, ServerStatus } from "../bindings";
import { call } from "../utils/ipc";
import { subscribeConfigChanged } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { playNotificationSound } from "../utils/sounds";
import { SpeechQueue } from "../utils/speechQueue";
import ProfileAvatar from "../components/ProfileAvatar";
import { Markdown } from "./chat/markdown";
import { ContextRing, CopyButton, ReasoningBlock, SysNotice, ToolCard } from "./chat/components";
import type { Tab } from "../App";

type Item =
  | {
      kind: "msg";
      role: "user" | "assistant";
      content: string;
      model?: string;
      tokps?: number | null;
      elapsedMs?: number;
      tokens?: number;
      promptTokens?: number;
      reasoning?: string;
      images?: string[];
    }
  | {
      kind: "tool";
      callId: string;
      tool: string;
      args: string;
      output?: { ok: boolean; text: string };
    }
  | {
      kind: "approval";
      tool: string;
      command: string | null;
      args: string;
      resolved?: string;
    }
  | { kind: "reasoning"; text: string }
  | { kind: "sys"; text: string };

interface AssistantAttachment {
  name: string;
  kind: "image" | "text";
  path: string;
  preview?: string;
  text?: string;
}

const SLASH = [
  { name: "/compact", hint: "Summarize older turns to free context" },
  { name: "/distill", hint: "Save what it learned into memory, then start fresh" },
  { name: "/rewind", hint: "Drop the last exchange" },
  { name: "/new", hint: "Start a fresh conversation" },
  { name: "/forget", hint: "Wipe every memory and start over (lobotomy)" },
  { name: "/help", hint: "List the assistant commands" },
];

/// Assistant chat surface: one continuous thread with personality, memory,
/// and the remember/ask_user/get_time/skill toolset.
export default function AssistantChat({
  go,
  active = true,
}: {
  go: (t: Tab) => void;
  active?: boolean;
}) {
  const [status, setStatus] = useState<ServerStatus>(getServerStatus());
  const [items, setItems] = useState<Item[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [starting, setStarting] = useState(false);
  const [streamText, setStreamText] = useState<string | null>(null);
  const [reasoningText, setReasoningText] = useState<string | null>(null);
  const [reasoningOpen, setReasoningOpen] = useState(false);
  const [reasoningLive, setReasoningLive] = useState(false);
  const [attachments, setAttachments] = useState<AssistantAttachment[]>([]);
  const [contextUsed, setContextUsed] = useState<number | null>(null);
  const [slotCtx, setSlotCtx] = useState<ContextStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [runStatus, setRunStatus] = useState<"loading" | "thinking" | null>(null);
  /// The saved transcript ends mid-run; offer resume or dismiss.
  const [interrupted, setInterrupted] = useState(false);
  /// Hold-to-confirm progress on the lobotomy button.
  const [holdPct, setHoldPct] = useState(0);
  const holdRaf = useRef(0);
  const [assistantName, setAssistantName] = useState("Werk");
  const [voice, setVoice] = useState<{ enabled: boolean; autoplay: boolean }>({
    enabled: false,
    autoplay: false,
  });
  const [avatar, setAvatar] = useState<string | null>(null);
  const [externalMode, setExternalMode] = useState(false);
  const [externalTarget, setExternalTarget] = useState("");
  // Reasoning traces visibility (all blocks, live and restored).
  const [showReasoning, setShowReasoning] = useState(() => {
    try {
      return localStorage.getItem("werk.assistant.reasoning") !== "0";
    } catch {
      return true;
    }
  });
  const toggleReasoning = () =>
    setShowReasoning((v) => {
      const next = !v;
      try {
        localStorage.setItem("werk.assistant.reasoning", next ? "1" : "0");
      } catch {}
      return next;
    });
  const [pendingQuestion, setPendingQuestion] = useState<{
    question: string;
    options: { title: string; description?: string }[];
  } | null>(null);
  const [questionDraft, setQuestionDraft] = useState("");

  const scrollRef = useRef<HTMLDivElement>(null);
  const accRef = useRef("");
  const reasoningAccRef = useRef("");
  const activeRef = useRef(active);
  const streamingRef = useRef(streaming);
  const startingRef = useRef(false);
  /// Narration settings for the event listener (state is render-lagged).
  const voiceRef = useRef({ enabled: false, autoplay: false });
  /// Reply narration: sentences synthesize while the model is still writing.
  const speech = useRef(new SpeechQueue());

  useEffect(() => {
    voiceRef.current = voice;
  }, [voice]);

  // Narration failures (bad reference file, model errors) must not vanish.
  useEffect(() => {
    speech.current.onError = (message) => {
      setItems((prev) => [...prev, { kind: "sys", text: `Narration failed: ${message}` }]);
    };
  }, []);

  const refreshConfig = () => {
    call(commands.getConfig())
      .then(applyConfig)
      .catch(() => {});
  };

  const loadAvatar = () => {
    call(commands.assistantAvatar())
      .then(setAvatar)
      .catch(() => setAvatar(null));
  };

  const applyConfig = (c: AppConfig) => {
    const external = c.server_mode === "external";
    setExternalMode(external);
    setExternalTarget(external ? (c.external_target ?? "").trim() : "");
    setAssistantName(c.assistant?.name?.trim() || "Werk");
    setVoice({
      enabled: c.assistant?.tts_enabled ?? false,
      autoplay: c.assistant?.tts_autoplay ?? false,
    });
  };

  const restore = async () => {
    if (streamingRef.current) return;
    try {
      const res = await call(commands.assistantHistory());
      const metaByIndex = new Map((res.meta ?? []).map((m) => [m.index, m.meta]));
      const restored: Item[] = [];
      res.messages.forEach((m, i) => {
        if (m.role === "user" && (m.content || (m.images && m.images.length > 0))) {
          if ((m.content ?? "").startsWith("[Compacted context")) {
            restored.push({ kind: "sys", text: m.content ?? "" });
            return;
          }
          if ((m.content ?? "").startsWith("[Reminder]")) {
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
            promptTokens: meta?.prompt_tokens ?? undefined,
            elapsedMs: meta?.elapsed_ms ?? undefined,
            reasoning: meta?.reasoning ?? undefined,
          });
        }
      });
      setItems(restored);
      setInterrupted(res.interrupted ?? false);
    } catch {}
  };

  // Mount: config, transcript, context stats, and the live stream.
  useEffect(() => {
    refreshConfig();
    loadAvatar();
    void restore();
    const poll = () => {
      if (!activeRef.current) return;
      call(commands.assistantContextStats()).then(setSlotCtx).catch(() => {});
    };
    poll();
    const id = setInterval(poll, 2000);
    const unsubStatus = subscribeServerStatus(setStatus);
    const unsubConfig = subscribeConfigChanged(() => {
      refreshConfig();
      loadAvatar();
    });
    return () => {
      clearInterval(id);
      unsubStatus();
      unsubConfig();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    activeRef.current = active;
    if (active) {
      void restore();
      loadAvatar();
      call(commands.assistantContextStats()).then(setSlotCtx).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  useEffect(() => {
    streamingRef.current = streaming;
  }, [streaming]);

  useEffect(() => {
    const unlisten = listen<string>("server_log", (e) => {
      const line = e.payload ?? "";
      const busy = () => streamingRef.current || startingRef.current;
      if (/loading model|ensure_model|waiting until model/i.test(line)) {
        if (busy()) setRunStatus("loading");
      } else if (/model loaded|listening on/i.test(line)) {
        if (busy()) setRunStatus("thinking");
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Live transcript over the assistant bus.
  useEffect(() => {
    const unlisten = listen<Record<string, unknown>>("assistant_event", (event) => {
      const ev = event.payload;
      switch (ev.type) {
        case "content":
          accRef.current += (ev.text as string) ?? "";
          setStreamText(accRef.current);
          if (!streamingRef.current) setRunStatus("thinking");
          // Narrate this run only; foreign runs are spoken by their starter.
          if (streamingRef.current && voiceRef.current.enabled && voiceRef.current.autoplay) {
            speech.current.push(accRef.current);
          }
          break;
        case "reasoning_delta":
          reasoningAccRef.current += (ev.text as string) ?? "";
          setReasoningText(reasoningAccRef.current);
          setReasoningLive(true);
          break;
        case "usage":
          setContextUsed(Number(ev.prompt_tokens ?? 0) || null);
          break;
        case "tool_call": {
          const preCallReasoning = reasoningAccRef.current;
          reasoningAccRef.current = "";
          setStreamText(null);
          setReasoningText(null);
          setReasoningOpen(false);
          setReasoningLive(false);
          speech.current.abort();
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
        case "tool_result":
          setItems((prev) =>
            prev.map((it) =>
              it.kind === "tool" && it.callId === ev.call_id && it.output === undefined
                ? { ...it, output: { ok: !!ev.ok, text: String(ev.output ?? "") } }
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
            setItems([{ kind: "sys", text: ev.text }]);
          }
          break;
        case "reminder":
          if (typeof ev.text === "string") {
            setItems((prev) => [...prev, { kind: "sys", text: `Reminder: ${ev.text}` }]);
          }
          break;
        case "notice":
          if (typeof ev.text === "string" && /loading/i.test(ev.text)) {
            setRunStatus("loading");
          }
          break;
        case "done":
          // A run this page did not start (overlay, reminder): drop the live
          // bubble and restore the transcript, or the reply would show twice.
          if (!streamingRef.current) {
            accRef.current = "";
            reasoningAccRef.current = "";
            speech.current.abort();
            setStreamText(null);
            setReasoningText(null);
            setReasoningOpen(false);
            setReasoningLive(false);
            setRunStatus(null);
            void restore();
          }
          break;
        default:
          break;
      }
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // ask_user questions arrive while the send invoke is still pending.
  useEffect(() => {
    const unlisten = listen<{
      question: string;
      options: { title: string; description?: string }[];
    }>("assistant_question", (event) => {
      const p = event.payload;
      setQuestionDraft("");
      setPendingQuestion({ question: p.question, options: p.options ?? [] });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // System-control approvals park the run until answered.
  useEffect(() => {
    const unlisten = listen<{ tool: string; command: string | null; args: string }>(
      "assistant_approval",
      (event) => {
        const p = event.payload;
        setItems((prev) => [
          ...prev,
          { kind: "approval", tool: p.tool, command: p.command, args: p.args },
        ]);
        void playNotificationSound("permissions");
      },
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Stay pinned to the newest message.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    const raf = requestAnimationFrame(() => {
      if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    });
    return () => cancelAnimationFrame(raf);
  }, [items, streamText, reasoningText]);

  const canSend = externalMode ? externalTarget !== "" : true;

  const ensureServerReady = async () => {
    if (externalMode) return true;
    const already = status.type === "running" && status.ready;
    if (!already) {
      startingRef.current = true;
      setStarting(true);
    }
    try {
      await call(commands.ensureServer());
      return true;
    } catch (e) {
      setError(`Could not start the server: ${e}`);
      return false;
    } finally {
      if (!already) {
        startingRef.current = false;
        setStarting(false);
      }
    }
  };

  const compact = async () => {
    if (streaming || starting) return;
    setError(null);
    if (!(await ensureServerReady())) return;
    try {
      await call(commands.assistantCompact());
    } catch (e) {
      setError(String(e));
    }
  };

  const distill = async () => {
    if (streaming || starting) return;
    setError(null);
    if (!(await ensureServerReady())) return;
    try {
      await call(commands.assistantDistill());
    } catch (e) {
      setError(String(e));
    }
  };

  const restart = async () => {
    if (streaming || starting) return;
    setError(null);
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.assistantReset());
      setItems([]);
    } catch (e) {
      setError(String(e));
    }
  };

  const rewind = async () => {
    if (streaming || starting) return;
    setError(null);
    try {
      await call(commands.assistantRewind());
      await restore();
    } catch (e) {
      setError(String(e));
    }
  };

  /// Full reset: conversation and every memory. The skull button requires a
  /// 3-second hold; the typed `/forget` is deliberate enough on its own.
  const forget = async () => {
    if (streaming || starting) return;
    setError(null);
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.assistantForget());
      setItems([{ kind: "sys", text: "Lobotomy complete — it has no idea who you are." }]);
    } catch (e) {
      setError(String(e));
    }
  };

  /// Hold-to-confirm progress for the skull button (0..1 over 3 seconds).
  const startForgetHold = () => {
    if (streaming || starting) return;
    const started = performance.now();
    cancelAnimationFrame(holdRaf.current);
    const tick = (t: number) => {
      const pct = Math.min(1, (t - started) / 3000);
      setHoldPct(pct);
      if (pct >= 1) {
        setHoldPct(0);
        void forget();
        return;
      }
      holdRaf.current = requestAnimationFrame(tick);
    };
    holdRaf.current = requestAnimationFrame(tick);
  };

  const cancelForgetHold = () => {
    cancelAnimationFrame(holdRaf.current);
    setHoldPct(0);
  };

  const send = async () => {
    const text = input.trim();
    if (!text || streaming || starting) return;
    if (text === "/help") {
      setInput("");
      setItems((prev) => [
        ...prev,
        { kind: "sys", text: SLASH.map((s) => `${s.name} — ${s.hint}`).join("\n") },
      ]);
      return;
    }
    if (text === "/compact") {
      setInput("");
      await compact();
      return;
    }
    if (text === "/distill") {
      setInput("");
      await distill();
      return;
    }
    if (text === "/rewind") {
      setInput("");
      await rewind();
      return;
    }
    if (text === "/new") {
      setInput("");
      await restart();
      return;
    }
    if (text === "/forget") {
      setInput("");
      await forget();
      return;
    }
    if (!canSend) return;
    // Commit the message and lock the composer before the server starts: the
    // first load can take a while, and the header reports that phase.
    setItems((prev) => [
      ...prev,
      {
        kind: "msg",
        role: "user",
        content: text || "(attachments only)",
        images: attachments
          .filter((a) => a.kind === "image" && a.preview)
          .map((a) => a.preview as string),
      },
    ]);
    setInput("");
    setAttachments([]);
    const payload = attachments.map((a) => ({
      name: a.name,
      kind: a.kind,
      path: a.path,
      data_base64: a.kind === "image" && a.preview ? a.preview.split(",", 2)[1] ?? null : null,
      text: a.kind === "text" ? (a.text ?? "") : null,
    }));
    await runTurn(() => call(commands.assistantSend(text, null, payload)));
  };

  /// Run one turn: lock the composer, stream, finalize, narrate.
  const runTurn = async (invoke: () => Promise<RunResult>) => {
    if (!(await ensureServerReady())) return;
    streamingRef.current = true;
    setStreaming(true);
    setStreamText("");
    setReasoningText(null);
    setReasoningOpen(false);
    setReasoningLive(false);
    setRunStatus("thinking");
    setError(null);
    accRef.current = "";
    reasoningAccRef.current = "";
    speech.current.reset();

    try {
      const res = await invoke();
      setInterrupted(false);
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
          promptTokens: res.prompt_tokens ?? undefined,
          reasoning: res.reasoning ?? (reasoningAccRef.current || undefined),
        },
      ]);
      setAttachments([]);
      void playNotificationSound("agent");
      // Narrate the reply when voice is on; the queue already synthesized the
      // sentences that finished while the model was still writing.
      if (voice.enabled && voice.autoplay && res.text) {
        void speech.current.finish(res.text);
      }
    } catch (e) {
      const msg = String(e);
      const aborted = msg.includes("aborted");
      speech.current.abort();
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
      streamingRef.current = false;
      setStreaming(false);
      setStreamText(null);
      setReasoningText(null);
      setReasoningLive(false);
      setAttachments([]);
      setPendingQuestion(null);
      setQuestionDraft("");
      setRunStatus(null);
    }
  };

  /// Continue an interrupted run from its saved transcript.
  const resume = async () => {
    if (streaming || starting) return;
    setInterrupted(false);
    await runTurn(() => call(commands.assistantResume()));
  };

  const dismissInterrupted = async () => {
    setInterrupted(false);
    try {
      await call(commands.assistantDismissInterrupted());
    } catch (e) {
      setError(String(e));
    }
  };

  const stop = async () => {
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.assistantAbort());
    } catch {}
  };

  const answerQuestion = async (answer: string) => {
    const text = answer.trim();
    if (!text || !pendingQuestion) return;
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.assistantQuestionAnswer(text));
    } catch {}
  };

  const decide = async (index: number, grant: string | null) => {
    setItems((prev) =>
      prev.map((it, i) =>
        i === index && it.kind === "approval"
          ? { ...it, resolved: grant === null ? "denied" : grant }
          : it,
      ),
    );
    try {
      await call(commands.assistantDecide(grant));
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
            {
              name: read.name,
              kind: "image",
              path,
              preview: `data:${mime};base64,${read.data_base64}`,
            },
          ]);
        } else if (read.text != null) {
          setAttachments((prev) => [
            ...prev,
            { name: read.name, kind: "text", path, text: read.text ?? undefined },
          ]);
        }
      } catch (e) {
        setError(String(e));
      }
    }
  };

  const slashMatches =
    input.startsWith("/") && !input.includes(" ")
      ? SLASH.filter((s) => s.name.startsWith(input))
      : [];

  const sessionUsage = items.reduce(
    (acc, it) => {
      if (it.kind === "msg" && it.role === "assistant") {
        acc.prompt += it.promptTokens ?? 0;
        acc.gen += it.tokens ?? 0;
      }
      return acc;
    },
    { prompt: 0, gen: 0 },
  );
  const hasSessionUsage = sessionUsage.prompt > 0 || sessionUsage.gen > 0;

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-3 px-4 py-2 border-b border-border shrink-0">
        <ProfileAvatar src={avatar} name={assistantName} size={36} />
        <div className="min-w-0">
          <p className="text-sm font-medium text-ink truncate">{assistantName}</p>
          <div className="flex items-center gap-1.5 text-[0.625rem] text-faint min-w-0">
            {starting || runStatus === "loading" ? (
              <RefreshCw size={10} className="animate-spin shrink-0" />
            ) : runStatus === "thinking" ? (
              <Brain size={10} className="animate-pulse shrink-0" />
            ) : null}
            <span className="truncate">
              {starting
                ? runStatus === "loading"
                  ? "Loading the model…"
                  : "Starting the server…"
                : runStatus === "loading"
                  ? "Loading model…"
                  : runStatus === "thinking"
                    ? "Thinking…"
                    : "Assistant"}
            </span>
          </div>
        </div>
        <div className="ml-auto flex items-center gap-1.5 shrink-0">
          <ContextRing
            used={slotCtx?.used ?? contextUsed}
            total={slotCtx?.total ?? null}
            avgTokps={null}
            sessionPrompt={hasSessionUsage ? sessionUsage.prompt : null}
            sessionGen={hasSessionUsage ? sessionUsage.gen : null}
            dropDown
          />
          <button
            className={`btn-ghost py-1 px-2 text-[0.625rem] ${
              showReasoning ? "text-ink bg-accent/15" : "opacity-50"
            }`}
            onClick={toggleReasoning}
            title={showReasoning ? "Hide reasoning traces" : "Show reasoning traces"}
          >
            <Brain size={11} /> Reasoning
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={compact}
            disabled={streaming || starting}
            title="Summarize older turns (also /compact)"
          >
            <Scissors size={11} /> Compact
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={distill}
            disabled={streaming || starting}
            title="Save what it learned into memory and start fresh (also /distill)"
          >
            <Sparkles size={11} /> Distill
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={rewind}
            disabled={streaming || starting}
            title="Drop the last exchange (also /rewind)"
          >
            <Undo2 size={11} />
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={restart}
            disabled={streaming || starting}
            title="Start a fresh conversation (also /new)"
          >
            <RotateCcw size={11} />
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem] hover:!text-accent-red relative overflow-hidden"
            onPointerDown={startForgetHold}
            onPointerUp={cancelForgetHold}
            onPointerLeave={cancelForgetHold}
            onPointerCancel={cancelForgetHold}
            disabled={streaming || starting}
            title="Hold 3 seconds to wipe every memory and start over (also /forget)"
          >
            <span
              className="absolute inset-y-0 left-0 bg-accent-red/40"
              style={{ width: `${holdPct * 100}%` }}
            />
            <Skull size={11} className="relative" />
          </button>
        </div>
      </div>

      <div ref={scrollRef} className="flex-1 overflow-y-auto px-6 py-4 space-y-3 select-text">
        {interrupted && !streaming && (
          <div className="card border-accent/40 bg-accent/5 flex items-center gap-3">
            <AlertTriangle size={14} className="text-accent shrink-0" />
            <span className="text-xs text-dim flex-1">
              The previous run was interrupted. Resume where it left off, or dismiss it.
            </span>
            <button
              className="btn-primary text-xs py-1 px-2 shrink-0"
              onClick={() => void resume()}
            >
              Resume
            </button>
            <button
              className="btn-secondary text-xs py-1 px-2 shrink-0"
              onClick={() => void dismissInterrupted()}
            >
              Dismiss
            </button>
          </div>
        )}
        {items.length === 0 && streamText === null && (
          <div className="h-full flex flex-col items-center justify-center text-center gap-2">
            <Sparkles size={20} className="text-faint" />
            <p className="text-sm text-dim">
              {canSend ? `Say something to ${assistantName}.` : "Set an external model first."}
            </p>
            <button className="btn-secondary text-xs py-1 px-2" onClick={() => go("assistant")}>
              <Sparkles size={11} /> Shape the persona
            </button>
          </div>
        )}
        {items.map((it, i) => {
          if (it.kind === "msg") {
            const isUser = it.role === "user";
            return (
              <div key={i} className={`flex gap-2.5 ${isUser ? "justify-end" : "justify-start"}`}>
                {!isUser && <ProfileAvatar src={avatar} name={assistantName} />}
                <div className="max-w-[75%] xl:max-w-[70%] min-w-0">
                  {showReasoning && it.reasoning && <ReasoningBlock text={it.reasoning} />}
                  <div
                    className={`rounded-2xl px-3.5 py-2.5 ${
                      isUser
                        ? "bg-accent/20 text-ink text-sm whitespace-pre-wrap break-words"
                        : "bg-surface-2 text-ink"
                    }`}
                  >
                    {isUser ? it.content : <Markdown content={it.content} />}
                  </div>
                  {it.images && it.images.length > 0 && (
                    <div className={`flex flex-wrap gap-1.5 mt-1.5 ${isUser ? "justify-end" : ""}`}>
                      {it.images.map((src, k) => (
                        <img
                          key={k}
                          src={src}
                          alt="attached image"
                          className="max-h-40 rounded border border-border object-contain"
                        />
                      ))}
                    </div>
                  )}
                  <div
                    className={`flex items-center gap-1.5 mt-1 px-1 text-[0.625rem] text-faint ${isUser ? "justify-end" : ""}`}
                  >
                    {!isUser && it.tokps != null && it.tokps > 0 && (
                      <span className="tabular-nums">{it.tokps.toFixed(1)} t/s</span>
                    )}
                    <CopyButton text={it.content} label="Copy" />
                  </div>
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
              />
            );
          }
          if (it.kind === "approval") {
            return (
              <div key={`approval-${i}`} className="flex justify-start">
                <div className="max-w-[85%] xl:max-w-[75%] rounded border border-accent-yellow/40 bg-accent-yellow/5 px-3 py-2 text-xs">
                  <p className="text-ink flex items-center gap-1.5 mb-1">
                    <FileWarning size={11} className="text-accent-yellow shrink-0" />
                    Approval requested: <span className="font-medium">{it.tool}</span>
                    {it.command && (
                      <span className="badge-gray text-[0.625rem]">{it.command}</span>
                    )}
                  </p>
                  <pre className="whitespace-pre-wrap break-words text-[0.6875rem] text-dim mb-2 max-h-40 overflow-y-auto">
                    {it.args}
                  </pre>
                  {!it.resolved ? (
                    <div className="flex flex-wrap items-center gap-2">
                      <button className="btn-secondary py-1 px-2" onClick={() => decide(i, "once")}>
                        <Check size={11} /> Allow once
                      </button>
                      <button
                        className="btn-secondary py-1 px-2"
                        onClick={() => decide(i, "session")}
                      >
                        <Check size={11} /> Allow session
                      </button>
                      <button
                        className="btn-secondary py-1 px-2"
                        onClick={() => decide(i, "project")}
                        title="Remember for this profile"
                      >
                        <Check size={11} /> Allow always
                      </button>
                      <button
                        className="btn-ghost py-1 px-2 text-accent-red"
                        onClick={() => decide(i, null)}
                      >
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
          }
          if (it.kind === "reasoning") {
            return showReasoning ? <ReasoningBlock key={`reasoning-${i}`} text={it.text} /> : null;
          }
          return <SysNotice key={`sys-${i}`} text={it.text} />;
        })}
        {showReasoning && reasoningText !== null && (
          <div className="flex gap-2.5 justify-start">
            <ProfileAvatar src={avatar} name={assistantName} />
            <div className="max-w-[75%] w-full">
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
          <div className="flex gap-2.5 justify-start">
            <ProfileAvatar src={avatar} name={assistantName} />
            <div className="max-w-[75%] rounded-2xl px-3.5 py-2.5 bg-surface-2 text-ink select-text">
              <Markdown content={streamText} />
              {streaming && (
                <span className="ml-0.5 inline-block w-2 h-4 bg-dim animate-pulse align-middle" />
              )}
            </div>
          </div>
        )}
        {(streaming || starting) &&
          streamText === null &&
          (reasoningText === null || !showReasoning) && (
            <div className="flex gap-2.5 justify-start items-center">
              <ProfileAvatar src={avatar} name={assistantName} />
              {starting || runStatus === "loading" ? (
                <RefreshCw size={13} className="animate-spin text-dim" />
              ) : (
                <Brain size={13} className="animate-pulse text-dim" />
              )}
              <span className="text-[0.6875rem] text-dim">
                {starting
                  ? runStatus === "loading"
                    ? "Loading the model…"
                    : "Starting the server…"
                  : "Thinking…"}
              </span>
            </div>
          )}
      </div>

      <div className="border-t border-border p-3 shrink-0">
        {error && (
          <div className="flex items-start gap-2 mb-2 text-xs text-accent-red">
            <span className="flex-1 min-w-0 break-words">{error}</span>
            <button className="shrink-0" onClick={() => setError(null)} title="Dismiss">
              <X size={11} />
            </button>
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
                onClick={() => answerQuestion(questionDraft)}
              >
                Answer
              </button>
            </div>
          </div>
        )}
        {attachments.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mb-2">
            {attachments.map((a, i) => (
              <span
                key={`${a.name}-${i}`}
                className="inline-flex items-center gap-1.5 badge-gray text-[0.625rem] px-1.5 py-0.5"
              >
                {a.kind === "image" && a.preview ? (
                  <img src={a.preview} alt={a.name} className="w-4 h-4 rounded object-cover" />
                ) : (
                  <Paperclip size={9} />
                )}
                <span className="max-w-[10rem] truncate">{a.name}</span>
                <button
                  onClick={() => setAttachments((prev) => prev.filter((_, k) => k !== i))}
                  title="Remove"
                >
                  <X size={9} />
                </button>
              </span>
            ))}
          </div>
        )}
        {slashMatches.length > 0 && (
          <div className="mb-2 space-y-0.5">
            {slashMatches.map((s) => (
              <button
                key={s.name}
                className="block text-left text-[0.6875rem] text-dim hover:text-ink"
                onClick={() => setInput(s.name)}
              >
                <span className="font-mono text-ink">{s.name}</span> — {s.hint}
              </button>
            ))}
          </div>
        )}
        <div className="flex items-end gap-2">
          <button
            className="btn-secondary shrink-0 py-2 px-2.5"
            onClick={attachFiles}
            disabled={streaming || starting}
            title="Attach images or text files"
          >
            <Paperclip size={13} />
          </button>
          <textarea
            className="input flex-1 resize-none text-sm"
            rows={1}
            placeholder={
              starting
                ? "Starting the server…"
                : canSend
                  ? `Message ${assistantName}…`
                  : "Set an external model first"
            }
            value={input}
            disabled={(!canSend && !input.startsWith("/")) || starting}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
          />
          {streaming ? (
            <button className="btn-secondary shrink-0 py-2 px-2.5" onClick={stop} title="Stop">
              <Square size={13} />
            </button>
          ) : starting ? (
            <button
              className="btn-secondary shrink-0 py-2 px-2.5"
              disabled
              title="Starting the server…"
            >
              <RefreshCw size={13} className="animate-spin" />
            </button>
          ) : (
            <button
              className="btn-primary shrink-0 py-2 px-2.5"
              onClick={() => void send()}
              disabled={!input.trim() || !canSend}
              title="Send"
            >
              <ArrowUp size={13} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ArrowUp,
  Brain,
  FolderOpen,
  Paperclip,
  RefreshCw,
  RotateCcw,
  Scissors,
  Sparkles,
  Square,
  Undo2,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  AssistantConfig,
  ContextStats,
  MemoryFileDto,
  Reminder,
  ServerStatus,
} from "../bindings";
import { call } from "../utils/ipc";
import { subscribeConfigChanged } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { playNotificationSound } from "../utils/sounds";
import Toggle from "../components/Toggle";
import { Markdown } from "./chat/markdown";
import { ContextRing, CopyButton, ReasoningBlock, SysNotice, ToolCard } from "./chat/components";
import { EFFORT_LABELS } from "./chat/external-controls";
import { SkillsCard } from "./agent/memory";

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
  { name: "/help", hint: "List the assistant commands" },
];

type Tab = "chat" | "persona" | "reminders" | "memory" | "behavior";

/// Assistant surface: one continuous thread with personality, memory, and
/// the remember/ask_user/get_time/skill toolset.
export default function Assistant({ active = true }: { active?: boolean }) {
  const [tab, setTab] = useState<Tab>("chat");
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
  const [assistantName, setAssistantName] = useState("Werk");
  const [externalMode, setExternalMode] = useState(false);
  const [externalTarget, setExternalTarget] = useState("");
  const [persona, setPersona] = useState<AssistantConfig | null>(null);
  const [personaError, setPersonaError] = useState<string | null>(null);
  const [personaSaved, setPersonaSaved] = useState(false);
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

  const refreshConfig = () => {
    call(commands.getConfig())
      .then(applyConfig)
      .catch(() => {});
  };

  const applyConfig = (c: AppConfig) => {
    const external = c.server_mode === "external";
    setExternalMode(external);
    setExternalTarget(external ? (c.external_target ?? "").trim() : "");
    setAssistantName(c.assistant?.name?.trim() || "Werk");
    setPersona(c.assistant ?? null);
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
    } catch {}
  };

  // Mount: config, transcript, context stats, and the live stream.
  useEffect(() => {
    refreshConfig();
    void restore();
    const poll = () => {
      if (!activeRef.current) return;
      call(commands.assistantContextStats()).then(setSlotCtx).catch(() => {});
    };
    poll();
    const id = setInterval(poll, 2000);
    const unsubStatus = subscribeServerStatus(setStatus);
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
    if (active) {
      void restore();
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
    if (!(await ensureServerReady())) return;
    setStreaming(true);
    setStreamText("");
    setReasoningText(null);
    setReasoningOpen(false);
    setReasoningLive(false);
    setRunStatus("thinking");
    setError(null);
    accRef.current = "";
    reasoningAccRef.current = "";

    try {
      const res = await call(
        commands.assistantSend(
          text,
          null,
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
          promptTokens: res.prompt_tokens ?? undefined,
          reasoning: res.reasoning ?? (reasoningAccRef.current || undefined),
        },
      ]);
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
      setRunStatus(null);
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

  const savePersona = async () => {
    if (!persona) return;
    setPersonaError(null);
    try {
      await call(commands.setAssistantConfig(persona));
      setPersonaSaved(true);
      setTimeout(() => setPersonaSaved(false), 1500);
      refreshConfig();
    } catch (e) {
      setPersonaError(String(e));
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

  const TABS: { id: Tab; label: string }[] = [
    { id: "chat", label: "Chat" },
    { id: "persona", label: "Persona" },
    { id: "reminders", label: "Reminders" },
    { id: "memory", label: "Memory" },
    { id: "behavior", label: "Behavior" },
  ];

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-0.5 px-4 pt-3 shrink-0">
        {TABS.map((t) => (
          <button
            key={t.id}
            onClick={() => setTab(t.id)}
            className={`px-2.5 py-1 rounded text-xs font-medium transition-colors ${
              tab === t.id ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "chat" && (
        <>
          <div className="flex items-center gap-3 px-4 py-2 border-b border-border shrink-0">
            <Avatar name={assistantName} size={36} />
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
            </div>
          </div>

          <div ref={scrollRef} className="flex-1 overflow-y-auto px-6 py-4 space-y-3 select-text">
            {items.length === 0 && streamText === null && (
              <div className="h-full flex flex-col items-center justify-center text-center gap-2">
                <Sparkles size={20} className="text-faint" />
                <p className="text-sm text-dim">
                  {canSend ? `Say something to ${assistantName}.` : "Set an external model first."}
                </p>
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => setTab("persona")}
                >
                  <Sparkles size={11} /> Shape the persona
                </button>
              </div>
            )}
            {items.map((it, i) => {
              if (it.kind === "msg") {
                const isUser = it.role === "user";
                return (
                  <div key={i} className={`flex gap-2.5 ${isUser ? "justify-end" : "justify-start"}`}>
                    {!isUser && <Avatar name={assistantName} />}
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
              if (it.kind === "reasoning") {
                return showReasoning ? <ReasoningBlock key={`reasoning-${i}`} text={it.text} /> : null;
              }
              return <SysNotice key={`sys-${i}`} text={it.text} />;
            })}
            {showReasoning && reasoningText !== null && (
              <div className="flex gap-2.5 justify-start">
                <Avatar name={assistantName} />
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
                <Avatar name={assistantName} />
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
                  <Avatar name={assistantName} />
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
        </>
      )}

      {tab === "persona" && persona && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <div className="card space-y-3">
              <div>
                <h2 className="section-title mb-0">Identity</h2>
                <p className="section-desc">
                  The assistant's name, personality, and optional core prompt override.
                </p>
              </div>
              <label className="block">
                <span className="text-[0.6875rem] text-dim">Name</span>
                <input
                  className="input w-full mt-1"
                  value={persona.name}
                  onChange={(e) => setPersona({ ...persona, name: e.target.value })}
                />
              </label>
              <label className="block">
                <span className="text-[0.6875rem] text-dim">Personality</span>
                <textarea
                  className="input w-full mt-1 text-xs"
                  rows={4}
                  placeholder="Voice, temperament, quirks, what it cares about…"
                  value={persona.persona}
                  onChange={(e) => setPersona({ ...persona, persona: e.target.value })}
                />
              </label>
              <label className="block">
                <span className="text-[0.6875rem] text-dim">
                  System prompt override (empty = built-in assistant prompt)
                </span>
                <textarea
                  className="input w-full mt-1 text-xs font-mono"
                  rows={6}
                  placeholder="Leave empty to use the built-in prompt. {{name}} is replaced with the assistant name."
                  value={persona.system_prompt ?? ""}
                  onChange={(e) =>
                    setPersona({ ...persona, system_prompt: e.target.value || null })
                  }
                />
              </label>
            </div>
            <div className="card space-y-3">
              <div>
                <h2 className="section-title mb-0">Generation</h2>
                <p className="section-desc">Per-request overrides; empty uses the server default.</p>
              </div>
              <div className="grid grid-cols-3 gap-3">
                <label className="block">
                  <span className="text-[0.6875rem] text-dim">Temperature</span>
                  <input
                    type="number"
                    step={0.05}
                    min={0}
                    max={2}
                    placeholder="default"
                    className="input w-full mt-1"
                    value={persona.temperature ?? ""}
                    onChange={(e) =>
                      setPersona({
                        ...persona,
                        temperature: e.target.value === "" ? null : Number(e.target.value),
                      })
                    }
                  />
                </label>
                <label className="block">
                  <span className="text-[0.6875rem] text-dim">Top P</span>
                  <input
                    type="number"
                    step={0.01}
                    min={0}
                    max={1}
                    placeholder="default"
                    className="input w-full mt-1"
                    value={persona.top_p ?? ""}
                    onChange={(e) =>
                      setPersona({
                        ...persona,
                        top_p: e.target.value === "" ? null : Number(e.target.value),
                      })
                    }
                  />
                </label>
                <label className="block">
                  <span className="text-[0.6875rem] text-dim">Repeat penalty</span>
                  <input
                    type="number"
                    step={0.01}
                    min={0}
                    max={2}
                    placeholder="default"
                    className="input w-full mt-1"
                    value={persona.repeat_penalty ?? ""}
                    onChange={(e) =>
                      setPersona({
                        ...persona,
                        repeat_penalty: e.target.value === "" ? null : Number(e.target.value),
                      })
                    }
                  />
                </label>
              </div>
              <label className="block">
                <span className="text-[0.6875rem] text-dim">Reasoning effort</span>
                <select
                  className="input w-full mt-1"
                  value={persona.reasoning_effort ?? ""}
                  onChange={(e) =>
                    setPersona({ ...persona, reasoning_effort: e.target.value || null })
                  }
                >
                  <option value="">Default</option>
                  {Object.entries(EFFORT_LABELS).map(([level, label]) => (
                    <option key={level} value={level}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            <div className="flex items-center justify-end gap-2">
              {personaError && <p className="text-xs text-accent-red mr-auto">{personaError}</p>}
              <button className="btn-primary text-xs py-1 px-2" onClick={savePersona}>
                {personaSaved ? "Saved" : "Save"}
              </button>
            </div>
          </div>
        </div>
      )}

      {tab === "memory" && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <AssistantMemoryCard />
            <SkillsCard />
          </div>
        </div>
      )}

      {tab === "reminders" && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <RemindersCard />
          </div>
        </div>
      )}

      {tab === "behavior" && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <BehaviorCard />
          </div>
        </div>
      )}
    </div>
  );
}

/// Assistant memory editor: the file the remember tool and distill write.
function AssistantMemoryCard() {
  const [file, setFile] = useState<MemoryFileDto | null>(null);
  const [draft, setDraft] = useState("");
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    call(commands.harnessMemoryGet("assistant"))
      .then((f) => {
        setFile(f);
        setDraft(f.text);
        setDirty(false);
      })
      .catch(() => {});
  };
  useEffect(load, []);

  const save = async () => {
    setError(null);
    try {
      await call(commands.harnessMemorySet("assistant", draft));
      setDirty(false);
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="card">
      <div className="flex items-baseline justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Memory</h2>
        <div className="flex items-center gap-1 shrink-0">
          {file?.path && (
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
              title="Reveal the memory file"
              onClick={() => {
                setError(null);
                call(commands.harnessMemoryEnsure("assistant"))
                  .then((path) => revealItemInDir(path))
                  .catch((e) => setError(String(e)));
              }}
            >
              <FolderOpen size={11} />
            </button>
          )}
          <button className="btn-primary text-[0.625rem] py-0.5 px-2" disabled={!dirty} onClick={save}>
            Save
          </button>
        </div>
      </div>
      <p className="section-desc">
        What the assistant remembers about you across conversations; the remember tool and distill
        write here.
      </p>
      <textarea
        className="input w-full mt-2 font-mono text-[0.6875rem] leading-snug"
        rows={10}
        placeholder="Empty — facts the assistant saves land here."
        value={draft}
        onChange={(e) => {
          setDraft(e.target.value);
          setDirty(true);
        }}
      />
      {error && <p className="text-xs text-accent-red mt-1">{error}</p>}
    </div>
  );
}

/// Reminder list + quick add; the assistant manages the same store.
function RemindersCard() {
  const [reminders, setReminders] = useState<Reminder[] | null>(null);
  const [text, setText] = useState("");
  const [due, setDue] = useState("");
  const [repeatMin, setRepeatMin] = useState("");
  const [kind, setKind] = useState<"notify" | "message">("message");
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    call(commands.assistantRemindersList())
      .then(setReminders)
      .catch(() => setReminders([]));
  };
  useEffect(load, []);

  const add = async () => {
    setError(null);
    const trimmed = text.trim();
    if (!trimmed) return;
    const dueSecs = due
      ? Math.floor(new Date(due).getTime() / 1000)
      : Math.floor(Date.now() / 1000) + 60;
    const repeatSecs =
      repeatMin.trim() === "" ? null : Math.max(60, Math.round(Number(repeatMin) * 60));
    try {
      await call(commands.assistantReminderAdd(trimmed, dueSecs, repeatSecs, kind));
      setText("");
      setDue("");
      setRepeatMin("");
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  const complete = (id: string) =>
    call(commands.assistantReminderComplete(id)).then(load).catch((e) => setError(String(e)));
  const remove = (id: string) =>
    call(commands.assistantReminderRemove(id)).then(load).catch((e) => setError(String(e)));

  return (
    <div className="card">
      <div className="flex items-baseline justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Reminders</h2>
        <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={load} title="Refresh">
          <RefreshCw size={11} /> Refresh
        </button>
      </div>
      <p className="section-desc">
        Due reminders flash the overlay (or the taskbar/dock when it is hidden); "message"
        reminders also start a proactive turn. The assistant manages the same list with its
        reminder tool.
      </p>
      <div className="flex flex-wrap items-center gap-2 mt-3">
        <input
          className="input flex-1 min-w-[12rem] text-xs"
          placeholder="Remind me to…"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void add();
          }}
        />
        <input
          type="datetime-local"
          className="input text-xs"
          value={due}
          onChange={(e) => setDue(e.target.value)}
          title="When; empty = in one minute"
        />
        <input
          type="number"
          min={1}
          className="input w-24 text-xs"
          placeholder="repeat m"
          value={repeatMin}
          onChange={(e) => setRepeatMin(e.target.value)}
          title="Repeat every N minutes; empty = once"
        />
        <select
          className="input text-xs"
          value={kind}
          onChange={(e) => setKind(e.target.value as "notify" | "message")}
        >
          <option value="message">message</option>
          <option value="notify">notify</option>
        </select>
        <button className="btn-primary text-xs py-1 px-2" onClick={add} disabled={!text.trim()}>
          Add
        </button>
      </div>
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
      {reminders && reminders.length === 0 && (
        <p className="text-[0.6875rem] text-dim mt-2">No reminders yet.</p>
      )}
      <div className="space-y-1.5 mt-3">
        {reminders?.map((r) => (
          <div
            key={r.id}
            className={`flex items-center gap-2 border border-border rounded px-2.5 py-1.5 ${
              r.done ? "opacity-50" : ""
            }`}
          >
            <span
              className={`badge-${r.kind === "message" ? "purple" : "gray"} text-[0.5625rem] shrink-0`}
            >
              {r.kind}
            </span>
            <div className="flex-1 min-w-0">
              <p className="text-xs text-ink truncate">{r.text}</p>
              <p className="text-[0.625rem] text-faint">
                {new Date(r.due * 1000).toLocaleString()}
                {r.repeat_secs ? ` · every ${Math.round(r.repeat_secs / 60)} min` : ""}
                {r.done ? " · done" : ""}
              </p>
            </div>
            {!r.done && (
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
                onClick={() => void complete(r.id)}
                title="Mark done"
              >
                Done
              </button>
            )}
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0 text-accent-red"
              onClick={() => void remove(r.id)}
              title="Delete"
            >
              Delete
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

/// Always-on behavior: notifications, proactive turns, autostart, hotkey.
function BehaviorCard() {
  const [behavior, setBehavior] = useState<{
    notify: boolean;
    proactive: boolean;
    autostart: boolean;
    minimize_on_start: boolean;
    hotkey: string;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const load = () => {
    call(commands.getConfig())
      .then((c) =>
        setBehavior({
          notify: c.assistant?.notify ?? true,
          proactive: c.assistant?.proactive ?? true,
          autostart: c.assistant?.autostart ?? false,
          minimize_on_start: c.assistant?.minimize_on_start ?? true,
          hotkey: c.assistant?.hotkey ?? "",
        }),
      )
      .catch(() => {});
  };
  useEffect(load, []);

  const save = async () => {
    if (!behavior) return;
    setError(null);
    try {
      await call(
        commands.setAssistantBehavior(
          behavior.notify,
          behavior.proactive,
          behavior.autostart,
          behavior.minimize_on_start,
          behavior.hotkey,
        ),
      );
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  if (!behavior) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Behavior</h2>
        <p className="section-desc">How the assistant behaves when the window is closed or idle.</p>
      </div>
      <Toggle
        label="Reminder alerts"
        hint="Flash the overlay (or taskbar/dock) when a reminder fires."
        checked={behavior.notify}
        onChange={(v) => setBehavior({ ...behavior, notify: v })}
      />
      <Toggle
        label="Proactive messages"
        hint='"message" reminders also start an assistant turn when they fire.'
        checked={behavior.proactive}
        onChange={(v) => setBehavior({ ...behavior, proactive: v })}
      />
      <Toggle
        label="Start with the system"
        hint="Launch werk at login, minimized to the tray."
        checked={behavior.autostart}
        onChange={(v) => setBehavior({ ...behavior, autostart: v })}
      />
      <Toggle
        label="Minimize to tray when running"
        hint="Hide the window once the assistant's server is up."
        checked={behavior.minimize_on_start}
        onChange={(v) => setBehavior({ ...behavior, minimize_on_start: v })}
      />
      <label className="block">
        <span className="text-[0.6875rem] text-dim">Global hotkey (empty disables)</span>
        <input
          className="input w-full mt-1 font-mono text-xs"
          placeholder="Ctrl+Alt+Space"
          value={behavior.hotkey}
          onChange={(e) => setBehavior({ ...behavior, hotkey: e.target.value })}
        />
      </label>
      <div className="flex items-center justify-end gap-2">
        {error && <p className="text-xs text-accent-red mr-auto">{error}</p>}
        <button className="btn-primary text-xs py-1 px-2" onClick={save}>
          {saved ? "Saved" : "Save"}
        </button>
      </div>
    </div>
  );
}

function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  const cls = "rounded-full border border-border shrink-0";
  const initials = name.trim().slice(0, 1).toUpperCase() || "?";
  return (
    <div
      className={`${cls} bg-surface-2 flex items-center justify-center text-[0.625rem] text-dim`}
      style={{ width: size, height: size }}
    >
      {initials}
    </div>
  );
}

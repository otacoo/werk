import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArrowUp,
  FolderOpen,
  Paperclip,
  RefreshCw,
  RotateCcw,
  Scissors,
  Sparkles,
  Square,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type { AppConfig, ContextStats, ServerStatus } from "../bindings";
import { call } from "../utils/ipc";
import { subscribeConfigChanged } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { playNotificationSound } from "../utils/sounds";
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

interface TalkAttachment {
  name: string;
  kind: "image" | "text";
  path: string;
  preview?: string;
  text?: string;
}

const SLASH = [
  { name: "/compact", hint: "Summarize older turns to free context" },
  { name: "/distill", hint: "Save the story into the character's memory, then start fresh" },
  { name: "/new", hint: "Start over with the greeting" },
  { name: "/help", hint: "List the talk commands" },
];

/// Roleplay chat surface: one continuous discussion per character, with
/// avatars and no agent tooling beyond remember.
export default function Talk({ go, active = true }: { go: (t: Tab) => void; active?: boolean }) {
  const [status, setStatus] = useState<ServerStatus>(getServerStatus());
  const [items, setItems] = useState<Item[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [streamText, setStreamText] = useState<string | null>(null);
  const [reasoningText, setReasoningText] = useState<string | null>(null);
  const [reasoningOpen, setReasoningOpen] = useState(false);
  const [reasoningLive, setReasoningLive] = useState(false);
  const [attachments, setAttachments] = useState<TalkAttachment[]>([]);
  const [contextUsed, setContextUsed] = useState<number | null>(null);
  const [slotCtx, setSlotCtx] = useState<ContextStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [runStatus, setRunStatus] = useState<"loading" | "thinking" | null>(null);
  const [pendingQuestion, setPendingQuestion] = useState<{ question: string; options: string[] } | null>(null);
  const [questionDraft, setQuestionDraft] = useState("");
  const [card, setCard] = useState<{ name: string; avatar: string | null } | null>(null);
  const [userAvatar, setUserAvatar] = useState<string | null>(null);
  const [userName, setUserName] = useState("User");
  const [activeProject, setActiveProject] = useState<string | null>(null);
  const [autoStart, setAutoStart] = useState(false);
  const [externalMode, setExternalMode] = useState(false);
  const [externalTarget, setExternalTarget] = useState("");

  const scrollRef = useRef<HTMLDivElement>(null);
  const accRef = useRef("");
  const reasoningAccRef = useRef("");
  const loadedCardRef = useRef<string | null | undefined>(undefined);
  const activeRef = useRef(active);
  const streamingRef = useRef(streaming);

  const refreshConfig = () => {
    call(commands.getConfig())
      .then(applyConfig)
      .catch(() => {});
  };

  const applyConfig = (c: AppConfig) => {
    setActiveProject(c.harness_active_project ?? null);
    setAutoStart(c.server_auto_start ?? false);
    const external = c.server_mode === "external";
    setExternalMode(external);
    setExternalTarget(external ? (c.external_target ?? "").trim() : "");
    setUserName(c.roleplay?.user_name?.trim() || "User");
    const cardId = c.roleplay?.card_id ?? null;
    if (cardId !== loadedCardRef.current) {
      loadedCardRef.current = cardId;
      if (cardId) {
        call(commands.roleplayGetCard(cardId))
          .then((k) => setCard({ name: k.name, avatar: null }))
          .catch(() => setCard(null));
        call(commands.roleplayCardAvatar(cardId))
          .then((a) => setCard((prev) => (prev ? { ...prev, avatar: a } : prev)))
          .catch(() => {});
      } else {
        setCard(null);
      }
      void restore();
    }
  };

  const loadUserAvatar = () => {
    call(commands.roleplayUserAvatar())
      .then(setUserAvatar)
      .catch(() => setUserAvatar(null));
  };

  const restore = async () => {
    if (streamingRef.current) return;
    try {
      const res = await call(commands.talkHistory());
      const metaByIndex = new Map((res.meta ?? []).map((m) => [m.index, m.meta]));
      const restored: Item[] = [];
      res.messages.forEach((m, i) => {
        if (m.role === "user" && (m.content || (m.images && m.images.length > 0))) {
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
          });
        }
      });
      setItems(restored);
    } catch {}
  };

  // Mount: config, avatars, transcript, and the live stream.
  useEffect(() => {
    refreshConfig();
    loadUserAvatar();
    void restore();
    const poll = () => {
      if (!activeRef.current) return;
      call(commands.talkContextStats()).then(setSlotCtx).catch(() => {});
    };
    poll();
    const id = setInterval(poll, 2000);
    const unsubStatus = subscribeServerStatus(setStatus);
    const unsubConfig = subscribeConfigChanged(() => {
      refreshConfig();
      loadUserAvatar();
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
      call(commands.talkContextStats()).then(setSlotCtx).catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  useEffect(() => {
    streamingRef.current = streaming;
  }, [streaming]);

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

  // Live transcript over the talk bus.
  useEffect(() => {
    const unlisten = listen<Record<string, unknown>>("talk_event", (event) => {
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
    const unlisten = listen<{ question: string; options: string[] }>("talk_question", (event) => {
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

  const serverRunning = status.type === "running";
  const canSend =
    (serverRunning || (externalMode && externalTarget !== "") || (autoStart && !externalMode)) &&
    activeProject != null;

  const ensureServerReady = async () => {
    if (externalMode) return true;
    try {
      await call(commands.ensureServer());
      return true;
    } catch (e) {
      setError(`Could not start the server: ${e}`);
      return false;
    }
  };

  const compact = async () => {
    if (streaming) return;
    setError(null);
    try {
      await call(commands.talkCompact());
    } catch (e) {
      setError(String(e));
    }
  };

  const distill = async () => {
    if (streaming) return;
    setError(null);
    try {
      await call(commands.talkDistill());
    } catch (e) {
      setError(String(e));
    }
  };

  const restart = async () => {
    if (streaming) return;
    setError(null);
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.roleplayStartChat());
      await restore();
    } catch (e) {
      setError(String(e));
    }
  };

  const send = async () => {
    const text = input.trim();
    if (!text || streaming) return;
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
    if (text === "/new") {
      setInput("");
      await restart();
      return;
    }
    if (!canSend) return;
    if (!(await ensureServerReady())) return;

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
        commands.talkSend(
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
      await call(commands.talkAbort());
    } catch {}
  };

  const answerQuestion = async (answer: string) => {
    const text = answer.trim();
    if (!text || !pendingQuestion) return;
    setPendingQuestion(null);
    setQuestionDraft("");
    try {
      await call(commands.talkQuestionAnswer(text));
    } catch {}
  };

  const pickProject = async () => {
    const picked = await openDialog({
      directory: true,
      title: "Select the roleplay working directory",
    });
    if (!picked || typeof picked !== "string") return;
    try {
      const project = await call(commands.harnessProjectAdd(picked));
      await call(commands.harnessProjectSetActive(project.id));
      setActiveProject(project.id);
    } catch (e) {
      setError(String(e));
    }
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

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-3 px-4 py-2 border-b border-border shrink-0">
        <Avatar src={card?.avatar ?? null} name={card?.name ?? "?"} size={36} />
        <div className="min-w-0">
          <p className="text-sm font-medium text-ink truncate">{card?.name ?? "No character"}</p>
          <p className="text-[0.625rem] text-faint truncate">
            {runStatus === "loading"
              ? "Loading model…"
              : runStatus === "thinking"
                ? "Thinking…"
                : "One continuous discussion"}
          </p>
        </div>
        <div className="ml-auto flex items-center gap-1.5 shrink-0">
          <ContextRing
            used={slotCtx?.used ?? contextUsed}
            total={slotCtx?.total ?? null}
            avgTokps={null}
            dropDown
          />
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={compact}
            disabled={streaming}
            title="Summarize older turns (also /compact)"
          >
            <Scissors size={11} /> Compact
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={distill}
            disabled={streaming}
            title="Save the story to the character's memory and start fresh (also /distill)"
          >
            <Sparkles size={11} /> Distill
          </button>
          <button
            className="btn-ghost py-1 px-2 text-[0.625rem]"
            onClick={restart}
            disabled={streaming}
            title="Start over with the selected greeting (also /new)"
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
              {card ? `Say something to ${card.name}.` : "Pick a character on the Roleplay page."}
            </p>
            {!card && (
              <button className="btn-secondary text-xs py-1 px-2" onClick={() => go("roleplay")}>
                <Sparkles size={11} /> Open Roleplay
              </button>
            )}
            {!activeProject && (
              <button className="btn-secondary text-xs py-1 px-2" onClick={pickProject}>
                <FolderOpen size={11} /> Select working directory
              </button>
            )}
          </div>
        )}
        {items.map((it, i) => {
          if (it.kind === "msg") {
            const isUser = it.role === "user";
            return (
              <div key={i} className={`flex gap-2.5 ${isUser ? "justify-end" : "justify-start"}`}>
                {!isUser && <Avatar src={card?.avatar ?? null} name={card?.name ?? "?"} />}
                <div className="max-w-[75%] xl:max-w-[70%] min-w-0">
                  {it.reasoning && <ReasoningBlock text={it.reasoning} />}
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
                  <div className={`flex items-center gap-1.5 mt-1 px-1 text-[0.625rem] text-faint ${isUser ? "justify-end" : ""}`}>
                    {!isUser && it.tokps != null && it.tokps > 0 && (
                      <span className="tabular-nums">{it.tokps.toFixed(1)} t/s</span>
                    )}
                    <CopyButton text={it.content} label="Copy" />
                  </div>
                </div>
                {isUser && <Avatar src={userAvatar} name={userName} />}
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
            return <ReasoningBlock key={`reasoning-${i}`} text={it.text} />;
          }
          return <SysNotice key={`sys-${i}`} text={it.text} />;
        })}
        {reasoningText !== null && (
          <div className="flex gap-2.5 justify-start">
            <Avatar src={card?.avatar ?? null} name={card?.name ?? "?"} />
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
            <Avatar src={card?.avatar ?? null} name={card?.name ?? "?"} />
            <div className="max-w-[75%] rounded-2xl px-3.5 py-2.5 bg-surface-2 text-ink select-text">
              <Markdown content={streamText} />
              {streaming && (
                <span className="ml-0.5 inline-block w-2 h-4 bg-dim animate-pulse align-middle" />
              )}
            </div>
          </div>
        )}
        {streaming && streamText === null && reasoningText === null && (
          <div className="flex gap-2.5 justify-start">
            <Avatar src={card?.avatar ?? null} name={card?.name ?? "?"} />
            <RefreshCw size={13} className="animate-spin text-dim mt-2" />
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
              {pendingQuestion.options.map((o) => (
                <button
                  key={o}
                  className="btn-secondary py-1 px-2 text-xs text-left"
                  onClick={() => answerQuestion(o)}
                >
                  {o}
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
            disabled={streaming}
            title="Attach images or text files"
          >
            <Paperclip size={13} />
          </button>
          <textarea
            className="input flex-1 resize-none text-sm"
            rows={1}
            placeholder={canSend ? `Reply as ${userName}…` : "Start the server or pick a project first"}
            value={input}
            disabled={!canSend && !input.startsWith("/")}
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

function Avatar({ src, name, size = 32 }: { src: string | null; name: string; size?: number }) {
  const cls = "rounded-full border border-border shrink-0";
  if (src) {
    return (
      <img
        src={src}
        alt={name}
        className={`${cls} object-cover`}
        style={{ width: size, height: size }}
      />
    );
  }
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

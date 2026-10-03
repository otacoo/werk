import { Fragment, memo, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Brain, Check, ChevronDown, Copy, ListChecks, Trash2, Wrench, X } from "lucide-react";
import type { RunChange, TodoDto } from "../../bindings";
import { formatElapsed } from "../../utils/format";
import { getBubbleAlign, getShowToolSnippets, subscribeBubbleAlign, subscribeShowToolSnippets } from "../../utils/appearance";

// ── System notice (compaction cards, restored summaries) ────────────────────
// Renders as a 1px divider labeled "Compaction" with the details below;
// long summaries stay expandable.

export function SysNotice({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const nl = text.indexOf("\n");
  const rawHead = nl < 0 ? text : text.slice(0, nl);
  const body = nl < 0 ? "" : text.slice(nl + 1).trim();
  // Restored summaries arrive as "[Compacted context — …]"; show the head
  // without the marker brackets.
  const head = rawHead.replace(/^\[/, "").replace(/\]$/, "");
  return (
    <div>
      <div className="flex items-center gap-2" aria-hidden="true">
        <span className="flex-1 h-px bg-border" />
        <span className="text-[0.625rem] uppercase tracking-widest text-faint select-none">Compaction</span>
        <span className="flex-1 h-px bg-border" />
      </div>
      <div className="flex justify-center mt-1">
        <div className="max-w-[85%] xl:max-w-[75%] text-center">
          <button
            className="text-[0.6875rem] text-dim hover:text-ink transition-colors select-text"
            onClick={() => body && setOpen((v) => !v)}
            title={body ? (open ? "Hide" : "Show") : undefined}
          >
            {head}
            {body && (
              <ChevronDown size={10} className={`inline ml-1 transition-transform ${open ? "rotate-180" : ""}`} />
            )}
          </button>
          {open && body && (
            <pre className="whitespace-pre-wrap break-words text-left text-[0.6875rem] text-dim mt-1.5 max-h-56 overflow-y-auto select-text">
              {body}
            </pre>
          )}
        </div>
      </div>
    </div>
  );
}


// ── Copy button with "Copied" feedback (user messages, errors) ─────────────

export function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="inline-flex items-center gap-1 hover:text-ink transition-colors"
      onClick={() => {
        navigator.clipboard.writeText(text).catch(() => {});
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
      title={`Copy ${label.toLowerCase()}`}
    >
      {copied ? <Check size={10} /> : <Copy size={10} />}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

// ── Response footer (model, tok/s, time, tokens, copy, delete) ──────────────

const CHANGE_STATUS: Record<string, { letter: string; cls: string }> = {
  added: { letter: "A", cls: "text-accent-green" },
  modified: { letter: "M", cls: "text-accent-yellow" },
  deleted: { letter: "D", cls: "text-accent-red" },
};

export function ChangesCard({
  files,
  added,
  removed,
}: {
  files: RunChange[];
  added: number;
  removed: number;
}) {
  const [expanded, setExpanded] = useState(false);
  const visible = expanded ? files : files.slice(0, 5);
  const hidden = files.length - visible.length;
  return (
    <div className="flex justify-start">
      <div className="max-w-[85%] xl:max-w-[75%] rounded border border-border bg-surface-2 px-3 py-2 text-xs space-y-1.5">
        <p className="text-ink font-medium">
          {files.length} file{files.length === 1 ? "" : "s"} changed
          {added > 0 && <span className="text-accent-green ml-2 tabular-nums">+{added}</span>}
          {removed > 0 && <span className="text-accent-red ml-1 tabular-nums">−{removed}</span>}
        </p>
        <div className="space-y-0.5">
          {visible.map((f) => {
            const status = CHANGE_STATUS[f.status] ?? CHANGE_STATUS.modified;
            return (
              <div key={f.path} className="flex items-center gap-2 font-mono text-[0.6875rem]">
                <span className={`${status.cls} shrink-0`} title={f.status}>
                  {status.letter}
                </span>
                <span className="flex-1 min-w-0 truncate text-dim" title={f.path}>
                  {f.path}
                </span>
                {f.added > 0 && (
                  <span className="text-accent-green tabular-nums shrink-0">+{f.added}</span>
                )}
                {f.removed > 0 && (
                  <span className="text-accent-red tabular-nums shrink-0">−{f.removed}</span>
                )}
              </div>
            );
          })}
        </div>
        {hidden > 0 && (
          <button
            className="text-accent hover:underline text-[0.6875rem]"
            onClick={() => setExpanded(true)}
          >
            Show {hidden} more
          </button>
        )}
        {expanded && files.length > 5 && (
          <button
            className="text-dim hover:text-ink text-[0.6875rem]"
            onClick={() => setExpanded(false)}
          >
            Show less
          </button>
        )}
      </div>
    </div>
  );
}

export function ResponseFooter({ model, tokps, elapsedMs, tokens, onCopy, onDelete }: {
  model?: string;
  tokps?: number | null;
  elapsedMs?: number;
  tokens?: number;
  onCopy: () => void;
  onDelete?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const parts: { key: string; node: ReactNode }[] = [];
  if (model) parts.push({ key: "model", node: <span className="font-mono truncate max-w-[200px]">{model.split(/[\\/]/).pop()}</span> });
  if (tokps != null && tokps > 0) parts.push({ key: "tokps", node: <span className="tabular-nums">{tokps.toFixed(1)} t/s</span> });
  if (tokens != null && tokens > 0) parts.push({ key: "tokens", node: <span className="tabular-nums">{tokens} tok</span> });
  if (elapsedMs != null && elapsedMs > 0) {
    parts.push({
      key: "elapsed",
      node: <span className="tabular-nums" title={`${Math.round(elapsedMs)}ms run time`}>{formatElapsed(elapsedMs)}</span>,
    });
  }
  return (
    <div className="flex items-center gap-1.5 mt-1 px-1 text-[0.625rem] text-faint">
      {parts.map((p, i) => (
        <Fragment key={p.key}>
          {i > 0 && <span className="text-faint select-none">·</span>}
          {p.node}
        </Fragment>
      ))}
      <button
        className="ml-auto inline-flex items-center gap-1 hover:text-ink transition-colors"
        onClick={() => {
          onCopy();
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
        title="Copy response"
      >
        {copied ? <Check size={10} /> : <Copy size={10} />}
        {copied ? "Copied" : "Copy"}
      </button>
      {onDelete && (
        <button className="inline-flex items-center gap-1 hover:text-accent-red transition-colors" onClick={onDelete} title="Delete this response and rewind to your message">
          <Trash2 size={10} />
          Delete
        </button>
      )}
    </div>
  );
}

// ── Edit diff stat: leading "(+N -M)" of edit_file output, colored ─────────

function DiffStat({ text }: { text: string }) {
  const m = text.match(/\(\+(\d+) -(\d+)\)/);
  if (!m) return null;
  return (
    <span className="shrink-0 text-[0.625rem] font-mono">
      <span className="text-accent-green">+{m[1]}</span>{" "}
      <span className="text-accent-red">-{m[2]}</span>
    </span>
  );
}

/** Short display label for a model id (router stem, provider id, or path). */
export function shortModelLabel(id: string): string {
  const leaf = id.split(/[\\/]/).pop() ?? id;
  const stem = leaf.replace(/\.gguf$/i, "");
  return stem.length > 20 ? `${stem.slice(0, 19)}…` : stem;
}

// ── Tool call card: collapsed by default so long outputs (PowerShell error
// walls included) don't flood the transcript. Chevron expands args + output.
// Subagent calls carry the model that ran them as a badge.
export const ToolCard = memo(function ToolCard({ tool, args, output, subagentModel }: {
  tool: string;
  args: string;
  output?: { ok: boolean; text: string };
  subagentModel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [showSnippets, setShowSnippets] = useState(getShowToolSnippets());
  useEffect(() => subscribeShowToolSnippets(setShowSnippets), []);
  const [align, setAlign] = useState(getBubbleAlign());
  useEffect(() => subscribeBubbleAlign(setAlign), []);
  return (
    <div className={`flex items-center gap-2 ${align === "right" ? "justify-start" : "justify-end"}`}>
      <div className="max-w-[85%] xl:max-w-[75%] rounded border border-border bg-surface-2 px-3 py-2 text-xs">
        <button
          className="w-full text-dim flex items-center gap-1.5 text-left"
          onClick={() => setOpen((v) => !v)}
          title={open ? "Collapse tool call" : "Expand tool call"}
        >
          <Wrench size={11} className="text-faint shrink-0" />
          <span className="text-ink font-medium">{tool}</span>
          {subagentModel && (
            <span
              className="badge-gray text-[0.625rem] shrink-0 max-w-[9rem] truncate"
              title={`Ran on ${subagentModel} (subagent)`}
            >
              {shortModelLabel(subagentModel)}
            </span>
          )}
          {showSnippets && <span className="text-faint truncate">{args}</span>}
          {output && (
            <span className={`shrink-0 text-[0.625rem] ${output.ok ? "text-accent-green" : "text-accent-yellow"}`}>
              {output.ok ? "✓" : "!"}
            </span>
          )}
          <span className={`ml-auto shrink-0 transition-transform ${open ? "rotate-180" : ""}`}>
            <ChevronDown size={12} />
          </span>
        </button>
        {open && (
          <>
            <pre className="whitespace-pre-wrap break-words text-[0.6875rem] leading-snug mt-1.5 max-h-40 overflow-y-auto text-dim select-text">
              {args || "(no arguments)"}
            </pre>
            {output && (
              <pre
                className={`whitespace-pre-wrap break-words text-[0.6875rem] leading-snug max-h-40 overflow-y-auto mt-1 ${
                  output.ok ? "text-dim" : "text-accent-yellow"
                }`}
              >
                {output.text}
              </pre>
            )}
          </>
        )}
      </div>
      {output && <DiffStat text={output.text} />}
    </div>
  );
});

// ── Task list strip: the agent's checkpoints above the composer ────────────

export function TodoStrip({ todos, onClear }: { todos: TodoDto[]; onClear: () => void }) {
  const [open, setOpen] = useState(false);
  const done = todos.filter((t) => t.done).length;
  const current = todos.find((t) => !t.done);
  return (
    <div className="rounded border border-border bg-surface-2 px-2 py-1 text-xs">
      <div className="flex items-center gap-1.5">
        <button
          className="flex-1 min-w-0 flex items-center gap-1.5 text-left text-dim hover:text-ink transition-colors"
          onClick={() => setOpen((v) => !v)}
          title={open ? "Collapse task list" : "Expand task list"}
        >
          <ListChecks size={12} className="shrink-0 text-accent-soft" />
          <span className="text-ink font-medium shrink-0">
            {done}/{todos.length}
          </span>
          {current ? <span className="truncate">· {current.text}</span> : <span className="text-faint">all done</span>}
          <ChevronDown
            size={11}
            className={`ml-auto shrink-0 text-faint transition-transform ${open ? "rotate-180" : ""}`}
          />
        </button>
        <button
          className="text-faint hover:text-ink shrink-0"
          onClick={onClear}
          title="Clear the task list"
        >
          <X size={11} />
        </button>
      </div>
      {open && (
        <ul className="mt-1 space-y-0.5 pl-4">
          {todos.map((t, i) => (
            <li
              key={i}
              className={`flex items-start gap-1.5 ${t.done ? "text-faint line-through" : "text-dim"}`}
            >
              <span className="shrink-0">{t.done ? "✓" : "○"}</span>
              <span className="break-words">{t.text}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ── Slash commands (handled locally, never sent to the model) ──────────────

export const SLASH_COMMANDS: { name: string; hint: string }[] = [
  { name: "/help", hint: "List chat commands" },
  { name: "/new", hint: "Start a new conversation" },
  { name: "/compact", hint: "Summarize older turns to free context" },
  { name: "/distill", hint: "Summarize this session into memory, then start fresh" },
];

export function ReasoningBlock({ text, streaming, open, onToggle }: {
  text: string;
  streaming?: boolean;
  open?: boolean;
  onToggle?: () => void;
}) {
  const [openLocal, setOpenLocal] = useState(false);
  const isOpen = open ?? openLocal;
  const toggle = onToggle ?? (() => setOpenLocal((v) => !v));
  const preRef = useRef<HTMLPreElement>(null);
  // Follow the reasoning while it streams in (and show the end on open).
  useEffect(() => {
    if (!isOpen) return;
    const el = preRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, isOpen]);
  return (
    <div className="rounded border border-border bg-surface-2 px-2.5 py-1.5 text-[0.6875rem] text-dim mb-1.5">
      <button
        className="w-full flex items-center gap-1.5 text-left"
        onClick={toggle}
        title={isOpen ? "Hide reasoning" : "Show reasoning"}
      >
        <Brain size={11} className={`text-faint shrink-0 ${streaming ? "animate-pulse" : ""}`} />
        <span className="text-dim">
          {streaming ? "Thinking…" : isOpen ? "Reasoning" : "Show reasoning"}
        </span>
        <span className={`ml-auto transition-transform ${isOpen ? "rotate-180" : ""}`}>
          <ChevronDown size={12} />
        </span>
      </button>
      {isOpen && (
        <pre
          ref={preRef}
          className="whitespace-pre-wrap break-words text-[0.6875rem] leading-snug mt-1.5 max-h-56 overflow-y-auto select-text"
        >
          {text}
        </pre>
      )}
    </div>
  );
}

// ── Context ring (usage vs. model context + session avg tok/s) ──────────────

export function fmtTok(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(2)}K`;
  return `${n}`;
}

export function ContextRing({ used, total, avgTokps, model, genTokens, liveGenTps, livePromptTps, sessionPrompt, sessionGen, cost, dropDown = false }: {
  used: number | null;
  total: number | null;
  avgTokps: number | null;
  model?: string;
  genTokens?: number;
  liveGenTps?: number | null;
  livePromptTps?: number | null;
  /** Session totals (prompt + generated tokens) and an estimated cost. */
  sessionPrompt?: number | null;
  sessionGen?: number | null;
  cost?: number | null;
  /** Open the details downward (header placement) instead of upward. */
  dropDown?: boolean;
}) {
  const [hover, setHover] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const open = hover || pinned;
  const pct =
    used != null && total != null && total > 0 ? Math.min(1, used / total) : 0;
  const known = used != null && total != null && total > 0;
  const r = 13;
  const c = 2 * Math.PI * r;
  const color =
    pct >= 0.9 ? "stroke-accent-red" : pct >= 0.7 ? "stroke-accent-yellow" : "stroke-accent";
  const dot =
    pct >= 0.9 ? "bg-accent-red" : pct >= 0.7 ? "bg-accent-yellow" : "bg-accent-green";
  const remaining = known ? Math.max(0, (total as number) - (used as number)) : null;
  return (
    <div
      className="relative shrink-0 w-9 h-9"
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => {
        setHover(false);
        if (!pinned) setDetailsOpen(false);
      }}
    >
      <button
        className="block w-9 h-9 cursor-default"
        onClick={() => setPinned((v) => !v)}
        title={pinned ? "Unpin context details" : "Pin context details"}
      >
        <svg viewBox="0 0 32 32" className="w-9 h-9 -rotate-90">
          <circle cx="16" cy="16" r={r} fill="none" className="stroke-surface-3" strokeWidth="3" />
          <circle
            cx="16"
            cy="16"
            r={r}
            fill="none"
            className={color}
            strokeWidth="3"
            strokeLinecap="round"
            strokeDasharray={`${(c * pct).toFixed(1)} ${c.toFixed(1)}`}
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center text-[0.5rem] tabular-nums text-dim pointer-events-none">
          {known ? `${Math.round(pct * 100)}%` : "–"}
        </span>
      </button>
      {open && (
        <div
          className={`absolute w-64 border border-border bg-surface-2 p-3 z-50 select-text rounded ${
            dropDown ? "top-full mt-2 right-0" : "bottom-full mb-2 left-0"
          }`}
        >
          <div className="flex items-center gap-2 text-xs font-semibold text-ink">
            <span className={`w-1.5 h-1.5 rounded-full ${dot}`} />
            <span>
              Context · {known ? `${fmtTok(used as number)} / ${fmtTok(total as number)}` : "unknown"}
            </span>
          </div>
          <div className="h-1 bg-surface-3 overflow-hidden mt-2">
            <div
              className={`h-full ${dot}`}
              style={{ width: `${Math.round(pct * 100)}%` }}
            />
          </div>
          <div className="flex items-center justify-between mt-1.5 text-[0.6875rem] text-dim">
            <span>{known ? `${Math.round(pct * 100)}% used` : "usage unknown"}</span>
            <span className="tabular-nums">
              {remaining != null ? `${fmtTok(remaining)} remaining` : ""}
              {avgTokps != null && avgTokps > 0 ? ` · avg ${avgTokps.toFixed(1)} t/s` : ""}
            </span>
          </div>
          {(sessionPrompt != null || sessionGen != null || cost != null) && (
            <div className="border-t border-border mt-2 pt-1.5 space-y-1 text-[0.6875rem] text-dim">
              <div className="flex items-center justify-between gap-2">
                <span>Session</span>
                <span className="tabular-nums text-ink">
                  {sessionPrompt != null ? `${fmtTok(sessionPrompt)} in` : "– in"}
                  {" · "}
                  {sessionGen != null ? `${fmtTok(sessionGen)} out` : "– out"}
                </span>
              </div>
              {cost != null && (
                <div className="flex items-center justify-between gap-2">
                  <span>Estimated cost</span>
                  <span className="tabular-nums text-ink">
                    ${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(3)}
                  </span>
                </div>
              )}
            </div>
          )}
          <div className="border-t border-border mt-2 pt-1.5">
            <button
              className="w-full flex items-center justify-between text-[0.6875rem] text-dim hover:text-ink transition-colors"
              onClick={() => setDetailsOpen((v) => !v)}
            >
              <span>Token usage details</span>
              <ChevronDown size={12} className={`transition-transform ${detailsOpen ? "rotate-180" : ""}`} />
            </button>
            {detailsOpen && (
              <div className="mt-1.5 space-y-1 text-[0.6875rem] text-dim">
                <div className="flex justify-between gap-2">
                  <span>Model</span>
                  <span className="font-mono text-ink truncate max-w-[150px]">{model ?? "–"}</span>
                </div>
                <div className="flex justify-between gap-2">
                  <span>Last response</span>
                  <span className="tabular-nums text-ink">
                    {genTokens != null ? `${fmtTok(genTokens)} tok` : "–"}
                  </span>
                </div>
                <div className="flex justify-between gap-2">
                  <span>Prompt tokens</span>
                  <span className="tabular-nums text-ink">
                    {used != null ? fmtTok(used) : "–"}
                  </span>
                </div>
                <div className="flex justify-between gap-2">
                  <span>Generation</span>
                  <span className="tabular-nums text-ink">
                    {liveGenTps != null ? `${liveGenTps.toFixed(1)} t/s` : "–"}
                  </span>
                </div>
                <div className="flex justify-between gap-2">
                  <span>Prompt</span>
                  <span className="tabular-nums text-ink">
                    {livePromptTps != null ? `${livePromptTps.toFixed(1)} t/s` : "–"}
                  </span>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Harness chat (agent loop with sandboxed tools) ──────────────────────────


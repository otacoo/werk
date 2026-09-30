import { memo, useEffect, useLayoutEffect, useRef, useState, Fragment } from "react";
import type { ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkBreaks from "remark-breaks";
import rehypeKatex from "rehype-katex";
import rehypeHighlight from "rehype-highlight";
import type { PluggableList } from "unified";
import "katex/dist/katex.min.css";
import {
  ArrowUp,
  Brain,
  Check,
  Copy,
  Download,
  Eye,
  ExternalLink,
  FileWarning,
  FolderOpen,
  MoreHorizontal,
  Paperclip,
  Pencil,
  Plus,
  RefreshCw,
  Square,
  ChevronDown,
  Cloud,
  FolderTree,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  HarnessCapabilities,
  HarnessProject,
  ReasoningOptions,
  ContextStats,
  RunChange,
  ServerStatus,
  SessionSummary,
  WorktreeDto,
} from "../bindings";
import { call } from "../utils/ipc";
import { formatElapsed } from "../utils/format";
import { getBubbleAlign, getShowFileTree, getShowRunChanges, getShowToolSnippets, subscribeBubbleAlign, subscribeShowFileTree, subscribeShowRunChanges, subscribeShowToolSnippets } from "../utils/appearance";
import { playNotificationSound } from "../utils/sounds";
import ProjectTree from "../components/ProjectTree";
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
  | { kind: "tool"; callId: string; tool: string; args: string; output?: { ok: boolean; text: string } }
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

const EFFORT_LABELS: Record<string, string> = {
  none: "Off",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  max: "Max",
  xhigh: "X-High",
};

/// OpenAI-style effort levels for provider models.
const EXTERNAL_EFFORT_LEVELS = ["minimal", "low", "medium", "high"];

/// External API mode controls below the composer: the favorite-model
/// switcher (stars come from the Mode tab) plus the provider reasoning level.
function ExternalChatControls({
  target,
  favorites,
  effort,
  onTarget,
  onEffort,
  disabled,
}: {
  target: string;
  favorites: string[];
  effort: string;
  onTarget: (t: string) => void;
  onEffort: (e: string) => void;
  disabled: boolean;
}) {
  const options = favorites.includes(target) || !target ? favorites : [...favorites, target];
  const modelOf = (t: string) => {
    const cut = t.indexOf(":");
    return cut > 0 ? t.slice(cut + 1) : t;
  };
  return (
    <div className="flex items-center gap-2 mt-2">
      {options.length > 0 ? (
        <select
          className="input py-1 px-2 text-xs font-mono max-w-[16rem]"
          value={target}
          onChange={(e) => onTarget(e.target.value)}
          disabled={disabled}
          title="Favorite model"
        >
          {!target && <option value="">Select model…</option>}
          {options.map((t) => (
            <option key={t} value={t}>
              {modelOf(t)}
            </option>
          ))}
        </select>
      ) : (
        <span className="text-[0.625rem] text-faint">
          Star models on the Mode tab to switch between them here
        </span>
      )}
      <div className="flex items-center gap-1" title="Reasoning effort sent to the provider">
        <Brain size={12} className="text-[#E5484D] shrink-0" />
        <select
          className="input py-1 px-1 text-[0.625rem] w-20"
          value={EXTERNAL_EFFORT_LEVELS.includes(effort) ? effort : ""}
          onChange={(e) => onEffort(e.target.value)}
          disabled={disabled}
        >
          <option value="">Default</option>
          {EXTERNAL_EFFORT_LEVELS.map((level) => (
            <option key={level} value={level}>
              {EFFORT_LABELS[level] ?? level}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}

const REMARK_PLUGINS = [remarkGfm, remarkMath, remarkBreaks];
// `detect: false` only colors fenced blocks that name a language; unknown
// languages pass through untouched.
const REHYPE_PLUGINS: PluggableList = [
  rehypeKatex,
  [rehypeHighlight, { detect: false, ignoreMissing: true }],
];

// Code blocks get a copy button on hover; inline code renders untouched.
function CodeBlock({ children }: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative group/code">
      <pre ref={ref}>{children}</pre>
      <button
        className="absolute top-1.5 right-1.5 text-[0.625rem] text-dim hover:text-ink bg-surface-2 border border-border rounded px-1.5 py-0.5 opacity-0 group-hover/code:opacity-100 transition-opacity"
        onClick={async () => {
          const text = ref.current?.textContent ?? "";
          try {
            await navigator.clipboard.writeText(text);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          } catch {}
        }}
        title="Copy code"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

// Stable component overrides: an inline object would remount code blocks on
// every streamed delta, making the copy button flicker mid-hover.
const MD_COMPONENTS: Components = {
  pre: (props) => <CodeBlock>{props.children}</CodeBlock>,
  a: ({ href, children }) => (
    <button
      className="text-accent-soft underline break-all"
      onClick={() => {
        if (href) openUrl(href).catch(() => {});
      }}
    >
      {children}
    </button>
  ),
};

function Markdown({ content }: { content: string }) {
  return (
    <div className="md select-text">
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        components={MD_COMPONENTS}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

// ── System notice (compaction cards, restored summaries) ────────────────────
// Renders as a 1px divider labeled "Compaction" with the details below;
// long summaries stay expandable.

function SysNotice({ text }: { text: string }) {
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
        <div className="max-w-[85%] text-center">
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

// ── Sidebar: projects + sessions ────────────────────────────────────────────

type RowMenuItems = { label: string; icon: ReactNode; danger?: boolean; onClick: () => void }[];

function RowMenu({ items }: { items: RowMenuItems }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative shrink-0">
      <button
        className={`text-faint hover:text-ink ${open ? "opacity-100" : "opacity-0 group-hover:opacity-100"}`}
        onClick={(e) => { e.stopPropagation(); setOpen((v) => !v); }}
        title="Actions"
      >
        <MoreHorizontal size={12} />
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={(e) => { e.stopPropagation(); setOpen(false); }} />
          <div className="absolute right-0 top-full z-50 min-w-[170px] bg-surface-2 border border-border shadow-lg py-1 rounded">
            {items.map((it) => (
              <button
                key={it.label}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left transition-colors ${it.danger ? "text-accent-red hover:bg-surface-3" : "text-ink hover:bg-surface-3"}`}
                onClick={(e) => { e.stopPropagation(); setOpen(false); it.onClick(); }}
              >
                {it.icon}
                {it.label}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

// Inline row editor for Rename (Enter commits, Esc/blur cancels).
function RenameInput({ value, onCommit, onCancel }: {
  value: string;
  onCommit: (v: string) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState(value);
  return (
    <input
      autoFocus
      className="input flex-1 min-w-0 py-0.5 px-1.5 text-xs"
      value={draft}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => setDraft(e.target.value)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") onCommit(draft.trim() || value);
        else if (e.key === "Escape") onCancel();
      }}
      onBlur={onCancel}
    />
  );
}

// Sidebar width limits (persisted in localStorage).
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 480;

function ChatSidebar({ onProjectChanged, onSessionPicked }: {
  onProjectChanged: () => void;
  onSessionPicked: () => void;
}) {
  const [projects, setProjects] = useState<HarnessProject[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [newAllowPath, setNewAllowPath] = useState("");
  const [wtOpen, setWtOpen] = useState(true);
  const [allowOpen, setAllowOpen] = useState(true);
  const [sessOpen, setSessOpen] = useState(true);
  const [worktrees, setWorktrees] = useState<WorktreeDto[]>([]);
  const [isGitRepo, setIsGitRepo] = useState(false);
  const [newBranch, setNewBranch] = useState("");
  const [wtError, setWtError] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ kind: "session" | "project"; id: string; value: string } | null>(null);
  const [width, setWidth] = useState(() => {
    const w = Number(localStorage.getItem("werk.chat.sidebar.w"));
    return w >= SIDEBAR_MIN && w <= SIDEBAR_MAX ? w : 240;
  });

  const refreshProjects = async () => {
    try {
      const c = await call(commands.getConfig());
      setProjects(c.harness_projects ?? []);
      setActive(c.harness_active_project ?? null);
    } catch {}
  };

  const refreshSessions = async () => {
    try {
      setSessions(await call(commands.harnessSessionsList()));
    } catch {}
  };

  const activeExtraRead = projects.find((p) => p.id === active)?.extra_read ?? [];
  const activePath = projects.find((p) => p.id === active)?.path ?? null;

  const refreshWorktrees = async (root: string | null) => {
    if (!root) {
      setWorktrees([]);
      setIsGitRepo(false);
      setWtError(null);
      return;
    }
    try {
      const isRepo = await call(commands.harnessGitIsRepo(root));
      setIsGitRepo(isRepo);
      if (!isRepo) {
        setWorktrees([]);
        setWtError(null);
        return;
      }
      setWorktrees(await call(commands.harnessWorktreeList(root)));
      setWtError(null);
    } catch {
      setWorktrees([]);
      setIsGitRepo(false);
      setWtError(null); // repo check failed — hide quietly
    }
  };

  useEffect(() => {
    refreshProjects();
    refreshSessions();
    const id = setInterval(refreshSessions, 5000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  useEffect(() => {
    refreshWorktrees(activePath);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const addProject = async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked === "string" && picked) {
        await call(commands.harnessProjectAdd(picked));
        await refreshProjects();
        onProjectChanged();
      }
    } catch {}
  };

  const removeProject = async (id: string) => {
    try {
      await call(commands.harnessProjectRemove(id));
    } catch {}
    await refreshProjects();
    onProjectChanged();
  };

  const renameProject = async (id: string, name: string) => {
    try {
      await call(commands.harnessProjectRename(id, name));
    } catch {}
    await refreshProjects();
    onProjectChanged();
  };

  const activateProject = async (id: string | null) => {
    try {
      await call(commands.harnessProjectSetActive(id));
    } catch {}
    setActive(id);
    const root = projects.find((p) => p.id === id)?.path ?? null;
    refreshWorktrees(root);
    onProjectChanged();
  };

  const deleteSession = async (id: string) => {
    try {
      await call(commands.harnessSessionDelete(id));
    } catch {}
    await refreshSessions();
    onSessionPicked();
  };

  const renameSession = async (id: string, title: string) => {
    try {
      await call(commands.harnessSessionRename(id, title));
    } catch {}
    await refreshSessions();
  };

  const exportSession = async (s: SessionSummary) => {
    try {
      const path = await saveDialog({
        defaultPath: `${s.title.replace(/[\\/:*?"<>|]/g, "_")}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return;
      await call(commands.harnessSessionExport(s.id, path));
    } catch {}
  };

  const loadSession = async (id: string) => {
    try {
      await call(commands.harnessSessionLoad(id));
    } catch {}
    onSessionPicked();
  };

  const saveAllowlist = async (paths: string[]) => {
    try {
      await call(commands.setHarnessProjectExtraRead(paths));
    } catch {}
    await refreshProjects();
    onProjectChanged();
  };

  const addAllowPath = async () => {
    const path = newAllowPath.trim();
    if (!path || activeExtraRead.includes(path)) return;
    setNewAllowPath("");
    await saveAllowlist([...activeExtraRead, path]);
  };

  const removeAllowPath = async (path: string) => {
    await saveAllowlist(activeExtraRead.filter((p) => p !== path));
  };

  const addWorktree = async () => {
    const branch = newBranch.trim();
    if (!branch || !activePath) return;
    try {
      await call(commands.harnessWorktreeAdd(activePath, branch));
      setNewBranch("");
      refreshWorktrees(activePath);
    } catch (e) {
      setWtError(String(e));
    }
  };

  const removeWorktree = async (path: string) => {
    if (!activePath) return;
    try {
      await call(commands.harnessWorktreeRemove(activePath, path, false));
    } catch (e) {
      const msg = String(e);
      if (/clean|uncommitted|locked|dirty/i.test(msg)) {
        if (!window.confirm(`Remove worktree ${path}?\n\n${msg}\n\nForce removal drops uncommitted changes.`)) {
          return;
        }
        try {
          await call(commands.harnessWorktreeRemove(activePath, path, true));
        } catch (e2) {
          setWtError(String(e2));
          return;
        }
      } else {
        setWtError(msg);
        return;
      }
    }
    refreshWorktrees(activePath);
  };

  // Drag the right edge to resize; persists to localStorage on release.
  const startDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    let cur = startW;
    const move = (ev: MouseEvent) => {
      cur = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startW + (ev.clientX - startX)));
      setWidth(cur);
    };
    const up = () => {
      localStorage.setItem("werk.chat.sidebar.w", String(cur));
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <aside
      className="shrink-0 border-r border-border bg-surface-1 flex flex-col overflow-y-auto relative"
      style={{ width }}
    >
      <div
        className="absolute top-0 right-0 w-1 h-full cursor-col-resize hover:bg-accent/40 transition-colors z-10"
        onMouseDown={startDrag}
        title="Drag to resize"
      />
      <div className="p-3 border-b border-border">
        <div className="flex items-center justify-between px-1 mb-1.5">
          <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">Projects</span>
          <div className="flex items-center gap-2">
            <button
              className="text-dim hover:text-ink transition-colors"
              onClick={() => refreshProjects()}
              title="Refresh projects"
            >
              <RefreshCw size={12} />
            </button>
            <button
              className="text-dim hover:text-ink transition-colors"
              onClick={addProject}
              title="Add a project (working directory)"
            >
              <Plus size={13} />
            </button>
          </div>
        </div>
        <div className="space-y-0.5">
          {projects.map((p) => (
            <div
              key={p.id}
              className={`group flex items-center gap-2 px-2 py-1.5 rounded text-xs cursor-pointer transition-colors ${
                p.id === active ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
              onClick={(e) => {
                if (e.shiftKey) { removeProject(p.id); return; }
                if (!editing) activateProject(p.id);
              }}
              title={p.path}
            >
              <FolderOpen size={12} className="shrink-0" />
              {editing?.kind === "project" && editing.id === p.id ? (
                <RenameInput
                  value={editing.value}
                  onCommit={(v) => { setEditing(null); renameProject(p.id, v); }}
                  onCancel={() => setEditing(null)}
                />
              ) : (
                <>
                  <span className="flex-1 truncate">{p.name}</span>
                  <RowMenu
                    items={[
                      { label: "Open location", icon: <ExternalLink size={11} />, onClick: () => { revealItemInDir(p.path).catch(() => {}); } },
                      { label: "Rename", icon: <Pencil size={11} />, onClick: () => setEditing({ kind: "project", id: p.id, value: p.name }) },
                      { label: "Delete (Shift+Click)", icon: <Trash2 size={11} />, danger: true, onClick: () => removeProject(p.id) },
                    ]}
                  />
                </>
              )}
            </div>
          ))}
          {projects.length === 0 && (
            <p className="text-[0.6875rem] text-faint px-2 leading-snug">
              Add a folder to sandbox the agent to a project.
            </p>
          )}
        </div>
      </div>

      {active && isGitRepo && (
        <div className="p-3 border-b border-border">
          <button
            className="w-full flex items-center justify-between px-1 mb-1.5"
            onClick={() => setWtOpen((v) => !v)}
            title={wtOpen ? "Collapse worktrees" : "Expand worktrees"}
          >
            <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">Worktrees</span>
            <ChevronDown size={12} className={`text-faint transition-transform ${wtOpen ? "" : "-rotate-90"}`} />
          </button>
          {wtOpen && (
          <>
          <div className="space-y-0.5">
            {worktrees.map((w) => (
              <div
                key={w.path}
                className="group flex items-center gap-2 px-2 py-1.5 rounded text-xs text-dim hover:text-ink hover:bg-accent/10 transition-colors"
                title={w.path}
              >
                <span className="flex-1 truncate">
                  {w.branch ?? "(detached)"}
                  {w.main && <span className="ml-1.5 text-[0.5625rem] text-faint uppercase">main</span>}
                </span>
                {!w.main && (
                  <button
                    className="opacity-0 group-hover:opacity-100 text-faint hover:text-accent-red"
                    onClick={() => removeWorktree(w.path)}
                    title={`Remove worktree ${w.branch ?? w.path}`}
                  >
                    <Trash2 size={11} />
                  </button>
                )}
              </div>
            ))}
          </div>
          <div className="flex items-center gap-1.5 mt-1.5 px-1">
            <input
              className="input flex-1 py-1 px-2 text-xs min-w-0"
              placeholder="new-branch"
              value={newBranch}
              onChange={(e) => setNewBranch(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addWorktree();
                }
              }}
            />
            <button
              className="text-dim hover:text-ink transition-colors shrink-0"
              onClick={addWorktree}
              title="Create a worktree on a new branch (sibling folder)"
            >
              <Plus size={13} />
            </button>
          </div>
          {wtError && (
            <p className="text-[0.625rem] text-accent-red px-1 mt-1 break-words">{wtError}</p>
          )}
          </>
          )}
        </div>
      )}

      {active && (
        <div className="p-3 border-b border-border">
          <button
            className="w-full flex items-center justify-between px-1 mb-1.5"
            onClick={() => setAllowOpen((v) => !v)}
            title={allowOpen ? "Collapse read allowlist" : "Expand read allowlist"}
          >
            <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">Read allowlist</span>
            <ChevronDown size={12} className={`text-faint transition-transform ${allowOpen ? "" : "-rotate-90"}`} />
          </button>
          {allowOpen && (
          <>
          <div className="space-y-0.5">
            {activeExtraRead.map((p) => (
              <div
                key={p}
                className="group flex items-center gap-2 px-2 py-1.5 rounded text-xs text-dim hover:text-ink hover:bg-accent/10 transition-colors"
                title={p}
              >
                <span className="flex-1 truncate font-mono">{p}</span>
                <button
                  className="opacity-0 group-hover:opacity-100 text-faint hover:text-accent-red shrink-0"
                  onClick={() => removeAllowPath(p)}
                  title={`Remove ${p} from the read allowlist`}
                >
                  <Trash2 size={11} />
                </button>
              </div>
            ))}
          </div>
          <div className="flex items-center gap-1.5 mt-1.5 px-1">
            <input
              className="input flex-1 py-1 px-2 text-xs min-w-0 font-mono"
              placeholder="D:\shared\assets"
              value={newAllowPath}
              onChange={(e) => setNewAllowPath(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addAllowPath();
                }
              }}
            />
            <button
              className="text-dim hover:text-ink transition-colors shrink-0"
              onClick={addAllowPath}
              title="Allowlist this path (read-only, applies on next run)"
            >
              <Plus size={13} />
            </button>
          </div>
          <p className="text-[0.625rem] text-faint px-1 mt-1 leading-snug">
            Read-only — the agent can never write outside the project.
          </p>
          </>
          )}
        </div>
      )}

      {active && (
      <div className="flex-1 overflow-y-auto p-3">
        <button
          className="w-full flex items-center justify-between px-1"
          onClick={() => setSessOpen((v) => !v)}
          title={sessOpen ? "Collapse chat sessions" : "Expand chat sessions"}
        >
          <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">Chat Sessions</span>
          <ChevronDown size={12} className={`text-faint transition-transform ${sessOpen ? "" : "-rotate-90"}`} />
        </button>
        {sessOpen && (
        <div className="space-y-0.5 mt-1.5">
          {sessions.map((s) => (
            <div
              key={s.id}
              className="group flex items-center gap-2 px-2 py-1.5 rounded text-xs text-dim hover:text-ink hover:bg-accent/10 cursor-pointer transition-colors"
              onClick={(e) => {
                if (e.shiftKey) { deleteSession(s.id); return; }
                if (!editing) loadSession(s.id);
              }}
              title={new Date(s.updated * 1000).toLocaleString()}
            >
              {editing?.kind === "session" && editing.id === s.id ? (
                <RenameInput
                  value={editing.value}
                  onCommit={(v) => { setEditing(null); renameSession(s.id, v); }}
                  onCancel={() => setEditing(null)}
                />
              ) : (
                <>
                  <span className="flex-1 truncate">{s.title}</span>
                  <button
                    className="opacity-0 group-hover:opacity-100 text-faint hover:text-accent-red shrink-0"
                    onClick={(e) => { e.stopPropagation(); deleteSession(s.id); }}
                    title="Delete chat"
                  >
                    <Trash2 size={11} />
                  </button>
                  <RowMenu
                    items={[
                      { label: "Rename", icon: <Pencil size={11} />, onClick: () => setEditing({ kind: "session", id: s.id, value: s.title }) },
                      { label: "Export…", icon: <Download size={11} />, onClick: () => exportSession(s) },
                      { label: "Delete (Shift+Click)", icon: <Trash2 size={11} />, danger: true, onClick: () => deleteSession(s.id) },
                    ]}
                  />
                </>
              )}
            </div>
          ))}
          {sessions.length === 0 && (
            <p className="text-[0.6875rem] text-faint px-2 leading-snug">No sessions yet.</p>
          )}
        </div>
        )}
      </div>
      )}
    </aside>
  );
}

// ── Copy button with "Copied" feedback (user messages, errors) ─────────────

function CopyButton({ text, label }: { text: string; label: string }) {
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

function ChangesCard({
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
      <div className="max-w-[85%] rounded border border-border bg-surface-2 px-3 py-2 text-xs space-y-1.5">
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

function ResponseFooter({ model, tokps, elapsedMs, tokens, onCopy, onDelete }: {
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

// ── Tool call card: collapsed by default so long outputs (PowerShell error
// walls included) don't flood the transcript. Chevron expands args + output.
const ToolCard = memo(function ToolCard({ tool, args, output }: {
  tool: string;
  args: string;
  output?: { ok: boolean; text: string };
}) {
  const [open, setOpen] = useState(false);
  const [showSnippets, setShowSnippets] = useState(getShowToolSnippets());
  useEffect(() => subscribeShowToolSnippets(setShowSnippets), []);
  const [align, setAlign] = useState(getBubbleAlign());
  useEffect(() => subscribeBubbleAlign(setAlign), []);
  return (
    <div className={`flex items-center gap-2 ${align === "right" ? "justify-start" : "justify-end"}`}>
      <div className="max-w-[85%] rounded border border-border bg-surface-2 px-3 py-2 text-xs">
        <button
          className="w-full text-dim flex items-center gap-1.5 text-left"
          onClick={() => setOpen((v) => !v)}
          title={open ? "Collapse tool call" : "Expand tool call"}
        >
          <Wrench size={11} className="text-faint shrink-0" />
          <span className="text-ink font-medium">{tool}</span>
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

// ── Slash commands (handled locally, never sent to the model) ──────────────

const SLASH_COMMANDS: { name: string; hint: string }[] = [
  { name: "/help", hint: "List chat commands" },
  { name: "/new", hint: "Start a new conversation" },
  { name: "/compact", hint: "Summarize older turns to free context" },
];

function ReasoningBlock({ text, streaming, open, onToggle }: {
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
        <Brain size={11} className="text-faint shrink-0" />
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

function fmtTok(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(2)}K`;
  return `${n}`;
}

function ContextRing({ used, total, avgTokps, model, genTokens, liveGenTps, livePromptTps }: {
  used: number | null;
  total: number | null;
  avgTokps: number | null;
  model?: string;
  genTokens?: number;
  liveGenTps?: number | null;
  livePromptTps?: number | null;
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
        <div className="absolute bottom-full left-0 mb-2 w-64 border border-border bg-surface-2 p-3 z-50 select-text rounded">
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

export default function Chat({ go }: { go: (t: Tab) => void }) {
  const [status, setStatus] = useState<ServerStatus>({ type: "stopped" });
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
  // Live slot context (refreshed with the status poll); falls back to the
  // last run's usage + GGUF length when the server can't report it.
  const [slotCtx, setSlotCtx] = useState<ContextStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  /// File tree: the Settings option enables the feature; the header button
  /// toggles the panel while it is enabled.
  const [filesEnabled, setFilesEnabled] = useState(getShowFileTree());
  const [filesOpen, setFilesOpen] = useState(() => {
    try {
      return localStorage.getItem("werk.chat.files.open") !== "0";
    } catch {
      return true;
    }
  });
  useEffect(
    () =>
      subscribeShowFileTree((v) => {
        setFilesEnabled(v);
        setFilesOpen(v);
      }),
    [],
  );
  const [runChangesShown, setRunChangesShown] = useState(getShowRunChanges());
  useEffect(() => subscribeShowRunChanges(setRunChangesShown), []);
  const setPanel = (v: boolean) => {
    try {
      localStorage.setItem("werk.chat.files.open", v ? "1" : "0");
    } catch {}
    setFilesOpen(v);
  };
  const [activeProject, setActiveProject] = useState<string | null>(null);
  /// External API mode: chat works server-less through the configured provider.
  const [externalMode, setExternalMode] = useState(false);
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
  const [pendingQuestion, setPendingQuestion] = useState<{ question: string; options: string[] } | null>(null);
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
    if (external) setFavorites(c.provider_favorites ?? []);
  };

  const selectTarget = async (t: string) => {
    setExternalTarget(t);
    try {
      await call(commands.setExternalTarget(t));
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    const refreshCaps = () => {
      call(commands.harnessAgentCapabilities()).then(setCaps).catch(() => {});
      call(commands.harnessReasoningOptions()).then(setReasoningOpts).catch(() => {});
    };
    // Model-dependent info goes stale when the server (re)starts with a
    // different model — refresh on the transition into running.
    let wasRunning = false;
    const poll = async () => {
      try {
        const [s, c] = await Promise.all([
          call(commands.getServerStatus()),
          call(commands.getConfig()).catch(() => null),
        ]);
        if (c) applyConfigFlags(c);
        const running = s.type === "running";
        if (running && !wasRunning) {
          refreshCaps();
        }
        wasRunning = running;
        setStatus(s);
        // Stats work server-less too (external provider / GGUF fallback).
        call(commands.harnessContextStats()).then(setSlotCtx).catch(() => {});
      } catch {}
    };
    refreshCaps();
    refreshActiveProject();
    // The transcript lives in the backend — restore it so chats are
    // consultable even when the server is stopped.
    restoreFromBackend();
    poll();
    const id = setInterval(poll, 2000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const restoreFromBackend = async () => {
    try {
      const res = await call(commands.harnessAgentHistory());
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
      options: string[];
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
              },
            ];
          });
          break;
        }
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
              text: `Context compacted — ${Number(ev.removed ?? 0)} older messages summarized to stay within the context window.`,
            },
          ]);
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
        try {
          await call(commands.harnessAgentCompact());
        } catch (e) {
          setError(String(e));
        }
        restoreFromBackend();
        return;
      }
      setInput("");
      setError(`Unknown command "${cmd}". Type /help for the list.`);
      return;
    }
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
  const canSend = (serverRunning || (externalMode && externalTarget !== "")) && activeProject != null;
  // Bubble alignment: user on the right by default; the model takes the other side.
  const userSide = bubbleAlign === "right";
  const userJustify = userSide ? "justify-end" : "justify-start";
  const modelJustify = userSide ? "justify-start" : "justify-end";

  return (
    <div className="flex h-full min-h-0">
      {sidebarOpen && (
        <ChatSidebar
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
            {filesEnabled && (
            <button
              className={`inline-flex items-center gap-1.5 text-xs ${filesOpen ? "text-ink" : "text-dim hover:text-ink"}`}
              onClick={() => setPanel(!filesOpen)}
              title={filesOpen ? "Hide project files" : "Show project files"}
            >
              <FolderTree size={13} className="shrink-0" />
              <span>Files</span>
            </button>
            )}
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
                  <div className="max-w-[80%]">
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
                />
              );
            }
            if (it.kind === "reasoning") {
              return <ReasoningBlock key={`reasoning-${i}`} text={it.text} />;
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
                <div className="max-w-[85%] rounded border border-accent-yellow/40 bg-accent-yellow/5 px-3 py-2 text-xs">
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
              <div className="max-w-[80%] w-full">
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
              <div className="max-w-[80%] rounded px-3 py-2 bg-surface-2 text-ink select-text">
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
                  disabled={!questionDraft.trim()}
                  onClick={() => answerQuestion(questionDraft)}
                >
                  Send
                </button>
              </div>
            </div>
          )}
          <div className="flex items-end gap-2">
            <ContextRing
              used={slotCtx?.used ?? contextUsed}
              total={slotCtx?.total ?? caps?.context_length ?? null}
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
      {filesEnabled && filesOpen && (
        <ProjectTree
          activeProject={activeProject}
          streaming={streaming}
          onClose={() => setPanel(false)}
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

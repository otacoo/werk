import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ChevronDown,
  Download,
  ExternalLink,
  FolderOpen,
  FolderTree,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
  SquareTerminal,
  Trash2,
} from "lucide-react";
import { commands } from "../../bindings";
import type { HarnessProject, SessionSummary, WorktreeDto } from "../../bindings";
import { call } from "../../utils/ipc";

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

export function ChatSidebar({ onProjectChanged, onSessionPicked, visible = true, panel, onPanel }: {
  onProjectChanged: () => void;
  onSessionPicked: () => void;
  visible?: boolean;
  panel: "files" | "terminal" | null;
  onPanel: (v: "files" | "terminal" | null) => void;
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
  const [undo, setUndo] = useState<{ id: string; title: string } | null>(null);
  const undoTimer = useRef<number | null>(null);

  // The undo bar is temporary; clear its timer on unmount.
  useEffect(() => () => {
    if (undoTimer.current !== null) window.clearTimeout(undoTimer.current);
  }, []);
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

  const visibleRef = useRef(visible);
  useEffect(() => {
    visibleRef.current = visible;
    if (visible) refreshSessions();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible]);

  useEffect(() => {
    refreshProjects();
    refreshSessions();
    const id = setInterval(() => {
      if (visibleRef.current) refreshSessions();
    }, 5000);
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
    const title = sessions.find((s) => s.id === id)?.title ?? "session";
    try {
      await call(commands.harnessSessionDelete(id));
    } catch {
      return;
    }
    await refreshSessions();
    onSessionPicked();
    setUndo({ id, title });
    if (undoTimer.current !== null) window.clearTimeout(undoTimer.current);
    undoTimer.current = window.setTimeout(() => setUndo(null), 12_000);
  };

  const undoDelete = async () => {
    if (undoTimer.current !== null) window.clearTimeout(undoTimer.current);
    setUndo(null);
    try {
      await call(commands.harnessSessionUndo());
    } catch {}
    await refreshSessions();
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
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-border">
        <button
          className={`transition-colors ${panel === "files" ? "text-ink" : "text-dim hover:text-ink"}`}
          onClick={() => onPanel(panel === "files" ? null : "files")}
          title={panel === "files" ? "Hide project files" : "Show project files"}
        >
          <FolderTree size={13} />
        </button>
        <button
          className={`transition-colors ${panel === "terminal" ? "text-ink" : "text-dim hover:text-ink"}`}
          onClick={() => onPanel(panel === "terminal" ? null : "terminal")}
          title={panel === "terminal" ? "Hide terminal" : "Open a terminal in the project folder"}
        >
          <SquareTerminal size={13} />
        </button>
      </div>
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
        {undo && (
          <div className="mt-2 flex items-center gap-2 rounded border border-border bg-surface-2 px-2 py-1.5 text-[0.6875rem] text-dim">
            <span className="flex-1 truncate" title={`Deleted "${undo.title}"`}>
              Deleted "{undo.title}"
            </span>
            <button
              className="btn-secondary py-0.5 px-1.5 text-[0.6875rem] shrink-0"
              onClick={undoDelete}
            >
              Undo
            </button>
          </div>
        )}
      </div>
      )}
    </aside>
  );
}

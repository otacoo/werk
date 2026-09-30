import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  File as FileIcon,
  FolderOpen,
  RefreshCw,
  X,
} from "lucide-react";
import { openPath } from "@tauri-apps/plugin-opener";
import { commands } from "../bindings";
import type { ProjectFiles } from "../bindings";
import { call } from "../utils/ipc";

/** Width limits for the panel; persisted in localStorage. */
const FILES_MIN = 168;
const FILES_MAX = 520;
const FILES_DEFAULT = 256;

type Node = { path: string; name: string; dir: boolean; changed: boolean; touched: boolean; children: Node[] };

function buildTree(files: ProjectFiles): Node[] {
  const map = new Map<string, Node>();
  const roots: Node[] = [];
  for (const e of files.entries) {
    const node: Node = {
      path: e.path,
      name: e.name,
      dir: e.dir,
      changed: e.changed,
      touched: e.touched,
      children: [],
    };
    map.set(e.path, node);
    const cut = e.path.lastIndexOf("/");
    const parent = cut > 0 ? map.get(e.path.slice(0, cut)) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  return roots;
}

const anyDeep = (n: Node, pred: (n: Node) => boolean): boolean =>
  pred(n) || n.children.some((c) => anyDeep(c, pred));

function TreeRow({
  node,
  depth,
  expanded,
  toggle,
  onOpen,
  selected,
  onSelect,
}: {
  node: Node;
  depth: number;
  expanded: Set<string>;
  toggle: (path: string) => void;
  onOpen: (node: Node) => void;
  selected: string | null;
  onSelect: (path: string) => void;
}) {
  const open = expanded.has(node.path);
  const changedDeep = anyDeep(node, (n) => n.changed);
  const touchedDeep = anyDeep(node, (n) => n.touched);
  return (
    <>
      <div
        className={`group flex items-center gap-1.5 pr-2 py-0.5 rounded text-xs cursor-pointer transition-colors select-none ${
          node.dir
            ? "text-dim hover:text-ink hover:bg-accent/10"
            : node.path === selected
              ? "bg-accent/15 text-ink"
              : "text-ink hover:bg-accent/10"
        }`}
        style={{ paddingLeft: `${depth * 12 + 6}px` }}
        onClick={() => (node.dir ? toggle(node.path) : onSelect(node.path))}
        onDoubleClick={() => (!node.dir ? onOpen(node) : undefined)}
        title={node.dir ? node.path : `${node.path} (double-click to open)`}
      >
        {node.dir ? (
          <>
            <ChevronDown
              size={11}
              className={`shrink-0 text-faint transition-transform ${open ? "" : "-rotate-90"}`}
            />
            <FolderOpen size={12} className="shrink-0 text-accent-soft" />
          </>
        ) : (
          <>
            <span className="w-[11px] shrink-0" />
            <FileIcon size={12} className="shrink-0 text-faint" />
          </>
        )}
        <span className="flex-1 truncate">{node.name}</span>
        {node.touched && (
          <span
            className="w-1.5 h-1.5 rounded-full bg-accent-soft shrink-0"
            title="Modified by the agent in this session"
          />
        )}
        {node.changed && (
          <span className="badge-yellow text-[0.5rem] px-1 py-0 shrink-0" title="Changed on disk (git)">
            M
          </span>
        )}
        {node.dir && !node.changed && !node.touched && changedDeep && (
          <span className="w-1.5 h-1.5 rounded-full bg-accent-yellow/70 shrink-0" title="Contains changed files" />
        )}
        {node.dir && !node.changed && !node.touched && !changedDeep && touchedDeep && (
          <span className="w-1.5 h-1.5 rounded-full bg-accent-soft/70 shrink-0" title="Contains agent edits" />
        )}
      </div>
      {node.dir &&
        open &&
        node.children.map((c) => (
          <TreeRow
            key={c.path}
            node={c}
            depth={depth + 1}
            expanded={expanded}
            toggle={toggle}
            onOpen={onOpen}
            selected={selected}
            onSelect={onSelect}
          />
        ))}
    </>
  );
}

/// Project folder tree for the Chat sidebar; `M` marks git changes, a dot
/// marks files the agent wrote or edited this session.
export default function ProjectTree({
  activeProject,
  streaming,
  onClose,
}: {
  activeProject: string | null;
  streaming: boolean;
  onClose: () => void;
}) {
  const [files, setFiles] = useState<ProjectFiles | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [width, setWidth] = useState(() => {
    const w = Number(localStorage.getItem("werk.chat.files.w"));
    return w >= FILES_MIN && w <= FILES_MAX ? w : FILES_DEFAULT;
  });
  const wasStreaming = useRef(streaming);

  const startDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    const move = (ev: MouseEvent) => {
      const cur = Math.min(FILES_MAX, Math.max(FILES_MIN, startW - (ev.clientX - startX)));
      setWidth(cur);
      localStorage.setItem("werk.chat.files.w", String(cur));
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await call(commands.projectFileTree());
      setFiles(res);
      setExpanded((prev) => {
        const next = new Set(prev);
        const walk = (n: Node) => {
          if (n.dir && anyDeep(n, (x) => x.changed || x.touched)) next.add(n.path);
          n.children.forEach(walk);
        };
        buildTree(res).forEach(walk);
        return next;
      });
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load, activeProject]);

  // A finished run usually changed files; refresh automatically.
  useEffect(() => {
    if (wasStreaming.current && !streaming) load();
    wasStreaming.current = streaming;
  }, [streaming, load]);

  const tree = useMemo(() => (files ? buildTree(files) : []), [files]);
  const toggle = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  const openFile = (node: Node) => {
    if (!files) return;
    // Opens in the OS default application, not inside werk.
    openPath(`${files.root}/${node.path}`).catch((e) => setError(String(e)));
  };

  const changedCount = files?.entries.filter((e) => !e.dir && e.changed).length ?? 0;
  const touchedCount = files?.entries.filter((e) => !e.dir && e.touched).length ?? 0;

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
        <span className="text-[0.6875rem] font-semibold uppercase tracking-wide text-dim">Files</span>
        <div className="flex items-center gap-1.5">
          <button
            className="text-dim hover:text-ink transition-colors"
            onClick={load}
            disabled={loading}
            title="Refresh the tree"
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </button>
          <button className="text-dim hover:text-ink transition-colors" onClick={onClose} title="Hide files">
            <X size={13} />
          </button>
        </div>
      </div>
      <div className="flex-1 overflow-y-auto py-1.5">
        {error ? (
          <p className="text-[0.6875rem] text-accent-yellow px-3">{error}</p>
        ) : !files ? (
          <p className="text-[0.6875rem] text-dim px-3">Loading…</p>
        ) : files.entries.length === 0 ? (
          <p className="text-[0.6875rem] text-dim px-3">No files in this project.</p>
        ) : (
          tree.map((n) => (
            <TreeRow
              key={n.path}
              node={n}
              depth={0}
              expanded={expanded}
              toggle={toggle}
              onOpen={openFile}
              selected={selected}
              onSelect={setSelected}
            />
          ))
        )}
      </div>
      <div className="px-3 py-1.5 border-t border-border text-[0.625rem] text-faint shrink-0 leading-snug">
        {files?.truncated && <p className="text-accent-yellow">Tree truncated (large project).</p>}
        <p>Double-click a file to open it in your OS.</p>
        <p>
          <span className="badge-yellow text-[0.5rem] px-1 py-0 mr-1">M</span>
          {files?.git ? `${changedCount} changed on disk` : "not a git repository"}
        </p>
        <p className="mt-0.5">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-accent-soft mr-1 align-middle" />
          {touchedCount} agent edit{touchedCount === 1 ? "" : "s"} this session
        </p>
      </div>
    </aside>
  );
}

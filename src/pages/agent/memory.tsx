import { useEffect, useState } from "react";
import { FolderOpen, RefreshCw } from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands } from "../../bindings";
import type { MemoryFileDto, SkillDto } from "../../bindings";
import { call } from "../../utils/ipc";

export function MemoryCard() {
  const [files, setFiles] = useState<MemoryFileDto[]>([]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [dirty, setDirty] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    setError(null);
    for (const scope of ["global", "project"]) {
      call(commands.harnessMemoryGet(scope))
        .then((f) => {
          setFiles((prev) => [...prev.filter((x) => x.scope !== scope), f]);
          setDrafts((d) => (d[scope] === undefined ? { ...d, [scope]: f.text } : d));
        })
        .catch(() => {});
    }
  };

  useEffect(load, []);

  const save = async (scope: string) => {
    setError(null);
    try {
      await call(commands.harnessMemorySet(scope, drafts[scope] ?? ""));
      setDirty((d) => ({ ...d, [scope]: false }));
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  const editor = (scope: string, title: string, hint: string) => {
    const f = files.find((x) => x.scope === scope);
    if (scope === "project" && !f) return null;
    return (
      <div key={scope} className="border border-border rounded p-3">
        <div className="flex items-baseline justify-between gap-2">
          <div className="min-w-0">
            <p className="text-xs font-medium text-ink">{title}</p>
            <p className="text-[0.625rem] text-faint truncate font-mono">{f?.path ?? ""}</p>
          </div>
          <div className="flex items-center gap-1 shrink-0">
            {f?.backup !== undefined && (
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                title="Load the pre-distill backup into the editor (Save to apply)"
                onClick={() => {
                  setDrafts((d) => ({ ...d, [scope]: f?.backup ?? "" }));
                  setDirty((d) => ({ ...d, [scope]: true }));
                }}
              >
                Restore backup
              </button>
            )}
            {f?.path && (
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                title="Reveal file in the file manager"
                onClick={() => void revealItemInDir(f.path).catch((e) => setError(String(e)))}
              >
                <FolderOpen size={11} />
              </button>
            )}
            <button
              className="btn-primary text-[0.625rem] py-0.5 px-2"
              disabled={!dirty[scope]}
              onClick={() => save(scope)}
            >
              Save
            </button>
          </div>
        </div>
        <p className="text-[0.625rem] text-faint mt-1">{hint}</p>
        <textarea
          className="input w-full mt-1.5 font-mono text-[0.6875rem] leading-snug"
          rows={5}
          placeholder="Empty — facts the agent saves with its remember tool land here."
          value={drafts[scope] ?? ""}
          onChange={(e) => {
            setDrafts((d) => ({ ...d, [scope]: e.target.value }));
            setDirty((d) => ({ ...d, [scope]: true }));
          }}
        />
      </div>
    );
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Memory</h2>
        <button
          className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
          title="Reload memory files from disk"
          onClick={load}
        >
          <RefreshCw size={11} /> Refresh
        </button>
      </div>
      <p className="section-desc">
        What the agent remembers across sessions. Injected into its system prompt; the agent
        curates it via the remember tool (writes need your approval).
      </p>
      <div className="space-y-3 mt-3">
        {editor("global", "Global memory (MEMORY.md)", "Applies to every project — facts about you and your preferences.")}
        {editor("project", "Project memory (MEMORY.md)", "Applies to the active project only — conventions and corrections.")}
      </div>
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
    </div>
  );
}

export function SkillsCard() {
  const [skills, setSkills] = useState<SkillDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    call(commands.skillsList())
      .then(setSkills)
      .catch(() => setSkills([]));
  };

  useEffect(load, []);

  const openGlobalFolder = async () => {
    setError(null);
    try {
      const mem = await call(commands.harnessMemoryGet("global"));
      const base = mem.path.split(/[\\/]/).slice(0, -1).join("/");
      await openPath(`${base}/skills`);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Skills</h2>
      <p className="section-desc">
        Procedures the agent has learned. Each skill is a folder with a SKILL.md (name +
        description + instructions); only names/descriptions reach the system prompt — the agent
        loads the rest via its skill tool.
      </p>
      <div className="flex items-center gap-1 mt-3">
        <button
          className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
          title="Open global skills folder"
          onClick={openGlobalFolder}
        >
          <FolderOpen size={11} /> Global folder
        </button>
        <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" title="Refresh list" onClick={load}>
          <RefreshCw size={11} /> Refresh
        </button>
      </div>
      {skills && skills.length === 0 && (
        <p className="text-[0.6875rem] text-dim mt-2">
          No skills yet — ask the agent to save one (manage_skill).
        </p>
      )}
      {skills && skills.length > 0 && (
        <div className="space-y-1 mt-2">
          {skills.map((s) => (
            <div
              key={`${s.scope}:${s.name}`}
              className="flex items-center gap-2 border border-border rounded px-2.5 py-1.5"
            >
              <span className={`badge-${s.scope === "project" ? "blue" : "purple"} text-[0.5625rem] shrink-0`}>
                {s.scope}
              </span>
              <div className="flex-1 min-w-0">
                <p className="text-xs text-ink truncate">{s.name}</p>
                {s.description && <p className="text-[0.625rem] text-dim truncate">{s.description}</p>}
              </div>
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
                title="Open skill folder"
                onClick={() => void openPath(s.dir).catch((e) => setError(String(e)))}
              >
                <FolderOpen size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
    </div>
  );
}


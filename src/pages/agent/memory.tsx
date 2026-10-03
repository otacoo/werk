import { useEffect, useState } from "react";
import { Check, FolderOpen, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands } from "../../bindings";
import type { MemoryEntryDto, MemoryFileDto, SkillDto } from "../../bindings";
import { call } from "../../utils/ipc";

export function MemoryCard() {
  const [reload, setReload] = useState(0);
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="card">
      <div className="flex items-center justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Memory</h2>
        <button
          className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
          title="Reload memory files from disk"
          onClick={() => setReload((v) => v + 1)}
        >
          <RefreshCw size={11} /> Refresh
        </button>
      </div>
      <p className="section-desc">
        What the agent remembers across sessions. Injected into its system prompt; the agent
        curates it via the remember tool (writes need your approval).
      </p>
      <div className="space-y-3 mt-3">
        <MemoryScopeEditor
          scope="global"
          title="Global memory (MEMORY.md)"
          hint="Applies to every project — facts about you and your preferences."
          reload={reload}
          onError={setError}
        />
        <MemoryScopeEditor
          scope="project"
          title="Project memory (MEMORY.md)"
          hint="Applies to the active project only — conventions and corrections."
          reload={reload}
          onError={setError}
        />
      </div>
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
    </div>
  );
}

function MemoryScopeEditor({
  scope,
  title,
  hint,
  reload,
  onError,
}: {
  scope: string;
  title: string;
  hint: string;
  reload: number;
  onError: (e: string | null) => void;
}) {
  const [file, setFile] = useState<MemoryFileDto | null>(null);
  const [entries, setEntries] = useState<MemoryEntryDto[] | null>(null);
  const [dirty, setDirty] = useState(false);
  const [raw, setRaw] = useState(false);
  const [rawDraft, setRawDraft] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [topic, setTopic] = useState("");
  const [text, setText] = useState("");

  const load = () => {
    call(commands.harnessMemoryGet(scope))
      .then((f) => {
        setFile(f);
        setRawDraft(f.text);
      })
      .catch(() => setFile(null));
    call(commands.harnessMemoryEntries(scope))
      .then((e) => {
        setEntries(e);
        setDirty(false);
        setEditing(null);
      })
      .catch(() => setEntries(null));
  };
  useEffect(load, [scope, reload]);

  // The project scope needs an active project; hide until one resolves.
  if (scope === "project" && !file) return null;

  const startEdit = (i: number) => {
    const e = entries?.[i];
    if (!e) return;
    setEditing(i);
    setTopic(e.topic);
    setText(e.text);
  };

  const commitEdit = () => {
    if (editing === null) return;
    setEntries((prev) =>
      prev
        ? prev.map((e, i) =>
            i === editing ? { ...e, topic: topic.trim() || "note", text: text.trim() } : e,
          )
        : prev,
    );
    setEditing(null);
    setDirty(true);
  };

  const removeEntry = (i: number) => {
    setEntries((prev) => (prev ? prev.filter((_, k) => k !== i) : prev));
    if (editing === i) setEditing(null);
    setDirty(true);
  };

  const addEntry = () => {
    setEntries((prev) => {
      const next = [...(prev ?? []), { date: "", topic: "note", text: "" }];
      setEditing(next.length - 1);
      return next;
    });
    setTopic("note");
    setText("");
    setDirty(true);
  };

  const saveEntries = async () => {
    onError(null);
    try {
      await call(commands.harnessMemorySaveEntries(scope, entries ?? []));
      load();
    } catch (e) {
      onError(String(e));
    }
  };

  const saveRaw = async () => {
    onError(null);
    try {
      await call(commands.harnessMemorySet(scope, rawDraft));
      setRaw(false);
      load();
    } catch (e) {
      onError(String(e));
    }
  };

  const rawDirty = file !== null && rawDraft !== file.text;

  return (
    <div className="border border-border rounded p-3">
      <div className="flex items-baseline justify-between gap-2">
        <div className="min-w-0">
          <p className="text-xs font-medium text-ink">{title}</p>
          <p className="text-[0.625rem] text-faint truncate font-mono">{file?.path ?? ""}</p>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {file?.backup !== undefined && (
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
              title="Load the pre-distill backup into the raw editor (Save to apply)"
              onClick={() => {
                setRaw(true);
                setRawDraft(file?.backup ?? "");
              }}
            >
              Restore backup
            </button>
          )}
          {file?.path && (
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
              title="Reveal file in the file manager"
              onClick={() => {
                onError(null);
                call(commands.harnessMemoryEnsure(scope))
                  .then((path) => revealItemInDir(path))
                  .catch((e) => onError(String(e)));
              }}
            >
              <FolderOpen size={11} />
            </button>
          )}
          <button
            className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
            title={raw ? "Back to the entry list" : "Edit the raw file"}
            onClick={() => setRaw((v) => !v)}
          >
            {raw ? "List" : "Raw"}
          </button>
          <button
            className="btn-primary text-[0.625rem] py-0.5 px-2"
            disabled={raw ? !rawDirty : !dirty}
            onClick={raw ? saveRaw : saveEntries}
          >
            Save
          </button>
        </div>
      </div>
      <p className="text-[0.625rem] text-faint mt-1">{hint}</p>

      {raw ? (
        <textarea
          className="input w-full mt-1.5 font-mono text-[0.6875rem] leading-snug"
          rows={5}
          placeholder="Empty — facts the agent saves with its remember tool land here."
          value={rawDraft}
          onChange={(e) => setRawDraft(e.target.value)}
        />
      ) : (
        <div className="mt-2 space-y-1">
          {(entries ?? []).map((e, i) =>
            editing === i ? (
              <div key={i} className="border border-accent/50 rounded p-2 space-y-1.5">
                <input
                  className="input w-32 py-0.5 px-1.5 text-[0.6875rem]"
                  placeholder="topic"
                  value={topic}
                  onChange={(ev) => setTopic(ev.target.value)}
                />
                <textarea
                  className="input w-full text-[0.6875rem] leading-snug"
                  rows={2}
                  placeholder="What should be remembered?"
                  value={text}
                  onChange={(ev) => setText(ev.target.value)}
                />
                <div className="flex items-center justify-end gap-1">
                  <button
                    className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                    onClick={() => setEditing(null)}
                  >
                    Cancel
                  </button>
                  <button
                    className="btn-primary text-[0.625rem] py-0.5 px-2"
                    disabled={!text.trim()}
                    onClick={commitEdit}
                  >
                    <Check size={10} /> Apply
                  </button>
                </div>
              </div>
            ) : (
              <div
                key={i}
                className="flex items-start gap-2 border border-border rounded px-2.5 py-1.5"
              >
                <span className="badge-gray text-[0.5625rem] shrink-0 mt-0.5">
                  {e.topic || "note"}
                </span>
                <div className="flex-1 min-w-0">
                  <p className="text-[0.6875rem] text-ink break-words">{e.text}</p>
                  {e.date && <p className="text-[0.5625rem] text-faint">{e.date}</p>}
                </div>
                <button
                  className="text-faint hover:text-ink shrink-0"
                  title="Edit entry"
                  onClick={() => startEdit(i)}
                >
                  <Pencil size={11} />
                </button>
                <button
                  className="text-faint hover:text-accent-red shrink-0"
                  title="Delete entry"
                  onClick={() => removeEntry(i)}
                >
                  <Trash2 size={11} />
                </button>
              </div>
            ),
          )}
          {entries !== null && entries.length === 0 && (
            <p className="text-[0.6875rem] text-dim">
              Empty — facts the agent saves with its remember tool land here.
            </p>
          )}
          {entries !== null && (
            <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={addEntry}>
              <Plus size={10} /> Add entry
            </button>
          )}
        </div>
      )}
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
      await openPath(await call(commands.skillsEnsureDir()));
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


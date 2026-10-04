import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen, ImagePlus, RefreshCw } from "lucide-react";
import { commands } from "../bindings";
import type { AssistantConfig, MemoryFileDto, Reminder } from "../bindings";
import { call } from "../utils/ipc";
import { subscribeConfigChanged } from "../utils/appSettings";
import Toggle from "../components/Toggle";
import ProfileAvatar from "../components/ProfileAvatar";
import { EFFORT_LABELS } from "./chat/external-controls";
import { SkillsCard } from "./agent/memory";

type Tab = "persona" | "reminders" | "memory" | "access" | "behavior";

/// Assistant settings: persona, reminders, memory, access, and behavior.
/// The conversation lives in its own Chat tab.
export default function Assistant({ active = true }: { active?: boolean }) {
  const [tab, setTab] = useState<Tab>("persona");
  const [persona, setPersona] = useState<AssistantConfig | null>(null);
  const [avatar, setAvatar] = useState<string | null>(null);
  const [builtInPrompt, setBuiltInPrompt] = useState("");
  const [personaError, setPersonaError] = useState<string | null>(null);
  const [personaSaved, setPersonaSaved] = useState(false);

  const refreshConfig = () => {
    call(commands.getConfig())
      .then((c) => setPersona(c.assistant ?? null))
      .catch(() => {});
  };

  const loadAvatar = () => {
    call(commands.assistantAvatar())
      .then(setAvatar)
      .catch(() => setAvatar(null));
  };

  useEffect(() => {
    refreshConfig();
    loadAvatar();
    call(commands.assistantSystemPromptDefault())
      .then(setBuiltInPrompt)
      .catch(() => {});
    const unsub = subscribeConfigChanged(() => {
      refreshConfig();
      loadAvatar();
    });
    return unsub;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (active) {
      refreshConfig();
      loadAvatar();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  const pickAvatar = async () => {
    let picked: string | string[] | null = null;
    try {
      picked = await openDialog({
        multiple: false,
        filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
      });
    } catch {
      return;
    }
    if (!picked || typeof picked !== "string") return;
    try {
      await call(commands.assistantSetAvatar(picked));
      loadAvatar();
    } catch (e) {
      setPersonaError(String(e));
    }
  };

  const clearAvatar = async () => {
    try {
      await call(commands.assistantSetAvatar(null));
      loadAvatar();
    } catch (e) {
      setPersonaError(String(e));
    }
  };

  const savePersona = async () => {
    if (!persona) return;
    setPersonaError(null);
    // An override equal to the built-in prompt is the same as none.
    const prompt = persona.system_prompt?.trim() ?? "";
    const toSave = {
      ...persona,
      system_prompt:
        prompt === "" || prompt === builtInPrompt.trim() ? null : persona.system_prompt,
    };
    try {
      await call(commands.setAssistantConfig(toSave));
      setPersonaSaved(true);
      setTimeout(() => setPersonaSaved(false), 1500);
      refreshConfig();
    } catch (e) {
      setPersonaError(String(e));
    }
  };

  const TABS: { id: Tab; label: string }[] = [
    { id: "persona", label: "Persona" },
    { id: "reminders", label: "Reminders" },
    { id: "memory", label: "Memory" },
    { id: "access", label: "Access" },
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

      {tab === "persona" && persona && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <div className="card space-y-3">
              <div>
                <h2 className="section-title mb-0">Identity</h2>
                <p className="section-desc">
                  The assistant's name, profile image, and personality.
                </p>
              </div>
              <div className="flex items-center gap-3">
                <ProfileAvatar src={avatar} name={persona.name || "Werk"} size={48} />
                <div className="flex items-center gap-1.5">
                  <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={pickAvatar}>
                    <ImagePlus size={11} /> Choose image
                  </button>
                  {avatar && (
                    <button
                      className="btn-ghost text-[0.625rem] py-0.5 px-1.5 text-accent-red"
                      onClick={clearAvatar}
                    >
                      Clear
                    </button>
                  )}
                </div>
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
            </div>
            <div className="card space-y-2">
              <div>
                <h2 className="section-title mb-0">System prompt</h2>
                <p className="section-desc">
                  Always sent first, before the persona, memory, and skills. Edit it here, or
                  reset to follow the built-in default; {"{{name}}"} becomes the assistant name.
                </p>
              </div>
              <textarea
                className="input w-full font-mono text-[0.6875rem] leading-snug"
                rows={10}
                value={persona.system_prompt ?? builtInPrompt}
                onChange={(e) => setPersona({ ...persona, system_prompt: e.target.value })}
              />
              <div className="flex items-center gap-2">
                <button
                  className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                  onClick={() => setPersona({ ...persona, system_prompt: null })}
                  title="Clear the custom prompt and follow the built-in default"
                >
                  Reset
                </button>
                <span className="text-[0.6875rem] text-faint ml-auto text-right">
                  {persona.system_prompt?.trim()
                    ? "Custom system prompt active."
                    : "Using the built-in default."}
                </span>
              </div>
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

      {tab === "access" && (
        <div className="flex-1 overflow-y-auto p-6">
          <div className="max-w-3xl mx-auto space-y-4">
            <AccessCard />
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

/// Opt-in system control: master switch, home folder, and per-tool toggles.
function AccessCard() {
  const [access, setAccess] = useState<{
    system_control: boolean;
    workspace: string | null;
    files: boolean;
    clipboard: boolean;
    windows: boolean;
    screen: boolean;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const load = () => {
    call(commands.getConfig())
      .then((c) =>
        setAccess({
          system_control: c.assistant?.system_control ?? false,
          workspace: c.assistant?.workspace ?? null,
          files: c.assistant?.tool_files ?? true,
          clipboard: c.assistant?.tool_clipboard ?? true,
          windows: c.assistant?.tool_windows ?? true,
          screen: c.assistant?.tool_screen ?? true,
        }),
      )
      .catch(() => {});
  };
  useEffect(load, []);

  const pickFolder = async () => {
    let picked: string | string[] | null = null;
    try {
      picked = await openDialog({ directory: true, multiple: false });
    } catch {
      return;
    }
    if (!picked || typeof picked !== "string") return;
    setAccess((a) => (a ? { ...a, workspace: picked } : a));
  };

  const save = async () => {
    if (!access) return;
    setError(null);
    try {
      await call(
        commands.setAssistantAccess(
          access.system_control,
          access.workspace,
          access.files,
          access.clipboard,
          access.windows,
          access.screen,
        ),
      );
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  if (!access) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">System access</h2>
        <p className="section-desc">
          Off by default. When enabled, the assistant gets file tools over its own folder plus an
          always-granted temp workspace, and clipboard, window, and screen tools; every mutating
          action still asks for approval, and the sensitive-file policy stays in force.
        </p>
      </div>
      <div className="space-y-1.5 border border-border rounded px-3 py-2">
        <p className="text-xs font-medium text-ink">Built-in tools</p>
        <p className="text-[0.625rem] text-faint">Always available to the assistant.</p>
        {(
          [
            ["remember", "Curate the assistant's memory."],
            ["web_search", "Search the web (DuckDuckGo)."],
            ["get_time", "Current date and time."],
            ["ask_user", "Ask a multiple-choice question."],
            ["skill", "Load a saved skill."],
            ["reminder", "Schedule reminders and alerts."],
          ] as const
        ).map(([name, desc]) => (
          <div key={name} className="flex items-baseline gap-2 min-w-0">
            <span className="font-mono text-[0.6875rem] text-ink shrink-0">{name}</span>
            <span className="text-[0.625rem] text-faint truncate">{desc}</span>
          </div>
        ))}
      </div>
      <Toggle
        label="Allow system control"
        hint="Master switch for everything on this tab."
        checked={access.system_control}
        onChange={(v) => setAccess({ ...access, system_control: v })}
      />
      {access.system_control && (
        <>
          <div className="space-y-2 border border-border rounded px-3 py-2">
            <div className="flex items-center justify-between">
              <p className="text-xs font-medium text-ink">Assistant folder</p>
              <div className="flex items-center gap-1">
                <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={pickFolder}>
                  <FolderOpen size={11} /> Choose…
                </button>
                {access.workspace && (
                  <button
                    className="btn-ghost text-[0.625rem] py-0.5 px-1.5 text-accent-red"
                    onClick={() => setAccess({ ...access, workspace: null })}
                  >
                    Clear
                  </button>
                )}
              </div>
            </div>
            <p className="font-mono text-[0.6875rem] text-dim break-all">
              {access.workspace ?? "Not set — the temp workspace is used for everything."}
            </p>
            <p className="text-[0.625rem] text-faint">
              The assistant's persistent files (documents it wants to keep) live here. A temp
              workspace under the system temp folder is always granted for scratch files, scripts,
              and screenshots.
            </p>
          </div>
          <Toggle
            label="File tools"
            hint="Read, write, edit, find, and search inside the assistant and temp folders."
            checked={access.files}
            onChange={(v) => setAccess({ ...access, files: v })}
          />
          <Toggle
            label="Clipboard"
            hint="Read the clipboard and copy text."
            checked={access.clipboard}
            onChange={(v) => setAccess({ ...access, clipboard: v })}
          />
          <Toggle
            label="Window control"
            hint="List, focus, minimize, maximize, close, and move windows."
            checked={access.windows}
            onChange={(v) => setAccess({ ...access, windows: v })}
          />
          <Toggle
            label="Screen capture"
            hint="Capture monitors or windows to PNG files."
            checked={access.screen}
            onChange={(v) => setAccess({ ...access, screen: v })}
          />
        </>
      )}
      <div className="flex items-center justify-end gap-2">
        {error && <p className="text-xs text-accent-red mr-auto">{error}</p>}
        <button className="btn-primary text-xs py-1 px-2" onClick={save}>
          {saved ? "Saved" : "Save"}
        </button>
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
        <p className="section-desc">
          How the assistant behaves when the window is closed or idle. Hiding to the tray follows
          the "Show in notification area" setting in Settings → General.
        </p>
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

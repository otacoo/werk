import { useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Download, FolderOpen, ImagePlus, Mic, Play, RefreshCw, Square, X } from "lucide-react";
import { commands } from "../bindings";
import type { AssistantConfig, MemoryFileDto, Reminder } from "../bindings";
import { call } from "../utils/ipc";
import { subscribeConfigChanged } from "../utils/appSettings";
import {
  getOverlayAnimations,
  getOverlayOpacity,
  OVERLAY_OPACITY_MIN,
  setOverlayAnimations,
  setOverlayOpacity,
} from "../utils/overlayPrefs";
import { VoiceRecorder } from "../utils/recorder";
import Toggle from "../components/Toggle";
import ProfileAvatar from "../components/ProfileAvatar";
import { EFFORT_LABELS } from "./chat/external-controls";
import { SkillsCard } from "./agent/memory";

type Tab = "persona" | "reminders" | "memory" | "voice" | "access" | "behavior";

/// Assistant settings: persona, reminders, memory, access, and behavior.
/// The conversation lives in its own Chat tab.
export default function Assistant({ active = true }: { active?: boolean }) {
  const [tab, setTab] = useState<Tab>("persona");
  const [persona, setPersona] = useState<AssistantConfig | null>(null);
  const [avatar, setAvatar] = useState<string | null>(null);
  const [builtInPrompt, setBuiltInPrompt] = useState("");
  const [personaError, setPersonaError] = useState<string | null>(null);

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

  /// Persist persona edits immediately; an override equal to the built-in
  /// prompt is stored as none.
  const savePersona = async (next: AssistantConfig) => {
    const prompt = next.system_prompt?.trim() ?? "";
    const toSave = {
      ...next,
      system_prompt:
        prompt === "" || prompt === builtInPrompt.trim() ? null : next.system_prompt,
    };
    setPersona(toSave);
    setPersonaError(null);
    try {
      await call(commands.setAssistantConfig(toSave));
    } catch (e) {
      setPersonaError(String(e));
      refreshConfig();
    }
  };

  const TABS: { id: Tab; label: string }[] = [
    { id: "persona", label: "Persona" },
    { id: "reminders", label: "Reminders" },
    { id: "memory", label: "Memory" },
    { id: "voice", label: "Voice" },
    { id: "access", label: "Access" },
    { id: "behavior", label: "Behavior" },
  ];

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-6xl mx-auto">
        <div>
          <h1 className="section-title">Assistant</h1>
          <p className="section-desc">
            Persona, memory, reminders, and everything the assistant can do.
          </p>
        </div>

        <div className="flex items-center gap-0.5">
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
          <div className="grid grid-cols-2 gap-4 items-start">
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
                  onBlur={() => savePersona(persona)}
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
                  onBlur={() => savePersona(persona)}
                />
              </label>
            </div>
            <div className="space-y-4">
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
                  onBlur={() => savePersona(persona)}
                />
                <div className="flex items-center gap-2">
                  <button
                    className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                    onClick={() => savePersona({ ...persona, system_prompt: null })}
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
                  <p className="section-desc">
                    Per-request overrides; empty uses the server default.
                  </p>
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
                    onBlur={() => savePersona(persona)}
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
                    onBlur={() => savePersona(persona)}
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
                    onBlur={() => savePersona(persona)}
                  />
                  </label>
                </div>
                <label className="block">
                  <span className="text-[0.6875rem] text-dim">Reasoning effort</span>
                  <select
                    className="input w-full mt-1"
                    value={persona.reasoning_effort ?? ""}
                    onChange={(e) =>
                      savePersona({ ...persona, reasoning_effort: e.target.value || null })
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
              {personaError && <p className="text-xs text-accent-red">{personaError}</p>}
            </div>
          </div>
        )}

        {tab === "memory" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <AssistantMemoryCard />
            <SkillsCard />
          </div>
        )}

        {tab === "reminders" && <RemindersCard />}

        {tab === "voice" && <VoiceCard />}

        {tab === "access" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <SystemControlCard />
            <div className="space-y-4">
              <BuiltInToolsCard />
              <FileSystemCard />
            </div>
          </div>
        )}

        {tab === "behavior" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <div className="space-y-4">
              <BehaviorCard />
              <LocalServerCard />
            </div>
            <OverlayCard />
          </div>
        )}
      </div>
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

/// Opt-in system control: master switch and per-tool toggles. Tools act
/// immediately; the File system card is the boundary. Changes apply at once.
function SystemControlCard() {
  const [access, setAccess] = useState<{
    system_control: boolean;
    files: boolean;
    clipboard: boolean;
    windows: boolean;
    screen: boolean;
    input: boolean;
    uia: boolean;
    browser: boolean;
    browser_user_profile: boolean;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [os, setOs] = useState("");

  const load = () => {
    call(commands.getPlatformStyle())
      .then((p) => setOs(p.os))
      .catch(() => {});
    call(commands.getConfig())
      .then((c) =>
        setAccess({
          system_control: c.assistant?.system_control ?? false,
          files: c.assistant?.tool_files ?? true,
          clipboard: c.assistant?.tool_clipboard ?? true,
          windows: c.assistant?.tool_windows ?? true,
          screen: c.assistant?.tool_screen ?? true,
          input: c.assistant?.tool_input ?? true,
          uia: c.assistant?.tool_uia ?? true,
          browser: c.assistant?.tool_browser ?? true,
          browser_user_profile: c.assistant?.browser_user_profile ?? false,
        }),
      )
      .catch(() => {});
  };
  useEffect(load, []);

  const apply = async (next: NonNullable<typeof access>) => {
    setAccess(next);
    setError(null);
    try {
      await call(
        commands.setAssistantAccess(
          next.system_control,
          next.files,
          next.clipboard,
          next.windows,
          next.screen,
          next.input,
          next.uia,
          next.browser,
          next.browser_user_profile,
        ),
      );
    } catch (e) {
      setError(String(e));
      load();
    }
  };

  if (!access) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">System control</h2>
        <p className="section-desc">
          Off by default. Warning: Use with caution. Tools enabled here do not require approval.
        </p>
      </div>
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <Toggle
            label="Allow system control"
            hint="Master switch."
            checked={access.system_control}
            onChange={(v) => apply({ ...access, system_control: v })}
          />
        </div>
        <span
          title="System tools can read your screen, control the mouse and keyboard, and change files. Everything outside the File system list is blocked."
          className="shrink-0"
        >
          <AlertTriangle size={14} className="text-accent-yellow mt-0.5" />
        </span>
      </div>
      {access.system_control && (
        <div className="flex flex-col">
          <div className="h-2.5 w-px bg-border ml-4" />
          <div className="w-full space-y-3 border border-border rounded px-3 py-2">
          <Toggle
            label="File tools"
            hint="Read, write, edit, find, search, and trash files in the allowed folders."
            checked={access.files}
            onChange={(v) => apply({ ...access, files: v })}
          />
          <Toggle
            label="Clipboard"
            hint="Read the clipboard and copy text."
            checked={access.clipboard}
            onChange={(v) => apply({ ...access, clipboard: v })}
          />
          <Toggle
            label="Window control"
            hint="List, focus, minimize, maximize, close, and move windows."
            checked={access.windows}
            onChange={(v) => apply({ ...access, windows: v })}
          />
          <Toggle
            label="Screen capture"
            hint="Capture monitors or windows to PNG files."
            checked={access.screen}
            onChange={(v) => apply({ ...access, screen: v })}
          />
          <Toggle
            label="Input control"
            hint="Move the mouse, click, scroll, type, and press keys. The most sensitive tool."
            checked={access.input}
            onChange={(v) => apply({ ...access, input: v })}
          />
          <Toggle
            label={
              os === "macos"
                ? "Accessibility (AX)"
                : os === "linux"
                  ? "Accessibility (AT-SPI)"
                  : "Accessibility (UIA)"
            }
            hint={
              os === "macos"
                ? "Find and press app controls by name without moving the mouse; needs Accessibility permission."
                : os === "linux"
                  ? "Find and activate app controls by name without moving the mouse; needs at-spi2-core."
                  : "Find and invoke app controls by name without moving the mouse."
            }
            checked={access.uia}
            onChange={(v) => apply({ ...access, uia: v })}
          />
          <Toggle
            label="Browser"
            hint="Open and drive Chrome, Edge, or Firefox."
            checked={access.browser}
            onChange={(v) => apply({ ...access, browser: v })}
          />
          {access.browser && (
            <div className="pl-4 ml-1 border-l border-border">
              <Toggle
                label="Use my current browser profile"
                hint="Firefox only: drive your real profile with its tabs and logins. Quit Firefox before the assistant first launches it. Chrome and Edge block remote debugging on the default profile, so they always use an isolated one."
                checked={access.browser_user_profile}
                onChange={(v) => apply({ ...access, browser_user_profile: v })}
              />
            </div>
          )}
          </div>
        </div>
      )}
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

/// The assistant's always-on toolset; independent of system control.
function BuiltInToolsCard() {
  return (
    <div className="card space-y-2">
      <div>
        <h2 className="section-title mb-0">Built-in tools</h2>
        <p className="section-desc">
          Always on: these tools need no switches and no approvals.
        </p>
      </div>
      <div className="space-y-1.5">
        {(
          [
            ["remember", "Curate the assistant's memory."],
            ["web_search", "Search the web (DuckDuckGo)."],
            ["get_time", "Current date and time."],
            ["ask_user", "Ask a multiple-choice question."],
            ["skill", "Load a saved skill."],
            ["skill_write", "Save a reusable procedure as a skill."],
            ["plugin_write", "Create a local-command plugin."],
            ["mcp", "List, add, or remove MCP servers."],
            ["reminder", "Schedule reminders and alerts."],
          ] as const
        ).map(([name, desc]) => (
          <div key={name} className="flex items-baseline gap-2 min-w-0">
            <span className="font-mono text-[0.6875rem] text-ink shrink-0">{name}</span>
            <span className="text-[0.625rem] text-faint truncate">{desc}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/// Where the assistant may read, write, and trash files; everything else is barred.
function FileSystemCard() {
  const [fs, setFs] = useState<{
    workspace: string | null;
    temp_enabled: boolean;
    folders: string[];
    tempPath: string;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    Promise.all([call(commands.getConfig()), call(commands.assistantTempDir())])
      .then(([c, tempPath]) =>
        setFs({
          workspace: c.assistant?.workspace ?? null,
          temp_enabled: c.assistant?.temp_enabled ?? true,
          folders: c.assistant?.folders ?? [],
          tempPath,
        }),
      )
      .catch(() => {});
  };
  useEffect(load, []);

  const apply = async (next: NonNullable<typeof fs>) => {
    setFs(next);
    setError(null);
    try {
      await call(commands.setAssistantFs(next.workspace, next.temp_enabled, next.folders));
    } catch (e) {
      setError(String(e));
      load();
    }
  };

  const pickWorkspace = async () => {
    if (!fs) return;
    let picked: string | string[] | null = null;
    try {
      picked = await openDialog({ directory: true, multiple: false });
    } catch {
      return;
    }
    if (!picked || typeof picked !== "string") return;
    apply({ ...fs, workspace: picked });
  };

  const addFolders = async () => {
    if (!fs) return;
    let picked: string | string[] | null = null;
    try {
      picked = await openDialog({ directory: true, multiple: true });
    } catch {
      return;
    }
    const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (paths.length === 0) return;
    const next = [...fs.folders];
    for (const path of paths) {
      if (path === fs.workspace || next.includes(path)) continue;
      next.push(path);
      if (next.length >= 16) break;
    }
    apply({ ...fs, folders: next });
  };

  if (!fs) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">File system</h2>
        <p className="section-desc">
          Where the assistant may read, write, and trash files. Everything outside these
          locations is completely blocked.
        </p>
      </div>
      <Toggle
        label="Temp working folder"
        hint={`Scratch files, scripts, and screenshots under ${fs.tempPath}.`}
        checked={fs.temp_enabled}
        onChange={(v) => apply({ ...fs, temp_enabled: v })}
      />
      <div className="space-y-2 border border-border rounded px-3 py-2">
        <div className="flex items-center justify-between">
          <p className="text-xs font-medium text-ink">Assistant folder</p>
          <div className="flex items-center gap-1">
            <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={pickWorkspace}>
              <FolderOpen size={11} /> Choose…
            </button>
            {fs.workspace && (
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5 text-accent-red"
                onClick={() => apply({ ...fs, workspace: null })}
              >
                Clear
              </button>
            )}
          </div>
        </div>
        <p className="font-mono text-[0.6875rem] text-dim break-all">
          {fs.workspace ?? "Not set — the assistant has nowhere persistent to keep files."}
        </p>
        <p className="text-[0.625rem] text-faint">
          The assistant's own files live here; it decides what to save. Relative paths resolve
          here.
        </p>
      </div>
      <div className="space-y-2 border border-border rounded px-3 py-2">
        <div className="flex items-center justify-between">
          <p className="text-xs font-medium text-ink">Accessible folders</p>
          <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={addFolders}>
            <FolderOpen size={11} /> Add folder
          </button>
        </div>
        {fs.folders.length === 0 && (
          <p className="text-[0.6875rem] text-dim">None yet.</p>
        )}
        {fs.folders.map((folder) => (
          <div key={folder} className="flex items-center gap-2">
            <span
              className="flex-1 min-w-0 truncate font-mono text-[0.6875rem] text-dim"
              title={folder}
            >
              {folder}
            </span>
            <button
              className="text-faint hover:text-accent-red shrink-0"
              onClick={() => apply({ ...fs, folders: fs.folders.filter((f) => f !== folder) })}
              title="Remove"
            >
              <X size={11} />
            </button>
          </div>
        ))}
        <p className="text-[0.625rem] text-faint">Read, write, and trash access for the assistant.</p>
      </div>
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

/// Always-on behavior: notifications, proactive turns, autostart. Toggles
/// apply immediately.
function BehaviorCard() {
  const [behavior, setBehavior] = useState<{
    notify: boolean;
    proactive: boolean;
    auto_distill: boolean;
    reflection: boolean;
    autostart: boolean;
    overlay_enabled: boolean;
    hotkey: string;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    call(commands.getConfig())
      .then((c) => {
        setBehavior({
          notify: c.assistant?.notify ?? true,
          proactive: c.assistant?.proactive ?? true,
          auto_distill: c.assistant?.auto_distill ?? false,
          reflection: c.assistant?.reflection ?? false,
          autostart: c.assistant?.autostart ?? false,
          overlay_enabled: c.assistant?.overlay_enabled ?? true,
          hotkey: c.assistant?.hotkey ?? "",
        });
      })
      .catch(() => {});
  };
  useEffect(load, []);

  const apply = async (next: NonNullable<typeof behavior>) => {
    setBehavior(next);
    setError(null);
    try {
      await call(
        commands.setAssistantBehavior(
          next.notify,
          next.proactive,
          next.auto_distill,
          next.reflection,
          next.autostart,
          next.overlay_enabled,
          next.hotkey,
        ),
      );
    } catch (e) {
      setError(String(e));
      load();
    }
  };

  if (!behavior) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Behavior</h2>
        <p className="section-desc">
          How the assistant behaves when the window is closed or idle.
        </p>
      </div>
      <Toggle
        label="Reminder alerts"
        hint="Flash the overlay (or taskbar/dock) when a reminder fires."
        checked={behavior.notify}
        onChange={(v) => apply({ ...behavior, notify: v })}
      />
      <Toggle
        label="Proactive messages"
        hint='"message" reminders also start an assistant turn when they fire.'
        checked={behavior.proactive}
        onChange={(v) => apply({ ...behavior, proactive: v })}
      />
      <Toggle
        label="Auto-distill sessions"
        hint="When a session is reset, summarize it into memory with the utility model first."
        checked={behavior.auto_distill}
        onChange={(v) => apply({ ...behavior, auto_distill: v })}
      />
      <Toggle
        label="Periodic reflection"
        hint="Once a day, merge duplicate memories and extract durable facts."
        checked={behavior.reflection}
        onChange={(v) => apply({ ...behavior, reflection: v })}
      />
      <Toggle
        label="Start with the system"
        hint="Launch werk at login, minimized to the tray."
        checked={behavior.autostart}
        onChange={(v) => apply({ ...behavior, autostart: v })}
      />
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

/// Local server lifecycle: the idle unload window (shared with the Agent
/// profile's Local server card).
function LocalServerCard() {
  const [idleDraft, setIdleDraft] = useState("5");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    call(commands.getConfig())
      .then((c) => setIdleDraft(String(c.server_idle_unload_minutes ?? 0)))
      .catch(() => {});
  }, []);

  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Local server</h2>
        <p className="section-desc">
          Stop the server after this many minutes without chat activity; 0 disables.
        </p>
      </div>
      <label className="block">
        <span className="text-[0.6875rem] text-dim">Unload after (minutes, 0 = never) </span>
        <input
          type="number"
          min={0}
          max={1440}
          className="input w-24 mt-1"
          value={idleDraft}
          onChange={(e) => setIdleDraft(e.target.value)}
          onBlur={async () => {
            const n = Math.max(0, Math.min(1440, Number(idleDraft) || 0));
            setIdleDraft(String(n));
            try {
              await call(commands.setServerLifecycle(n));
            } catch (e) {
              setError(String(e));
            }
          }}
        />
      </label>
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

/// Text-to-speech via llama.cpp's llama-tts (Qwen3-TTS): model, reference
/// voice, and a test phrase.
const TTS_REPO = "mradermacher/Qwen3-TTS-12Hz-0.6B-Base-GGUF";
const TTS_MMPROJ = "Qwen3-TTS-12Hz-0.6B-Base.mmproj-Q8_0.gguf";
const TTS_MMPROJ_SIZE = "401 MB";
const TTS_SUGGESTED: { file: string; quant: string; size: string }[] = [
  { file: "Qwen3-TTS-12Hz-0.6B-Base.Q8_0.gguf", quant: "Q8_0", size: "646 MB" },
  { file: "Qwen3-TTS-12Hz-0.6B-Base.Q6_K.gguf", quant: "Q6_K", size: "500 MB" },
  { file: "Qwen3-TTS-12Hz-0.6B-Base.Q4_K_M.gguf", quant: "Q4_K_M", size: "361 MB" },
];

/// Speech-to-text via llama.cpp's llama-mtmd-cli (Qwen3-ASR).
const ASR_REPO = "ggml-org/Qwen3-ASR-0.6B-GGUF";
const ASR_MODEL = "Qwen3-ASR-0.6B-Q8_0.gguf";
const ASR_MMPROJ = "mmproj-Qwen3-ASR-0.6B-Q8_0.gguf";
const ASR_SIZE = "805 MB + 214 MB";

/// Suggested voice models with live download progress. Installing a quant
/// also fetches the matching mmproj and points the config at both.
function VoiceDownloadsCard({
  activeModel,
  onInstalled,
  activeSttModel,
  onSttInstalled,
}: {
  activeModel: string;
  onInstalled: (model: string, mmproj: string) => Promise<void>;
  activeSttModel: string;
  onSttInstalled: (model: string, mmproj: string) => Promise<void>;
}) {
  const [progress, setProgress] = useState<
    Record<string, { downloaded: number; total: number | null }>
  >({});
  const [installing, setInstalling] = useState<{ model: string; mmproj: string } | null>(null);
  const [phase, setPhase] = useState<"model" | "mmproj">("model");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = listen<{ id: string; downloaded: number; total?: number | null }>(
      "download_progress",
      (e) => {
        const p = e.payload;
        setProgress((prev) => ({
          ...prev,
          [p.id]: { downloaded: p.downloaded, total: p.total ?? null },
        }));
      },
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const install = async (
    repo: string,
    modelFile: string,
    mmprojFile: string,
    onDone: (model: string, mmproj: string) => Promise<void>,
  ) => {
    setInstalling({ model: modelFile, mmproj: mmprojFile });
    setPhase("model");
    setError(null);
    try {
      const modelPath = await call(commands.downloadModel(repo, modelFile, null, null));
      setPhase("mmproj");
      const mmprojPath = await call(commands.downloadModel(repo, mmprojFile, null, null));
      await onDone(modelPath, mmprojPath);
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(null);
    }
  };

  const pct = (file: string) => {
    const p = progress[file];
    if (!p || !p.total) return null;
    return Math.min(100, (p.downloaded / p.total) * 100);
  };

  const row = (
    repo: string,
    modelFile: string,
    mmprojFile: string,
    tag: string,
    size: string,
    active: string,
    onDone: (model: string, mmproj: string) => Promise<void>,
  ) => {
    const inUse =
      active === modelFile || active.endsWith(`/${modelFile}`) || active.endsWith(`\\${modelFile}`);
    const busy = installing?.model === modelFile;
    const bar = busy ? (phase === "model" ? pct(modelFile) : pct(mmprojFile)) : null;
    const label = busy ? (phase === "model" ? "Model" : "Audio projector") : tag;
    return (
      <div key={modelFile} className="space-y-1">
        <div className="flex items-center gap-2">
          <span className="badge-blue text-[0.5625rem] shrink-0">{label}</span>
          <span className="text-[0.6875rem] text-dim flex-1 min-w-0 truncate">{size}</span>
          {inUse ? (
            <span className="badge-green text-[0.5625rem] shrink-0">in use</span>
          ) : (
            <button
              className="btn-secondary text-xs py-0.5 px-2 shrink-0"
              disabled={installing !== null}
              onClick={() => void install(repo, modelFile, mmprojFile, onDone)}
              title={modelFile}
            >
              {busy ? <RefreshCw size={11} className="animate-spin" /> : <Download size={11} />}
              {busy ? "Downloading…" : "Download"}
            </button>
          )}
        </div>
        {bar !== null && (
          <div className="h-1 rounded-full bg-surface-3 overflow-hidden">
            <div
              className="h-full bg-accent transition-[width] duration-150"
              style={{ width: `${bar}%` }}
            />
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Model downloads</h2>
        <p className="section-desc">
          Download local voice models; each one fetches everything it needs to run.
        </p>
      </div>
      <div className="space-y-2.5">
        <div>
          <p className="text-xs font-medium text-ink">Qwen3-TTS 0.6B</p>
          <p className="text-[0.6875rem] text-dim">
            Text-to-speech; each download also fetches the audio projector ({TTS_MMPROJ_SIZE}).
          </p>
        </div>
        {TTS_SUGGESTED.map((s) =>
          row(TTS_REPO, s.file, TTS_MMPROJ, s.quant, s.size, activeModel, onInstalled),
        )}
        <div className="border-t border-border pt-2.5">
          <p className="text-xs font-medium text-ink">Qwen3-ASR 0.6B</p>
          <p className="text-[0.6875rem] text-dim">
            Speech-to-text for the overlay microphone; also fetches the audio projector ({ASR_SIZE}).
          </p>
        </div>
        {row(ASR_REPO, ASR_MODEL, ASR_MMPROJ, "Q8_0", ASR_SIZE, activeSttModel, onSttInstalled)}
      </div>
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

/// Records a global shortcut: focus the field (or press Set) and hit the combo.
function HotkeyField({
  label,
  value,
  placeholder,
  disabled,
  onChange,
}: {
  label: string;
  value: string;
  placeholder?: string;
  disabled?: boolean;
  onChange: (v: string) => void;
}) {
  const [capturing, setCapturing] = useState(false);

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setCapturing(false);
        return;
      }
      const accel = acceleratorFromEvent(e);
      if (accel) {
        onChange(accel);
        setCapturing(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [capturing, onChange]);

  return (
    <div className="space-y-1">
      <span className="text-[0.6875rem] text-dim">{label}</span>
      <div className="flex items-center gap-2">
        <input
          className={`input flex-1 min-w-0 font-mono text-xs ${capturing ? "border-accent" : ""}`}
          value={capturing ? "Press keys…" : formatHotkey(value)}
          placeholder={placeholder}
          readOnly
          disabled={disabled}
          onFocus={() => setCapturing(true)}
          onBlur={() => setCapturing(false)}
          title="Click, then press the shortcut"
        />
        <button
          className="btn-secondary text-xs py-1 px-2"
          disabled={disabled}
          onClick={() => setCapturing(true)}
        >
          Set
        </button>
        <button
          className="btn-secondary text-xs py-1 px-2"
          disabled={disabled || !value}
          onClick={() => onChange("")}
        >
          Clear
        </button>
      </div>
    </div>
  );
}

const IS_MAC = /mac/i.test(navigator.userAgent);

/// "CmdOrCtrl+Alt+D" -> "Ctrl+Alt+D" (or "Cmd+Alt+D" on macOS) for display.
function formatHotkey(value: string): string {
  return value.replace(/CmdOrCtrl/gi, IS_MAC ? "Cmd" : "Ctrl");
}

/// Browser key event -> the accelerator syntax the global-shortcut parser takes.
function acceleratorFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("CmdOrCtrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (mods.length === 0) return null; // a bare key would hijack typing
  const key = keyFromCode(e.code);
  return key ? [...mods, key].join("+") : null;
}

function keyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  const named: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    NumpadEnter: "Enter",
    Tab: "Tab",
    Backspace: "Backspace",
    Delete: "Delete",
    Insert: "Insert",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Comma: ",",
    Period: ".",
    Slash: "/",
    Semicolon: ";",
    Quote: "'",
    Minus: "-",
    Equal: "=",
    Backquote: "`",
    BracketLeft: "[",
    BracketRight: "]",
  };
  return named[code] ?? null;
}

/// Transcription languages Qwen3-ASR supports (full names, as it tags them).
const STT_LANGS = [
  "Chinese",
  "English",
  "Cantonese",
  "Arabic",
  "German",
  "French",
  "Spanish",
  "Portuguese",
  "Indonesian",
  "Italian",
  "Korean",
  "Russian",
  "Thai",
  "Vietnamese",
  "Japanese",
  "Turkish",
  "Hindi",
  "Malay",
  "Dutch",
  "Swedish",
  "Danish",
  "Finnish",
  "Polish",
  "Czech",
  "Filipino",
  "Persian",
  "Greek",
  "Hungarian",
  "Macedonian",
  "Romanian",
];

function VoiceCard() {
  const [voice, setVoice] = useState<{
    enabled: boolean;
    model: string;
    mmproj: string;
    speaker: string;
    lang: string;
    autoplay: boolean;
  } | null>(null);
  const [stt, setStt] = useState<{
    enabled: boolean;
    model: string;
    mmproj: string;
    hotkey: string;
    lang: string;
  } | null>(null);
  const [testText, setTestText] = useState("Hello! This is my assistant voice.");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastPath, setLastPath] = useState<string | null>(null);
  const [recording, setRecording] = useState(false);
  const [transcript, setTranscript] = useState<string | null>(null);
  const [sttError, setSttError] = useState<string | null>(null);
  const recorder = useRef(new VoiceRecorder());

  const load = () => {
    call(commands.getConfig())
      .then((c) => {
        setVoice({
          enabled: c.assistant?.tts_enabled ?? false,
          model: c.assistant?.tts_model ?? "",
          mmproj: c.assistant?.tts_mmproj ?? "",
          speaker: c.assistant?.tts_speaker ?? "",
          lang: c.assistant?.tts_lang ?? "en",
          autoplay: c.assistant?.tts_autoplay ?? false,
        });
        setStt({
          enabled: c.assistant?.stt_enabled ?? false,
          model: c.assistant?.stt_model ?? "",
          mmproj: c.assistant?.stt_mmproj ?? "",
          hotkey: c.assistant?.stt_hotkey ?? "",
          lang: c.assistant?.stt_lang ?? "",
        });
      })
      .catch(() => {});
  };
  useEffect(load, []);

  const applyStt = async (next: NonNullable<typeof stt>) => {
    setStt(next);
    setSttError(null);
    try {
      await call(
        commands.setAssistantStt(
          next.enabled,
          next.model || null,
          next.mmproj || null,
          next.hotkey,
          next.lang,
        ),
      );
    } catch (e) {
      setSttError(String(e));
      load();
    }
  };

  const browseStt = async (kind: "model" | "mmproj") => {
    if (!stt) return;
    const picked = await openDialog({
      multiple: false,
      directory: false,
      title: "Pick a GGUF file",
      filters: [{ name: "GGUF", extensions: ["gguf"] }],
    }).catch(() => null);
    if (typeof picked === "string" && picked) {
      await applyStt({ ...stt, [kind]: picked });
    }
  };

  /// Record from the mic, transcribe, and show the text.
  const toggleRecording = async () => {
    setSttError(null);
    if (recording) {
      setRecording(false);
      setBusy(true);
      try {
        const audio = await recorder.current.stop();
        if (!audio) return;
        const res = await call(commands.assistantSttTranscribe(audio));
        setTranscript(res.text || "(nothing heard)");
      } catch (e) {
        setSttError(String(e));
      } finally {
        setBusy(false);
      }
    } else {
      try {
        await recorder.current.start();
        setRecording(true);
      } catch (e) {
        setSttError(`Microphone unavailable: ${e}`);
      }
    }
  };

  const apply = async (next: NonNullable<typeof voice>) => {
    setVoice(next);
    setError(null);
    try {
      await call(
        commands.setAssistantVoice(
          next.enabled,
          next.model || null,
          next.mmproj || null,
          next.speaker || null,
          next.lang,
          next.autoplay,
        ),
      );
    } catch (e) {
      setError(String(e));
      load();
    }
  };

  const browse = async (kind: "model" | "mmproj" | "speaker") => {
    if (!voice) return;
    const picked = await openDialog({
      multiple: false,
      directory: false,
      title:
        kind === "speaker" ? "Pick a reference voice" : "Pick a GGUF file",
      filters:
        kind === "speaker"
          ? [{ name: "Audio", extensions: ["wav", "mp3", "flac", "ogg", "m4a"] }]
          : [{ name: "GGUF", extensions: ["gguf"] }],
    }).catch(() => null);
    if (typeof picked === "string" && picked) {
      await apply({ ...voice, [kind]: picked });
    }
  };

  const speak = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await call(commands.assistantTtsSpeak(testText));
      setLastPath(res.path);
      await new Audio(`data:audio/wav;base64,${res.audio}`).play();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!voice || !stt) return null;
  return (
    <div className="grid grid-cols-2 gap-4 items-start">
      <div className="space-y-4">
      <div className="card space-y-3">
        <div>
          <h2 className="section-title mb-0">Voice</h2>
          <p className="section-desc">
            Speak & hear locally with llama.cpp's llama-tts. Nothing leaves
            the machine.
          </p>
        </div>
        <Toggle
          label="Enable voice"
          hint="Allow speaking replies and narration."
          checked={voice.enabled}
          onChange={(v) => apply({ ...voice, enabled: v })}
        />
        {voice.enabled && (
          <>
            <div className="space-y-1">
              <span className="text-[0.6875rem] text-dim">TTS model (GGUF)</span>
              <div className="flex items-center gap-2">
                <input
                  className="input flex-1 min-w-0 font-mono text-xs"
                  placeholder="…/Qwen3-TTS-12Hz-0.6B-Base-Q8_0.gguf"
                  value={voice.model}
                  onChange={(e) => setVoice({ ...voice, model: e.target.value })}
                  onBlur={() => apply(voice)}
                />
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => void browse("model")}
                  title="Browse"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
            <div className="space-y-1">
              <span className="text-[0.6875rem] text-dim">
                Audio projector (mmproj, ships with the TTS GGUF)
              </span>
              <div className="flex items-center gap-2">
                <input
                  className="input flex-1 min-w-0 font-mono text-xs"
                  placeholder="…/mmproj-Qwen3-TTS-….gguf"
                  value={voice.mmproj}
                  onChange={(e) => setVoice({ ...voice, mmproj: e.target.value })}
                  onBlur={() => apply(voice)}
                />
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => void browse("mmproj")}
                  title="Browse"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
            <div className="space-y-1">
              <span className="text-[0.6875rem] text-dim">
                Reference voice (optional, for cloning)
              </span>
              <div className="flex items-center gap-2">
                <input
                  className="input flex-1 min-w-0 font-mono text-xs"
                  placeholder="speaker.wav"
                  value={voice.speaker}
                  onChange={(e) => setVoice({ ...voice, speaker: e.target.value })}
                  onBlur={() => apply(voice)}
                />
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => void browse("speaker")}
                  title="Browse"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
            <label className="block">
              <span className="text-[0.6875rem] text-dim">Language</span>
              <select
                className="input w-full mt-1 text-xs"
                value={voice.lang}
                onChange={(e) => apply({ ...voice, lang: e.target.value })}
              >
                {["en", "de", "es", "fr", "it", "pt", "ru", "zh", "ja", "ko"].map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
            <Toggle
              label="Narrate replies"
              hint="Speak each assistant reply as soon as it finishes."
              checked={voice.autoplay}
              onChange={(v) => apply({ ...voice, autoplay: v })}
            />
          </>
        )}
        {error && <p className="text-xs text-accent-red">{error}</p>}
      </div>
      <div className="card space-y-3">
        <div>
          <h2 className="section-title mb-0">Voice input</h2>
          <p className="section-desc">
            Dictate with the overlay microphone; Qwen3-ASR transcribes locally.
          </p>
        </div>
        <Toggle
          label="Enable voice input"
          hint="Shows a microphone in the overlay pill."
          checked={stt.enabled}
          onChange={(v) => applyStt({ ...stt, enabled: v })}
        />
        {stt.enabled && (
          <>
            <div className="space-y-1">
              <span className="text-[0.6875rem] text-dim">STT model (GGUF)</span>
              <div className="flex items-center gap-2">
                <input
                  className="input flex-1 min-w-0 font-mono text-xs"
                  placeholder="…/Qwen3-ASR-0.6B-Q8_0.gguf"
                  value={stt.model}
                  onChange={(e) => setStt({ ...stt, model: e.target.value })}
                  onBlur={() => applyStt(stt)}
                />
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => void browseStt("model")}
                  title="Browse"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
            <div className="space-y-1">
              <span className="text-[0.6875rem] text-dim">
                Audio projector (mmproj, ships with the STT GGUF)
              </span>
              <div className="flex items-center gap-2">
                <input
                  className="input flex-1 min-w-0 font-mono text-xs"
                  placeholder="…/mmproj-Qwen3-ASR-….gguf"
                  value={stt.mmproj}
                  onChange={(e) => setStt({ ...stt, mmproj: e.target.value })}
                  onBlur={() => applyStt(stt)}
                />
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  onClick={() => void browseStt("mmproj")}
                  title="Browse"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
            </div>
            <label className="block">
              <span className="text-[0.6875rem] text-dim">
                Language (auto-detected when empty)
              </span>
              <select
                className="input w-full mt-1 text-xs"
                value={stt.lang}
                onChange={(e) => applyStt({ ...stt, lang: e.target.value })}
              >
                <option value="">Auto</option>
                {STT_LANGS.map((l) => (
                  <option key={l} value={l}>
                    {l}
                  </option>
                ))}
              </select>
            </label>
            <HotkeyField
              label="Dictation hotkey (empty disables)"
              value={stt.hotkey}
              placeholder="Ctrl+Alt+D"
              onChange={(v) => applyStt({ ...stt, hotkey: v })}
            />
            <div className="flex items-center gap-2">
              <button
                className={`${recording ? "btn-danger" : "btn-secondary"} text-xs py-1 px-2 shrink-0`}
                disabled={busy}
                onClick={() => void toggleRecording()}
              >
                {busy ? (
                  <RefreshCw size={12} className="animate-spin" />
                ) : recording ? (
                  <Square size={12} />
                ) : (
                  <Mic size={12} />
                )}
                {busy ? "Transcribing…" : recording ? "Stop & transcribe" : "Record test"}
              </button>
              {transcript && (
                <span className="text-xs text-dim flex-1 min-w-0 truncate" title={transcript}>
                  {transcript}
                </span>
              )}
            </div>
            {sttError && <p className="text-xs text-accent-red">{sttError}</p>}
          </>
        )}
      </div>
      </div>
      <div className="space-y-4">
        {voice.enabled && (
          <div className="card space-y-3">
            <div>
              <h2 className="section-title mb-0">Test voice</h2>
              <p className="section-desc">Synthesize a phrase and play it back.</p>
            </div>
            <textarea
              className="input w-full text-xs min-h-[4rem]"
              value={testText}
              onChange={(e) => setTestText(e.target.value)}
            />
            <div className="flex items-center gap-2">
              <button
                className="btn-secondary text-xs py-1 px-2"
                disabled={busy}
                onClick={() => void speak()}
              >
                {busy ? <RefreshCw size={12} className="animate-spin" /> : <Play size={12} />}
                {busy ? "Synthesizing…" : "Speak"}
              </button>
              {lastPath && <span className="text-[0.625rem] text-faint truncate">{lastPath}</span>}
            </div>
            <p className="text-[0.625rem] text-faint">
              The first run loads the model, so it can take a few seconds.
            </p>
          </div>
        )}
        <VoiceDownloadsCard
          activeModel={voice.model}
          onInstalled={(model, mmproj) => apply({ ...voice, enabled: true, model, mmproj })}
          activeSttModel={stt.model}
          onSttInstalled={(model, mmproj) => applyStt({ ...stt, enabled: true, model, mmproj })}
        />
      </div>
    </div>
  );
}

/// The always-on-top overlay: visibility, summon hotkey, and a preview.
/// Changes apply immediately.
function OverlayCard() {
  const [overlay, setOverlay] = useState<{
    enabled: boolean;
    hotkey: string;
    notify: boolean;
    proactive: boolean;
    auto_distill: boolean;
    reflection: boolean;
    autostart: boolean;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opacity, setOpacity] = useState(getOverlayOpacity);
  const [animations, setAnimations] = useState(getOverlayAnimations);

  const load = () => {
    call(commands.getConfig())
      .then((c) =>
        setOverlay({
          enabled: c.assistant?.overlay_enabled ?? true,
          hotkey: c.assistant?.hotkey ?? "",
          notify: c.assistant?.notify ?? true,
          proactive: c.assistant?.proactive ?? true,
          auto_distill: c.assistant?.auto_distill ?? false,
          reflection: c.assistant?.reflection ?? false,
          autostart: c.assistant?.autostart ?? false,
        }),
      )
      .catch(() => {});
  };
  useEffect(load, []);

  const apply = async (next: NonNullable<typeof overlay>) => {
    setOverlay(next);
    setError(null);
    try {
      await call(
        commands.setAssistantBehavior(
          next.notify,
          next.proactive,
          next.auto_distill,
          next.reflection,
          next.autostart,
          next.enabled,
          next.hotkey,
        ),
      );
    } catch (e) {
      setError(String(e));
      load();
    }
  };

  if (!overlay) return null;
  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Overlay</h2>
        <p className="section-desc">
          A pulsating circle that floats above the desktop while the assistant profile is
          active; click it (or press the hotkey) to type an instruction.
        </p>
      </div>
      <Toggle
        label="Show the overlay"
        hint="Off hides the circle, the hotkey, and reminder flashes on it."
        checked={overlay.enabled}
        onChange={(v) => apply({ ...overlay, enabled: v })}
      />
      <HotkeyField
        label="Summon hotkey (empty disables)"
        value={overlay.hotkey}
        placeholder="Ctrl+Alt+Space"
        disabled={!overlay.enabled}
        onChange={(v) => apply({ ...overlay, hotkey: v })}
      />
      <div className="space-y-3 border-t border-border pt-3">
        <p className="text-xs font-medium text-ink">Appearance</p>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Opacity</span>
          <div className="flex items-center gap-2 mt-1">
            <input
              type="range"
              min={OVERLAY_OPACITY_MIN}
              max={100}
              step={5}
              value={opacity}
              disabled={!overlay.enabled}
              onChange={(e) => {
                const v = Number(e.target.value);
                setOpacity(v);
                setOverlayOpacity(v);
              }}
              className="flex-1 accent-accent"
            />
            <span className="text-xs font-mono text-ink w-10 text-right">{opacity}%</span>
          </div>
        </label>
        <Toggle
          label="Disable animations"
          hint="Assistant won't emote."
          checked={!animations}
          onChange={(v) => {
            setAnimations(!v);
            setOverlayAnimations(!v);
          }}
        />
      </div>
      <div className="flex items-center gap-2">
        <button
          className="btn-secondary text-xs py-1 px-2"
          disabled={!overlay.enabled}
          onClick={() => call(commands.setOverlayVisible(true)).catch(() => {})}
          title="Show the overlay now (the assistant profile must be active)"
        >
          Show now
        </button>
        <span className="text-[0.625rem] text-faint">
          The overlay only appears with the assistant profile.
        </span>
      </div>
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

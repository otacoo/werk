import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  ArrowUpCircle,
  Brain,
  Download,
  ExternalLink,
  FolderOpen,
  Info,
  Palette,
  Plus,
  RefreshCw,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";
import { commands } from "../bindings";
import type { AppConfig, MemoryFileDto, SkillDto, SystemPromptPreset } from "../bindings";
import { call } from "../utils/ipc";
import { useAppConfig } from "../utils/useAppConfig";
import Toggle from "./Toggle";
import { THEME_OPTIONS, ThemeIcon } from "./ThemeIcon";
import { notifyConfigChanged } from "../utils/appSettings";
import { playNotificationSound } from "../utils/sounds";
import {
  FONT_SIZES,
  accentPalette,
  getAccent,
  getCorners,
  getFont,
  getShowToolSnippets,
  getShowFileTree,
  getShowRunChanges,
  getTheme,
  getBubbleAlign,
  setAccent,
  setCorners,
  setFont,
  setShowToolSnippets,
  setShowFileTree,
  setShowRunChanges,
  setTheme,
  setBubbleAlign,
  subscribeAppearance,
  subscribeShowToolSnippets,
  subscribeShowFileTree,
  subscribeShowRunChanges,
  subscribeBubbleAlign,
  type AccentName,
  type CornersMode,
  type FontSizeName,
  type ThemeName,
} from "../utils/appearance";

const REPO_URL = "https://github.com/otacoo/werk";

export type SettingsSection = "general" | "agent" | "appearance" | "about";

function BehaviorCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  return (
    <div className="card">
      <h2 className="section-title mb-1">Verification nudges</h2>
      <p className="section-desc">
        Tell the model to verify its claims before committing; especially useful for smaller
        models. Off disables claim checks; thorough also asks to check read claims.
      </p>
      <select
        className="input w-full py-1 px-2 text-xs mt-3"
        value={appConfig?.verify_mode ?? "normal"}
        onChange={async (e) => {
          const mode = e.target.value as "off" | "normal" | "thorough";
          setAppConfig((c) => (c ? { ...c, verify_mode: mode } : c));
          try {
            await call(commands.setVerifyMode(mode));
          } catch {}
        }}
      >
        <option value="off">Off</option>
        <option value="normal">High-stakes only</option>
        <option value="thorough">Thorough</option>
      </select>
    </div>
  );
}

function UtilityModelCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  const favorites = appConfig?.provider_favorites ?? [];
  const roles: { value: string; label: string }[] = [];
  if (appConfig?.server_mode === "router") {
    if (appConfig.harness_roles?.orchestrator) {
      roles.push({ value: "orchestrator", label: "Local: orchestrator" });
    }
    if (appConfig.harness_roles?.worker) {
      roles.push({ value: "worker", label: "Local: worker" });
    }
  }
  return (
    <div className="card">
      <h2 className="section-title mb-1">Utility model</h2>
      <p className="section-desc">
        Runs housekeeping turns — context compaction and /distill — keeping them off your main
        model. Favorites come from the Mode tab.
      </p>
      <select
        className="input w-full py-1 px-2 text-xs mt-3"
        value={appConfig?.utility_target ?? ""}
        onChange={async (e) => {
          const target = e.target.value || null;
          setAppConfig((c) => (c ? { ...c, utility_target: target } : c));
          try {
            await call(commands.setUtilityTarget(target));
          } catch {}
        }}
      >
        <option value="">Same as the main model</option>
        {roles.map((r) => (
          <option key={r.value} value={r.value}>{r.label}</option>
        ))}
        {favorites.map((t) => (
          <option key={t} value={t}>{t}</option>
        ))}
      </select>
    </div>
  );
}

function SystemPromptCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  const [promptDraft, setPromptDraft] = useState<string | null>(null);  const [builtInPrompt, setBuiltInPrompt] = useState("");
  const [renaming, setRenaming] = useState<{ name: string; value: string } | null>(null);

  useEffect(() => {
    call(commands.getHarnessSystemPromptDefault()).then(setBuiltInPrompt).catch(() => {});
    setPromptDraft(null);
  }, []);

  const storedPrompt = appConfig?.harness_system_prompt ?? "";
  const shownBase = storedPrompt !== "" ? storedPrompt : builtInPrompt;
  const promptDirty = promptDraft !== null && promptDraft.trim() !== shownBase.trim();
  const presets = appConfig?.system_prompt_presets ?? [];

  // The active preset follows the stored prompt: the default, a preset whose
  // prompt matches, or a custom override (null).
  const activePreset = (() => {
    if (storedPrompt === "") return "default";
    const hit = presets.find((p) => p.prompt.trim() === storedPrompt.trim());
    return hit ? hit.name : null;
  })();

  const savePresets = (next: SystemPromptPreset[]) => {
    setAppConfig((c) => (c ? { ...c, system_prompt_presets: next } : c));
    call(commands.setSystemPromptPresets(next)).catch(() => {});
  };

  // Clicking a preset activates it right away; Save only stores edits.
  const activate = (prompt: string) => {
    setPromptDraft(null);
    setAppConfig((c) => (c ? { ...c, harness_system_prompt: prompt.trim() || null } : c));
    call(commands.setSystemPrompt(prompt)).catch(() => {});
  };

  const addPreset = () => {
    if (presets.length >= 5) return;
    const name = `Preset ${presets.length + 1}`;
    const prompt = (promptDraft ?? shownBase).trim();
    savePresets([...presets, { name, prompt }]);
    activate(prompt);
  };

  const renamePreset = (oldName: string, newName: string) => {
    const trimmed = newName.trim();
    if (!trimmed || trimmed === oldName) return;
    savePresets(presets.map((p) => (p.name === oldName ? { ...p, name: trimmed } : p)));
  };

  const deletePreset = (name: string) => {
    savePresets(presets.filter((p) => p.name !== name));
  };

  const presetButton = (p: SystemPromptPreset) => (
    <button
      key={p.name}
      className={`px-2.5 py-1 text-xs rounded transition-colors ${
        activePreset === p.name ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
      }`}
      onClick={(e) => {
        if (e.shiftKey) {
          deletePreset(p.name);
          return;
        }
        activate(p.prompt);
      }}
      onDoubleClick={() => setRenaming({ name: p.name, value: p.name })}
      title={`${p.name} — click to activate, double-click to rename, shift-click to delete`}
    >
      {p.name}
    </button>
  );

  return (
    <div className="card">
      <h2 className="section-title mb-1">System prompt</h2>
      <p className="section-desc">
        Werk ships a built-in system prompt that is updated dynamically for your OS, mode, and the
        tools available; the project directory and saved memory are appended to custom prompts and
        presets as well.
        <br />
        Click a preset to activate it; edit and Save to update it. Clearing and saving restores the
        built-in default.
      </p>
      <div className="flex flex-wrap items-center gap-1.5 mt-3">
        <button
          className={`px-2.5 py-1 text-xs rounded transition-colors ${
            activePreset === "default" ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
          }`}
          onClick={() => activate("")}
          title="Use the built-in default prompt"
        >
          Default
        </button>
        {presets.map((p) =>
          renaming?.name === p.name ? (
            <input
              key={p.name}
              autoFocus
              className="input px-2 py-1 text-xs w-28"
              value={renaming.value}
              onChange={(e) => setRenaming({ name: p.name, value: e.target.value })}
              onBlur={() => {
                renamePreset(p.name, renaming.value);
                setRenaming(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                if (e.key === "Escape") setRenaming(null);
              }}
            />
          ) : (
            presetButton(p)
          ),
        )}
        {presets.length < 5 && (
          <button
            className="px-2 py-1 text-dim hover:text-ink hover:bg-accent/10 rounded transition-colors"
            onClick={addPreset}
            title="Add a preset from the current prompt (max 5)"
          >
            <Plus size={12} />
          </button>
        )}
      </div>
      <textarea
        className="input w-full mt-3 font-mono text-xs leading-relaxed"
        rows={8}
        value={promptDraft ?? shownBase}
        onChange={(e) => setPromptDraft(e.target.value)}
      />
      <div className="flex items-center gap-2 mt-2">
        <button
          className="btn-primary text-xs"
          disabled={!promptDirty}
          onClick={() => {
            const v = (promptDraft ?? "").trim();
            if (activePreset && activePreset !== "default") {
              // Update the active preset in place and keep it active.
              savePresets(presets.map((p) => (p.name === activePreset ? { ...p, prompt: v } : p)));
              activate(v);
              return;
            }
            const next = v === "" || v === builtInPrompt.trim() ? "" : v;
            activate(next);
          }}
        >
          Save
        </button>
        {activePreset === "default" && !promptDirty && (
          <span className="text-[0.6875rem] text-faint">Using built-in default</span>
        )}
        {activePreset === null && (
          <span className="text-[0.6875rem] text-dim">Custom prompt active</span>
        )}
        {activePreset && activePreset !== "default" && (
          <span className="text-[0.6875rem] text-dim">Prompt "{activePreset}" active</span>
        )}
        <span
          className="ml-auto text-[0.6875rem] text-faint"
          title="Rough estimate from the character count; the model's tokenizer may differ."
        >
          ~{Math.ceil((promptDraft ?? shownBase).length / 4)} tokens
        </span>
      </div>
    </div>
  );
}

/// Visibility of the agent's own instruction files for every tool.
function AgentFilesCard({ appConfig, setAppConfig, refresh }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const hidden = appConfig?.agent_files_hidden ?? false;

  const setVisible = async (visible: boolean) => {
    setAppConfig((c) => (c ? { ...c, agent_files_hidden: !visible } : c));
    try {
      await call(commands.setAgentFilesHidden(!visible));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await refresh();
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Agent files</h2>
      <p className="section-desc">
        <span className="font-mono">AGENTS.md</span> and <span className="font-mono">.agent*</span>{" "}
        files carry the agent's own instructions. When hidden, every tool — reads, writes, edits,
        globs, and content search — skips them for the next run.
      </p>
      <div className="mt-3">
        <Toggle label="Visible to the agent" checked={!hidden} onChange={setVisible} />
      </div>
      {error && <p className="text-[0.625rem] text-accent-red mt-2 break-words">{error}</p>}
    </div>
  );
}

function MemoryCard() {
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

function SkillsCard() {
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

function GeneralCard({ appConfig, setAppConfig, refresh }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [showSnippets, setShowSnippets] = useState(getShowToolSnippets());
  useEffect(() => subscribeShowToolSnippets(setShowSnippets), []);
  const [bubbleAlign, setBubbleAlignState] = useState(getBubbleAlign());
  useEffect(() => subscribeBubbleAlign(setBubbleAlignState), []);
  const [showFiles, setShowFilesState] = useState(getShowFileTree());
  useEffect(() => subscribeShowFileTree(setShowFilesState), []);
  const [showChanges, setShowChangesState] = useState(getShowRunChanges());
  useEffect(() => subscribeShowRunChanges(setShowChangesState), []);
  const confirm = async (apply: (c: AppConfig) => AppConfig, save: () => Promise<unknown>) => {
    setAppConfig((c) => (c ? apply(c) : c));
    try {
      await save();
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
      await refresh();
    }
    // Pages holding config snapshots (Run, Chat) refetch instead of going stale.
    notifyConfigChanged();
  };
  const setSound = async (key: "agent" | "permissions" | "errors", v: boolean) => {
    const cmd =
      key === "agent"
        ? commands.setSoundAgent(v)
        : key === "permissions"
          ? commands.setSoundPermissions(v)
          : commands.setSoundErrors(v);
    const apply = (c: AppConfig) =>
      key === "agent"
        ? { ...c, sound_agent: v }
        : key === "permissions"
          ? { ...c, sound_permissions: v }
          : { ...c, sound_errors: v };
    await confirm(apply, () => call(cmd));
    if (v) void playNotificationSound(key);
  };

  const rows: { key: "agent" | "permissions" | "errors"; label: string; hint: string }[] = [
    { key: "agent", label: "Agent", hint: "Play a sound when the agent finishes a prompt." },
    { key: "permissions", label: "Permissions", hint: "Play a sound when the agent needs your attention." },
    { key: "errors", label: "Errors", hint: "Play a sound when an error occurs." },
  ];
  const checked = (key: "agent" | "permissions" | "errors") =>
    key === "agent"
      ? (appConfig?.sound_agent ?? true)
      : key === "permissions"
        ? (appConfig?.sound_permissions ?? true)
        : (appConfig?.sound_errors ?? true);

  return (
    <>
      <div className="card">
        <h2 className="section-title mb-1">General</h2>
        <p className="section-desc">Core app behavior.</p>
        <div className="space-y-3 mt-3">
          <Toggle
            label="Show in notification area"
            hint="Closing hides werk. to the tray instead of quitting."
            checked={appConfig?.close_to_tray ?? false}
            onChange={(v) =>
              confirm((c) => ({ ...c, close_to_tray: v }), () => call(commands.setCloseToTray(v)))
            }
          />
          <Toggle
            label="Enable Quick Bench"
            hint="Toggle benchmarking tools."
            checked={appConfig?.bench_visible ?? true}
            onChange={(v) =>
              confirm((c) => ({ ...c, bench_visible: v }), () => call(commands.setBenchVisible(v)))
            }
          />
          {error && <p className="text-xs text-accent-red">{error}</p>}
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Chat</h2>
        <p className="section-desc">Chat transcript display options.</p>
        <div className="space-y-3 mt-3">
          <Toggle
            label="Show tool call snippets"
            hint="Off keeps collapsed tool cards to name and status; arguments show on expand."
            checked={showSnippets}
            onChange={setShowToolSnippets}
          />
          <Toggle
            label="Show file tree"
            hint="Project file tree beside chat, with change markers. Also toggled with Files in the chat header."
            checked={showFiles}
            onChange={setShowFileTree}
          />
          <Toggle
            label="Show changed files summary"
            hint="After a run, list the git diff for the files the agent changed (max. 5 shown)."
            checked={showChanges}
            onChange={setShowRunChanges}
          />
          <div>
            <p className="label mb-1.5 font-bold text-ink">Chat bubble alignment:</p>
            <div className="flex gap-0.5">
              {(["left", "right"] as const).map((v) => (
                <button
                  key={v}
                  onClick={() => setBubbleAlign(v)}
                  className={`px-2.5 py-1 text-xs font-medium rounded transition-colors ${
                    bubbleAlign === v
                      ? "bg-accent/20 text-ink"
                      : "text-dim hover:text-ink hover:bg-accent/10"
                  }`}
                >
                  {v === "left" ? "Left" : "Right"}
                </button>
              ))}
            </div>
            <p className="section-desc mt-1.5">
              User messages on the {bubbleAlign} side; the model takes the other side.
            </p>
          </div>
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Notifications</h2>
        <p className="section-desc">Sound effects for agent activity.</p>
        <div className="mt-3 divide-y divide-border">
          {rows.map(({ key, label, hint }) => (
            <div key={key} className="py-2.5 first:pt-0 last:pb-0">
              <Toggle label={label} hint={hint} checked={checked(key)} onChange={(v) => setSound(key, v)} />
            </div>
          ))}
        </div>
      </div>
      <UpdatesCard />
    </>
  );
}

function UpdatesCard() {
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [autoCheck, setAutoCheck] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [checking, setChecking] = useState(false);
  const [checked, setChecked] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [downloadPercent, setDownloadPercent] = useState<number | null>(null);
  const [installNote, setInstallNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
    call(commands.getConfig())
      .then((c) => {
        const on = c.auto_check_updates ?? false;
        setAutoCheck(on);
        if (on) void checkForUpdate();
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const checkForUpdate = async () => {
    setChecking(true);
    setChecked(false);
    setError(null);
    try {
      const update = await check();
      setUpdateAvailable(update != null);
      setUpdateVersion(update?.version ?? null);
      setPendingUpdate(update);
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
      setChecked(true);
    }
  };

  const installUpdate = async () => {
    if (!pendingUpdate) return;
    setUpdating(true);
    setDownloadPercent(null);
    setInstallNote(null);
    try {
      let total = 0;
      let downloaded = 0;
      await pendingUpdate.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            downloaded = 0;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            if (total > 0) {
              setDownloadPercent(Math.min(100, (downloaded / total) * 100));
            }
            break;
          case "Finished":
            setDownloadPercent(100);
            break;
        }
      });
      setInstallNote("Installer launched — werk. will close and reopen.");
      await relaunch();
    } catch (e) {
      setError(String(e));
      setUpdating(false);
    }
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <h2 className="section-title mb-0">App Updates</h2>
        {appVersion && (
          <span className="text-xs text-dim tabular-nums">v{appVersion}</span>
        )}
      </div>
      <div className="space-y-3">
        <Toggle
          label="Check for updates on app start"
          checked={autoCheck}
          onChange={async (v) => {
            setAutoCheck(v);
            try {
              await call(commands.setAutoCheckUpdates(v));
            } catch {}
            if (v) void checkForUpdate();
          }}
        />
        <div className="flex flex-wrap items-center gap-3">
          <button
            className="btn-secondary text-xs"
            onClick={checkForUpdate}
            disabled={checking || updating}
          >
            <RefreshCw size={13} className={checking ? "animate-spin" : ""} />
            Check now
          </button>
          {checking && <span className="text-xs text-dim">Checking…</span>}
          {error && <span className="text-xs text-accent-red">{error}</span>}
          {checked && !error && updateAvailable && !updating && (
            <button className="btn-primary text-xs" onClick={installUpdate}>
              <Download size={13} />
              Update to v{updateVersion}
            </button>
          )}
          {checked && !error && updateAvailable && updating && (
            <span className="flex items-center gap-2 text-xs text-dim">
              <ArrowUpCircle size={13} className="text-accent-soft" />
              {downloadPercent != null
                ? `Downloading update… ${downloadPercent.toFixed(0)}%`
                : "Downloading update…"}
            </span>
          )}
          {checked && !error && !updateAvailable && !updating && (
            <span className="text-xs text-accent-green">Up to date</span>
          )}
        </div>
        {updating && downloadPercent != null && (
          <div className="w-full h-1.5 bg-surface-3 rounded overflow-hidden">
            <div
              className="h-full bg-accent transition-all duration-150"
              style={{ width: `${downloadPercent}%` }}
            />
          </div>
        )}
        {installNote && <p className="text-xs text-accent-yellow">{installNote}</p>}
      </div>
    </div>
  );
}


function AppearanceCard() {
  const [theme, setThemeState] = useState<ThemeName>("system");
  const [accent, setAccentState] = useState<AccentName>("blue");
  const [font, setFontState] = useState<FontSizeName>("default");
  const [corners, setCornersState] = useState<CornersMode>("auto");

  useEffect(() => {
    setThemeState(getTheme());
    setAccentState(getAccent());
    setFontState(getFont());
    setCornersState(getCorners());
    const unsubAppearance = subscribeAppearance(() => {
      setThemeState(getTheme());
      setAccentState(getAccent());
      setFontState(getFont());
      setCornersState(getCorners());
    });
    return unsubAppearance;
  }, []);

  const pick = <T extends string>(values: readonly T[], current: T, onPick: (v: T) => void) => (
    <div className="flex flex-wrap gap-1.5">
      {values.map((x) => (
        <button
          key={x}
          onClick={() => onPick(x)}
          className={`px-2.5 py-1 text-xs rounded transition-colors ${
            current === x ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
          }`}
        >
          {x}
        </button>
      ))}
    </div>
  );

  return (
    <>
      <div className="card">
        <h2 className="section-title mb-1">Theme</h2>
        <p className="section-desc">Choose how Werk looks.</p>
        <div className="grid grid-cols-2 gap-2 mt-3">
          {THEME_OPTIONS.map((opt) => {
            const active = theme === opt.value;
            return (
              <button
                key={opt.value}
                onClick={() => {
                  setTheme(opt.value);
                  setThemeState(opt.value);
                }}
                className={`flex flex-col items-center gap-1.5 px-3 py-3 rounded border text-center transition-colors ${
                  active
                    ? "border-accent bg-accent/10 text-ink"
                    : "border-border bg-surface-2 hover:bg-surface-3 text-dim"
                }`}
              >
                <ThemeIcon theme={opt.value} className={active ? "text-accent" : "text-faint"} />
                <span className="text-xs font-medium">{opt.label}</span>
                <span className="text-[0.625rem] text-faint leading-tight">{opt.description}</span>
              </button>
            );
          })}
        </div>
      </div>
      <div className="card">
        <div className="grid grid-cols-2 divide-x divide-border">
          <div className="pr-4">
            <h2 className="section-title mb-1">Accent</h2>
            <p className="section-desc">Highlight color used across the app.</p>
            <div className="flex flex-wrap gap-1.5 mt-2">
              {(Object.keys(accentPalette()) as AccentName[]).map((a) => (
                <button
                  key={a}
                  title={accentPalette()[a].label}
                  onClick={() => {
                    setAccent(a);
                    setAccentState(a);
                  }}
                  className={`w-7 h-7 rounded-full border-2 transition-transform ${
                    accent === a ? "border-ink scale-110" : "border-transparent"
                  }`}
                  style={{ backgroundColor: `rgb(${accentPalette()[a].rgb})` }}
                />
              ))}
            </div>
          </div>
          <div className="pl-4">
            <h2 className="section-title mb-1">Corners</h2>
            <p className="section-desc">Auto follows OS default.</p>
            <div className="mt-2">
              {pick(["auto", "sharp", "round"] as const, corners, (v) => {
                setCorners(v);
                setCornersState(v);
              })}
            </div>
          </div>
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Font</h2>
        <p className="section-desc">Global text size for the whole app.</p>
        <div className="flex flex-wrap gap-1.5 mt-2">
          {(Object.keys(FONT_SIZES) as FontSizeName[]).map((f) => (
            <button
              key={f}
              onClick={() => {
                setFont(f);
                setFontState(f);
              }}
              className={`px-2.5 py-1 text-xs rounded transition-colors ${
                font === f ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              {FONT_SIZES[f].label}
            </button>
          ))}
        </div>
      </div>
    </>
  );
}

export default function SettingsPanel({ open, onClose, section, onSectionChange }: {
  open: boolean;
  onClose: () => void;
  section: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
}) {
  const [appConfig, setAppConfig, configError, refreshConfig] = useAppConfig(open);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  const visible: { id: SettingsSection; label: string; icon: LucideIcon }[] = [
    { id: "general", label: "General", icon: SlidersHorizontal },
    { id: "agent", label: "Agent", icon: Brain },
    { id: "appearance", label: "Appearance", icon: Palette },
    { id: "about", label: "About", icon: Info },
  ];

  return (
    <div className="absolute inset-0 z-40 flex bg-surface-0">
      <aside className="w-44 shrink-0 border-r border-border bg-surface-1 p-3 overflow-y-auto">
        <div className="flex items-center justify-between px-2 pb-3 pt-1">
          <span className="text-sm font-semibold text-ink">Settings</span>
          <button className="text-faint hover:text-ink transition-colors" onClick={onClose} title="Close (Esc)">
            ×
          </button>
        </div>
        <div className="space-y-0.5">
          {visible.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => onSectionChange(id)}
              className={`w-full flex items-center gap-2 px-2.5 py-1.5 rounded text-xs font-medium transition-colors ${
                section === id ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              <Icon size={13} />
              {label}
            </button>
          ))}
        </div>
      </aside>
      <div className="flex-1 overflow-y-auto p-6">
        <div className={section === "agent" ? "space-y-4" : "max-w-3xl space-y-4"}>
          {configError && (
            <div className="card border-accent-red/30 bg-accent-red/5">
              <p className="text-sm text-accent-red">{configError}</p>
            </div>
          )}
          {section === "general" && (
            <GeneralCard appConfig={appConfig} setAppConfig={setAppConfig} refresh={refreshConfig} />
          )}
          {section === "agent" && (
            <div className="grid grid-cols-2 gap-4 items-start">
              <div className="space-y-4">
                <AgentFilesCard
                  appConfig={appConfig}
                  setAppConfig={setAppConfig}
                  refresh={refreshConfig}
                />
                <BehaviorCard appConfig={appConfig} setAppConfig={setAppConfig} />
                <UtilityModelCard appConfig={appConfig} setAppConfig={setAppConfig} />
                <SystemPromptCard appConfig={appConfig} setAppConfig={setAppConfig} />
              </div>
              <div className="space-y-4">
                <SkillsCard />
                <MemoryCard />
              </div>
            </div>
          )}
          {section === "appearance" && <AppearanceCard />}
          {section === "about" && (
            <div className="card text-center">
              <p className="text-3xl font-bold tracking-tight select-none">
                werk<span className="text-accent">.</span>
              </p>
              <p className="text-sm text-dim mt-3">
                A llama.cpp launcher and agent harness, licensed under the{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() => void openUrl(`${REPO_URL}/blob/main/LICENSE`).catch(() => {})}
                >
                  Apache License 2.0
                </button>
              </p>
              <p className="text-xs text-dim mt-4 leading-snug">
                Werk bundles open-source components — among them{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() => void openUrl("https://katex.org").catch(() => {})}
                >
                  KaTeX
                </button>{" "}
                (MIT) for math rendering. The full list, with license texts, is in the{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() =>
                    void openUrl(`${REPO_URL}/blob/main/THIRD-PARTY-NOTICES.md`).catch(() => {})
                  }
                >
                  third-party notices
                </button>
                , shipped alongside the app.
              </p>
              <button
                className="inline-flex items-center gap-1.5 text-xs text-dim hover:text-ink mt-4"
                onClick={() => void openUrl(REPO_URL).catch(() => {})}
              >
                <ExternalLink size={12} /> github.com/otacoo/werk
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

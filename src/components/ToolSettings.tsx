import { useEffect, useState } from "react";
import { FolderOpen, Pencil, Plus, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import { openPath } from "@tauri-apps/plugin-opener";
import { commands } from "../bindings";
import type { AppConfig, LspServerConfig } from "../bindings";
import { call } from "../utils/ipc";
import Toggle from "./Toggle";

export interface BuiltinInfo {
  name: string;
  summary: string;
  approval: string;
  note: string;
  enabled: boolean;
}

export interface PluginInfo {
  name: string;
  description: string;
  scope: string;
  dir: string;
  enabled: boolean;
  approval: boolean;
}

export interface PluginErrorInfo {
  dir: string;
  error: string;
}

/// One shared tools_list load for the agent and plugin cards.
export function useToolsList() {
  const [builtin, setBuiltin] = useState<BuiltinInfo[]>([]);
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [errors, setErrors] = useState<PluginErrorInfo[]>([]);
  const [pluginsDir, setPluginsDir] = useState("");

  const reload = () => {
    call(commands.toolsList())
      .then((t) => {
        setBuiltin(t.builtin as BuiltinInfo[]);
        setPlugins(t.plugins);
        setErrors(t.plugin_errors ?? []);
        setPluginsDir(t.plugins_dir ?? "");
      })
      .catch(() => {});
  };

  useEffect(reload, []);
  return { builtin, plugins, errors, pluginsDir, setPlugins, reload };
}

function badge(approval: string) {
  return approval === "auto" ? (
    <span className="badge-green text-[0.5625rem] shrink-0">auto</span>
  ) : approval === "conditional" ? (
    <span className="badge-blue text-[0.5625rem] shrink-0">conditional</span>
  ) : (
    <span className="badge-yellow text-[0.5625rem] shrink-0">approval</span>
  );
}

/// The agent's own tools (harness side), each switchable for future runs.
export function AgentToolsCard({ builtin, reload }: { builtin: BuiltinInfo[]; reload: () => void }) {
  const [error, setError] = useState<string | null>(null);

  const toggle = async (name: string, on: boolean) => {
    try {
      await call(commands.setAgentToolEnabled(name, on));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    reload();
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Agent tools</h2>
      <p className="section-desc">
        werk's own tools for the chat agent. Changes are applied on the next run.
      </p>
      <div className="space-y-1 mt-3">
        {builtin.map((t) => (
          <div
            key={t.name}
            className={`flex items-center gap-2 border border-border px-2.5 py-1.5 ${t.enabled ? "" : "opacity-60"}`}
          >
            <span className="font-mono text-xs text-ink shrink-0">{t.name}</span>
            {badge(t.approval)}
            <div className="flex-1 min-w-0">
              <p className="text-[0.6875rem] text-dim truncate">{t.summary}</p>
              {t.note && <p className="text-[0.625rem] text-faint truncate">{t.note}</p>}
            </div>
            <Toggle checked={t.enabled} onChange={(on) => toggle(t.name, on)} />
          </div>
        ))}
      </div>
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
    </div>
  );
}

/// stdio language servers: enable toggle, list with editor, restore defaults.
export function LspCard({ appConfig, setAppConfig, refresh, reloadTools }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => Promise<void>;
  reloadTools: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<number | "new" | null>(null);
  const [draftName, setDraftName] = useState("");
  const [draftCommand, setDraftCommand] = useState("");
  const [draftExts, setDraftExts] = useState("");
  const [draftLang, setDraftLang] = useState("");
  const [draftRoots, setDraftRoots] = useState("");

  const servers = appConfig?.lsp_servers ?? [];

  const setLsp = async (enabled: boolean) => {
    setAppConfig((c) => (c ? { ...c, lsp_enabled: enabled } : c));
    if (!enabled) setEditing(null);
    try {
      await call(commands.setLspEnabled(enabled));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await refresh();
    reloadTools();
  };

  const splitList = (raw: string) =>
    raw.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);

  const startEdit = (target: number | "new") => {
    const server = target === "new" ? null : servers[target];
    setDraftName(server?.name ?? "");
    setDraftCommand(server?.command ?? "");
    setDraftExts((server?.extensions ?? []).join(", "));
    setDraftLang(server?.language ?? "");
    setDraftRoots((server?.roots ?? []).join(", "));
    setEditing(target);
  };

  const persistServers = async (list: LspServerConfig[]) => {
    setAppConfig((c) => (c ? { ...c, lsp_servers: list } : c));
    try {
      await call(commands.setLspServers(list));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await refresh();
  };

  const saveServer = async () => {
    const entry: LspServerConfig = {
      name: draftName.trim(),
      command: draftCommand.trim(),
      extensions: splitList(draftExts),
      language: draftLang.trim() || null,
      roots: splitList(draftRoots),
    };
    const list =
      editing === "new" ? [...servers, entry] : servers.map((s, i) => (i === editing ? entry : s));
    await persistServers(list);
    setEditing(null);
  };

  const removeServer = async (index: number) => {
    await persistServers(servers.filter((_, i) => i !== index));
    if (editing === index) setEditing(null);
  };

  const restoreServers = async () => {
    try {
      const list = await call(commands.resetLspServers());
      setAppConfig((c) => (c ? { ...c, lsp_servers: list } : c));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await refresh();
    setEditing(null);
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Language servers</h2>
      <p className="section-desc">
        Enable to connect to a stdio language server.<br></br>
        The program must be on PATH or an absolute path.
      </p>
      <div className="mt-3">
        <Toggle
          label="Enable LSP"
          hint="Diagnostics after writes and precise go-to-definition/references."
          checked={appConfig?.lsp_enabled ?? false}
          onChange={setLsp}
        />
      </div>
      {(appConfig?.lsp_enabled ?? false) && (
        <div className="space-y-1 mt-3">
          {servers.map((s, i) =>
            editing === i ? null : (
              <div key={`${s.name}-${i}`} className="flex items-center gap-2 border border-border px-2.5 py-1.5">
                <span className="font-mono text-xs text-ink shrink-0">{s.name}</span>
                <span className="font-mono text-[0.625rem] text-faint truncate flex-1" title={s.command}>
                  {s.command}
                </span>
                <span className="text-[0.625rem] text-dim shrink-0 font-mono">
                  {(s.extensions ?? []).map((e) => `.${e}`).join(" ")}
                </span>
                <button className="text-faint hover:text-ink shrink-0" onClick={() => startEdit(i)} title="Edit">
                  <Pencil size={11} />
                </button>
                <button
                  className="text-faint hover:text-accent-red shrink-0"
                  onClick={() => removeServer(i)}
                  title="Remove"
                >
                  <Trash2 size={11} />
                </button>
              </div>
            ),
          )}
          {editing !== null && (
            <div className="border border-border p-2.5 space-y-1.5">
              <div className="grid grid-cols-2 gap-2">
                <input
                  className="input py-1 px-2 text-xs"
                  placeholder="Name"
                  value={draftName}
                  onChange={(e) => setDraftName(e.target.value)}
                />
                <input
                  className="input py-1 px-2 text-xs"
                  placeholder="Language id (optional)"
                  value={draftLang}
                  onChange={(e) => setDraftLang(e.target.value)}
                />
              </div>
              <input
                className="input w-full py-1 px-2 text-xs font-mono"
                placeholder="Command — e.g. typescript-language-server --stdio"
                value={draftCommand}
                onChange={(e) => setDraftCommand(e.target.value)}
              />
              <div className="grid grid-cols-2 gap-2">
                <input
                  className="input py-1 px-2 text-xs"
                  placeholder="Extensions: gd, gdshader"
                  value={draftExts}
                  onChange={(e) => setDraftExts(e.target.value)}
                />
                <input
                  className="input py-1 px-2 text-xs"
                  placeholder="Root files: project.godot"
                  value={draftRoots}
                  onChange={(e) => setDraftRoots(e.target.value)}
                />
              </div>
              <div className="flex items-center justify-end gap-2">
                <button className="btn-ghost text-xs py-1 px-2" onClick={() => setEditing(null)}>
                  Cancel
                </button>
                <button
                  className="btn-secondary text-xs py-1 px-2"
                  disabled={!draftName.trim() || !draftCommand.trim() || !draftExts.trim()}
                  onClick={saveServer}
                >
                  Save
                </button>
              </div>
            </div>
          )}
          {editing === null && (
            <div className="flex items-center gap-2 pt-1">
              <button className="btn-secondary text-xs py-1 px-2" onClick={() => startEdit("new")}>
                <Plus size={11} /> Add server
              </button>
              <button
                className="btn-ghost text-xs py-1 px-2"
                onClick={restoreServers}
                title="Restore the built-in server list"
              >
                <RotateCcw size={11} /> Restore defaults
              </button>
            </div>
          )}
        </div>
      )}
      {error && <p className="text-xs text-accent-red mt-2">{error}</p>}
    </div>
  );
}

/// File-defined plugin tools with per-plugin enable switches.
export function PluginToolsCard({ plugins, errors, pluginsDir, setPlugins, reload }: {
  plugins: PluginInfo[];
  errors: PluginErrorInfo[];
  pluginsDir: string;
  setPlugins: React.Dispatch<React.SetStateAction<PluginInfo[]>>;
  reload: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const createPlugin = async () => {
    setError(null);
    setCreating(true);
    try {
      const path = await call(commands.scaffoldPlugin());
      await openPath(path);
      reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Plugin tools</h2>
        <div className="flex items-center gap-1">
          <button
            className="btn-ghost text-[0.625rem]"
            onClick={createPlugin}
            disabled={creating}
            title="Scaffold a commented plugin in your global plugins folder and open it"
          >
            <Plus size={11} /> New plugin
          </button>
          <button className="btn-ghost text-[0.625rem]" onClick={reload} title="Refresh list">
            <RefreshCw size={11} /> Refresh
          </button>
        </div>
      </div>
      <p className="section-desc">
        A folder with a <span className="font-mono">plugin.json</span> plus its executable. Stdin
        gets <span className="font-mono">{"{"}"args": …{"}"}</span>; stdout is the result.
      </p>
      <div className="space-y-1 mt-2">
        {plugins.map((p) => (
          <div key={`${p.scope}:${p.name}`} className="flex items-center gap-2 border border-border px-2.5 py-1.5">
            <span className={`badge-${p.scope === "project" ? "blue" : "gray"} text-[0.5625rem] shrink-0`}>
              {p.scope}
            </span>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <p className="font-mono text-xs text-ink truncate">{p.name}</p>
                {badge(p.approval ? "approval" : "auto")}
              </div>
              {p.description && <p className="text-[0.625rem] text-faint truncate">{p.description}</p>}
            </div>
            <button
              className="text-faint hover:text-ink shrink-0"
              onClick={() => openPath(p.dir).catch(() => {})}
              title="Open the plugin folder"
            >
              <FolderOpen size={12} />
            </button>
            <Toggle
              checked={p.enabled}
              onChange={async (v) => {
                setPlugins((prev) => prev.map((x) => (x.name === p.name ? { ...x, enabled: v } : x)));
                try {
                  await call(commands.pluginSetEnabled(p.name, v));
                } catch {
                  reload();
                }
              }}
            />
          </div>
        ))}
      </div>
      {plugins.length === 0 && (
        <p className="text-[0.6875rem] text-dim mt-2">
          No plugins yet — press New plugin, or drop a single <span className="font-mono">.json</span>
          manifest{pluginsDir ? <> or a folder in <span className="font-mono">{pluginsDir}</span></> : ""} and press Refresh.
        </p>
      )}
      {error && <p className="text-[0.625rem] text-accent-red mt-2 break-words">{error}</p>}
      {errors.length > 0 && (
        <div className="space-y-1 mt-2">
          {errors.map((e) => (
            <p key={e.dir} className="text-[0.625rem] text-accent-red break-words">
              <span className="font-mono">{e.dir}</span>: {e.error}
            </p>
          ))}
        </div>
      )}
    </div>
  );
}

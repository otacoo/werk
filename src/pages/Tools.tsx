import { useEffect, useRef, useState, useCallback } from "react";
import {
  Activity,
  AlertTriangle,
  Bot,
  CheckCircle2,
  ExternalLink,
  FileCode,
  FolderOpen,
  Globe,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands } from "../bindings";
import type { McpServerEntry, McpServerStatus, ServerStatus, ServerToolInfo } from "../bindings";
import { call } from "../utils/ipc";
import { useAppConfig } from "../utils/useAppConfig";
import Toggle from "../components/Toggle";
import { AgentToolsCard, LspCard, PluginToolsCard, useToolsList } from "../components/ToolSettings";
import type { SettingsSection } from "../components/SettingsPanel";
import { KNOWN_TOOLS, toolsArgValue } from "../utils/tools";

const emptyEntry = (): McpServerEntry => ({
  name: "",
  command: "",
  args: [],
  env: {},
  cwd: null,
  timeout_ms: null,
  enabled: true,
});

// ── Text ↔ structured conversions for the MCP editor ───────────────────────

const parseArgs = (text: string): string[] =>
  text.split(/\r?\n/).map((s) => s.trim()).filter(Boolean);

const argsToText = (args: string[] | undefined): string => (args ?? []).join("\n");

const parseEnv = (text: string): Record<string, string> => {
  const env: Record<string, string> = {};
  for (const line of text.split(/\r?\n/)) {
    const idx = line.indexOf("=");
    if (idx > 0) env[line.slice(0, idx).trim()] = line.slice(idx + 1);
  }
  return env;
};

const envToText = (env: Record<string, string> | undefined): string =>
  Object.entries(env ?? {}).map(([k, v]) => `${k}=${v}`).join("\n");

export default function Tools({ active = true, onOpenSettings }: {
  active?: boolean;
  onOpenSettings?: (section: SettingsSection) => void;
}) {
  const [serverStatus, setServerStatus] = useState<ServerStatus>({ type: "stopped" });
  const [appConfig, setAppConfig, , refreshConfig] = useAppConfig(true);
  const toolsList = useToolsList();
  const [mcpPath, setMcpPath] = useState("");
  const [servers, setServers] = useState<McpServerEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const selectedRef = useRef<Set<string>>(new Set());
  const saveTimer = useRef<number | null>(null);

  const [showForm, setShowForm] = useState(false);
  const [editId, setEditId] = useState<string | null>(null);
  const [draft, setDraft] = useState<McpServerEntry>(emptyEntry());
  const [argsText, setArgsText] = useState("");
  const [envText, setEnvText] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Live = what the running server and agent MCP servers currently offer;
  // Agent = run by werk; Server = run by llama-server for WebUI/API clients.
  const [pane, setPane] = useState<"live" | "agent" | "server" | "mcp" | "lsp">(() => {
    const saved = localStorage.getItem("werk.tools.pane");
    return saved === "live" || saved === "server" || saved === "mcp" || saved === "lsp"
      ? saved
      : "agent";
  });
  const switchPane = (p: "live" | "agent" | "server" | "mcp" | "lsp") => {
    setPane(p);
    localStorage.setItem("werk.tools.pane", p);
  };

  // Agent-side MCP status. `probe = false` reads cached state only;
  // `probe = true` starts the servers once to refresh their tool lists.
  const [agentMcp, setAgentMcp] = useState<McpServerStatus[] | null>(null);
  const [agentMcpLoading, setAgentMcpLoading] = useState(false);

  // Live probe of what the running server advertises at /tools.
  const [liveTools, setLiveTools] = useState<ServerToolInfo[] | null>(null);
  const [liveToolsError, setLiveToolsError] = useState<string | null>(null);
  const [liveToolsLoading, setLiveToolsLoading] = useState(false);

  const fetchLiveTools = useCallback(async (status: ServerStatus) => {
    if (status.type !== "running") {
      setLiveTools(null);
      setLiveToolsError(null);
      return;
    }
    setLiveToolsLoading(true);
    setLiveToolsError(null);
    try {
      setLiveTools(await call(commands.getServerTools()));
    } catch (e) {
      setLiveTools(null);
      setLiveToolsError(String(e));
    } finally {
      setLiveToolsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (serverStatus.type === "running" && active) {
      fetchLiveTools(serverStatus);
      const id = setInterval(() => fetchLiveTools(serverStatus), 5000);
      return () => clearInterval(id);
    }
    if (serverStatus.type !== "running") {
      setLiveTools(null);
      setLiveToolsError(null);
    }
  }, [serverStatus, active, fetchLiveTools]);

  const loadAgentMcp = useCallback(async (probe: boolean) => {
    setAgentMcpLoading(true);
    try {
      setAgentMcp(await call(commands.getMcpAgentTools(probe)));
    } catch (e) {
      setError(String(e));
    } finally {
      setAgentMcpLoading(false);
    }
  }, []);

  // Opening the Live tab only reads cached state — nothing is started.
  useEffect(() => {
    if (pane === "live") loadAgentMcp(false);
  }, [pane, loadAgentMcp]);

  const load = async () => {
    try {
      const [mc, srv] = await Promise.all([
        call(commands.listMcpServers()),
        call(commands.getServerStatus()).catch(() => ({ type: "stopped" }) as ServerStatus),
      ]);
      setMcpPath(mc.path);
      setServers(mc.servers);
      setServerStatus(srv);
    } catch (e) {
      setError(String(e));
    }
  };

  // Server-native tool selection follows the config snapshot.
  useEffect(() => {
    if (!appConfig) return;
    setSelected(new Set(appConfig.server_tools ?? []));
    selectedRef.current = new Set(appConfig.server_tools ?? []);
  }, [appConfig?.server_tools]);

  useEffect(() => {
    load();
    return () => {
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
    };
  }, []);

  // ── Built-in tools ────────────────────────────────────────────────────

  const persistTools = (sel: Set<string>) => {
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(async () => {
      try {
        await call(commands.setTools([...sel].sort()));
      } catch (e) {
        setError(String(e));
      }
    }, 400);
  };

  const toggleTool = (name: string, on: boolean) => {
    if (on && name === "exec_shell_command") {
      if (!window.confirm(
        "Shell Command lets the model execute arbitrary commands on your " +
        "computer with your user's permissions. Continue?")) return;
    }
    const next = new Set(selectedRef.current);
    if (on) next.add(name); else next.delete(name);
    selectedRef.current = next;
    setSelected(next);
    persistTools(next);
  };

  const passing = toolsArgValue([...selected].sort().join(","));

  // ── MCP servers ───────────────────────────────────────────────────────

  const openAdd = () => {
    setEditId(null);
    setDraft(emptyEntry());
    setArgsText("");
    setEnvText("");
    setShowForm(true);
  };

  const openEdit = (e: McpServerEntry) => {
    setEditId(e.name);
    setDraft({ ...e, args: [...(e.args ?? [])], env: { ...(e.env ?? {}) } });
    setArgsText(argsToText(e.args));
    setEnvText(envToText(e.env));
    setShowForm(true);
  };

  const closeForm = () => {
    setShowForm(false);
    setEditId(null);
  };

  const saveServers = async (next: McpServerEntry[]) => {
    try {
      await call(commands.saveMcpServers(next));
      setServers(next);
      setShowForm(false);
      setEditId(null);
      // No live refresh here: saving must not spawn MCP servers.
    } catch (e) {
      setError(String(e));
    }
  };

  const saveEntry = async () => {
    const name = draft.name.trim();
    const command = draft.command.trim();
    if (!name || !command) {
      setError("Name and command are required for an MCP server.");
      return;
    }
    setError(null);
    const entry: McpServerEntry = {
      ...draft,
      name,
      command,
      args: parseArgs(argsText),
      env: parseEnv(envText),
      cwd: draft.cwd?.trim() || null,
      timeout_ms: draft.timeout_ms ?? null,
      enabled: draft.enabled ?? true,
    };
    const idx = servers.findIndex((s) => s.name === name);
    const next = idx >= 0
      ? servers.map((s) => (s.name === name ? entry : s))
      : [...servers, entry];
    await saveServers(next);
  };

  const removeServer = async (name: string) => {
    if (!window.confirm(`Remove MCP server "${name}"?`)) return;
    setError(null);
    await saveServers(servers.filter((s) => s.name !== name));
  };

  const toggleEnabled = async (name: string, on: boolean) => {
    await saveServers(servers.map((s) => (s.name === name ? { ...s, enabled: on } : s)));
  };

  const showPath = async (path: string, dir: boolean) => {
    try {
      if (dir) await revealItemInDir(path);
      else await openPath(path);
    } catch (e) {
      setError(String(e));
    }
  };

  const running = serverStatus.type === "running" || serverStatus.type === "starting";
  const mode = appConfig?.server_mode ?? "single";

  const formFields = (
    <>
      <div className="flex items-center justify-between">
        <p className="text-sm font-medium text-ink">
          {editId ? `Edit "${editId}"` : "Add MCP Server"}
        </p>
        <button className="btn-ghost text-xs px-1.5 py-0.5" onClick={closeForm}>
          <X size={12} />
        </button>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <div>
          <p className="text-[0.625rem] uppercase tracking-wider text-dim mb-1">Name</p>
          <input type="text" className="input w-full" value={draft.name} placeholder="e.g. fetch"
            onChange={(e) => setDraft((d) => ({ ...d, name: e.target.value }))} />
        </div>
        <div>
          <p className="text-[0.625rem] uppercase tracking-wider text-dim mb-1">Command</p>
          <input type="text" className="input w-full font-mono text-xs" value={draft.command}
            placeholder="e.g. npx, uvx, ./server(.exe)" onChange={(e) => setDraft((d) => ({ ...d, command: e.target.value }))} />
        </div>
      </div>
      <div>
        <p className="text-[0.625rem] uppercase tracking-wider text-dim">Args</p>
        <p className="text-xs text-faint mb-1">One argument per line</p>
        <textarea className="input w-full font-mono text-xs min-h-[96px]" value={argsText}
          placeholder={"-y\n@modelcontextprotocol/server-fetch"}
          onChange={(e) => setArgsText(e.target.value)} />
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
        <div>
          <p className="text-[0.625rem] uppercase tracking-wider text-dim">Environment</p>
          <p className="text-xs text-faint mb-1">KEY=VALUE, one per line</p>
          <textarea className="input w-full font-mono text-xs min-h-[96px]" value={envText}
            placeholder={"API_KEY=xyz"}
            onChange={(e) => setEnvText(e.target.value)} />
        </div>
        <div className="space-y-4">
          <div>
            <p className="text-[0.625rem] uppercase tracking-wider text-dim mb-1">Working Directory (optional)</p>
            <input type="text" className="input w-full font-mono text-xs" value={draft.cwd ?? ""}
              placeholder="e.g. D:\work" onChange={(e) => setDraft((d) => ({ ...d, cwd: e.target.value }))} />
          </div>
          <div>
            <p className="text-[0.625rem] uppercase tracking-wider text-dim mb-1">Timeout (ms, optional)</p>
            <input type="number" min={0} className="input w-full font-mono text-xs"
              value={draft.timeout_ms ?? ""} placeholder="default: 30000"
              onChange={(e) => {
                const n = e.target.value === "" ? null : Number(e.target.value);
                setDraft((d) => ({ ...d, timeout_ms: n && !isNaN(n) ? n : null }));
              }} />
          </div>
        </div>
      </div>
      <div className="flex gap-2">
        <button className="btn-primary text-xs" onClick={saveEntry}>
          {!editId && <Plus size={12} />}
          Save Server
        </button>
        <button className="btn-ghost text-xs" onClick={closeForm}>Cancel</button>
      </div>
    </>
  );

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-5xl mx-auto">
        <div>
          <h1 className="section-title">Tools</h1>
          <p className="section-desc">
            {pane === "agent"
              ? "Tools for werk's agent harness. Available in every mode."
              : pane === "live"
                ? "What the running server and the agent's MCP servers currently offer."
                : pane === "mcp"
                  ? "MCP servers are shared — llama-server exposes them to its Web UI/API clients, and werk starts them for the chat agent."
                  : "Tools that llama-server executes for its own API and Web UI clients."}
          </p>
        </div>

        {error && (
          <div className="card border-accent-red/30 bg-accent-red/5 text-sm text-accent-red">
            {error}
          </div>
        )}

        <div className="flex items-center gap-0.5">
          {(
            [
              { id: "live" as const, label: "Live Tools", icon: Activity },
              { id: "agent" as const, label: "Agent", icon: Bot },
              { id: "server" as const, label: "Server", icon: Server },
              { id: "mcp" as const, label: "MCP", icon: Plug },
              { id: "lsp" as const, label: "LSP", icon: FileCode },
            ]
          ).map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => switchPane(id)}
              className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium transition-colors ${
                pane === id ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              <Icon size={13} />
              {label}
            </button>
          ))}
        </div>

        {pane === "agent" && (
          <>
            <div className="card border-accent/30 bg-accent/5">
              <p className="text-xs text-dim">
                More agent options — system prompt, memory, utility model, and agent file
                visibility — live in{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() => onOpenSettings?.("agent")}
                >
                  Settings → Agent
                </button>
                .
              </p>
            </div>
            <AgentToolsCard builtin={toolsList.builtin} reload={toolsList.reload} />
            <PluginToolsCard
              plugins={toolsList.plugins}
              errors={toolsList.errors}
              pluginsDir={toolsList.pluginsDir}
              setPlugins={toolsList.setPlugins}
              reload={toolsList.reload}
            />
          </>
        )}

        {pane === "lsp" && (
          <LspCard
            appConfig={appConfig}
            setAppConfig={setAppConfig}
            refresh={refreshConfig}
            reloadTools={toolsList.reload}
          />
        )}

        {pane === "server" && mode === "external" && (
          <div className="card">
            <h2 className="section-title mb-1">Not used in External API mode</h2>
            <p className="section-desc">
              There is no local llama-server, so its built-in tools don't apply. The agent tools
              and the MCP servers in the MCP tab work in every mode.
            </p>
          </div>
        )}

        {pane === "live" && (
          <>
            <div className="card">
              <div className="flex items-center justify-between mb-1">
                <h2 className="section-title mb-0">Server (llama-server)</h2>
                <button
                  className="btn-ghost text-xs py-1 px-2"
                  onClick={() => fetchLiveTools(serverStatus)}
                  disabled={liveToolsLoading || serverStatus.type !== "running"}
                  title="Refresh from /tools"
                >
                  <RefreshCw size={12} className={liveToolsLoading ? "animate-spin" : ""} /> Refresh
                </button>
              </div>
              <p className="section-desc">
                What the running llama-server advertises at <span className="font-mono">/tools</span> —
                its built-in file tools plus MCP tools for the Web UI.
              </p>
              {serverStatus.type !== "running" ? (
                <p className="text-xs text-dim mt-3">Server not running.</p>
              ) : liveToolsLoading && !liveTools ? (
                <p className="text-xs text-dim mt-3 flex items-center gap-2">
                  <RefreshCw size={12} className="animate-spin" /> Checking…
                </p>
              ) : liveToolsError ? (
                <p className="text-xs text-accent-yellow mt-3">Could not fetch: <span className="font-mono">{liveToolsError}</span></p>
              ) : liveTools ? (
                (() => {
                  const serverTools = liveTools.filter((t) => t.type === "server");
                  const mcpTools = liveTools.filter((t) => t.type === "mcp");
                  const hasMcp = mcpTools.length > 0;
                  return (
                    <div className="flex flex-wrap items-center gap-2 text-xs mt-3">
                      <span className="flex items-center gap-1.5">
                        <Wrench size={12} className="text-dim" />
                        <span className="text-dim">Built-in:</span>
                        <span className="text-ink">{serverTools.length}</span>
                      </span>
                      <span className="text-faint">•</span>
                      <span className="flex items-center gap-1.5">
                        <Globe size={12} className={hasMcp ? "text-accent-green" : "text-faint"} />
                        <span className={hasMcp ? "text-accent-green" : "text-faint"}>MCP:</span>
                        <span className={hasMcp ? "text-accent-green font-medium" : "text-faint"}>{mcpTools.length}</span>
                        {hasMcp ? <CheckCircle2 size={12} className="text-accent-green" /> : <AlertTriangle size={12} className="text-accent-yellow" />}
                      </span>
                      {hasMcp && <span className="text-dim">({mcpTools.map((t) => t.name).join(", ")})</span>}
                    </div>
                  );
                })()
              ) : null}
            </div>

            <div className="card">
              <div className="flex items-center justify-between mb-1">
                <h2 className="section-title mb-0">Agent (werk)</h2>
                <button
                  className="btn-ghost text-xs py-1 px-2"
                  onClick={() => loadAgentMcp(true)}
                  disabled={agentMcpLoading}
                  title="Start the servers once to refresh what they expose"
                >
                  <RefreshCw size={12} className={agentMcpLoading ? "animate-spin" : ""} /> Check now
                </button>
              </div>
              <p className="section-desc">
                MCP servers the chat agent offers. They stay idle until one of their tools is
                called in a run; "Check now" starts them once to refresh the list.
              </p>
              {agentMcpLoading && !agentMcp ? (
                <p className="text-[0.6875rem] text-dim mt-2 flex items-center gap-2">
                  <RefreshCw size={11} className="animate-spin" /> Checking…
                </p>
              ) : agentMcp ? (
                <>
                  {agentMcp.map((s) => (
                    <div key={s.name} className="border border-border px-2.5 py-1.5 mt-1.5">
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-xs text-ink">{s.name}</span>
                        <span
                          className={`${s.running ? "badge-green" : "badge-gray"} text-[0.5625rem]`}
                        >
                          {s.running ? "running" : "idle"}
                        </span>
                        <span className="text-[0.625rem] text-faint">
                          {s.tools.length > 0
                            ? `${s.tools.length} tool${s.tools.length === 1 ? "" : "s"}`
                            : "not discovered yet"}
                        </span>
                      </div>
                      {s.tools.length > 0 && (
                        <p className="text-[0.625rem] text-faint font-mono truncate mt-0.5">
                          {s.tools.join(", ")}
                        </p>
                      )}
                      {s.error && (
                        <p className="text-[0.625rem] text-accent-yellow mt-0.5">{s.error}</p>
                      )}
                    </div>
                  ))}
                  {agentMcp.length === 0 && (
                    <p className="text-[0.6875rem] text-faint mt-2">No MCP servers configured.</p>
                  )}
                </>
              ) : null}
            </div>
          </>
        )}

        {pane === "server" && mode !== "external" && (
          <>
            <div className="card">
              <h2 className="section-title mb-1">Built-in Tools</h2>
              <p className="section-desc">
                The tools here are only for the <span className="font-bold">Web UI</span> and exposed at{" "}
                <span className="font-mono">/tools</span>.
                {running && (
                  <span className="text-accent-yellow"> Restart the running server to pick up changes.</span>
                )}
              </p>
              <div className="grid grid-cols-2 gap-x-4 gap-y-2.5 mt-3">
                {KNOWN_TOOLS.filter((t) => !t.dangerous).map((t) => (
                  <Toggle key={t.name} label={t.label} hint={t.hint}
                    checked={selected.has(t.name)}
                    onChange={(on) => toggleTool(t.name, on)} />
                ))}
              </div>
              <div className="space-y-2.5 mt-3">
                {KNOWN_TOOLS.filter((t) => t.dangerous).map((t) => (
                  <Toggle key={t.name} label={t.label} hint={t.hint}
                    checked={selected.has(t.name)}
                    onChange={(on) => toggleTool(t.name, on)} />
                ))}
              </div>
              <div className="mt-3 text-xs text-dim">
                {passing ? (
                  <>
                    Passing: <code className="font-mono text-ink">--tools {passing}</code>
                  </>
                ) : (
                  "No tools enabled — the server keeps file access off."
                )}
              </div>
            </div>
          </>
        )}

        {pane === "mcp" && (
          <>
            <div className="card">
              <div className="flex items-center justify-between mb-1">
                <h2 className="section-title mb-0">MCP Servers</h2>
                {!showForm && (
                  <button className="btn-secondary text-xs py-1 px-2" onClick={openAdd}>
                    <Plus size={12} /> Add MCP server
                  </button>
                )}
              </div>
              <p className="section-desc">
                Cursor-compatible servers from <span className="font-mono">mcp.json</span>.
              </p>

              {servers.length > 0 ? (
                <div className="space-y-2 mt-3">
                  {servers.map((s) => {
                    const editing = showForm && editId === s.name;
                    return (
                      <div key={s.name}>
                        <div
                          className={`flex items-center gap-3 border border-border bg-surface-1 px-3 py-2 ${
                            editing ? "rounded-t border-b-0" : "rounded"
                          } ${s.enabled === false ? "opacity-50" : ""}`}
                        >
                          <div className="flex-1 min-w-0">
                            <p className="text-sm font-medium text-ink font-mono">{s.name}</p>
                            <p className="text-xs text-dim truncate font-mono">{s.command}</p>
                            <p className="text-xs text-faint mt-0.5">
                              {(s.args ?? []).length > 0 && `${s.args!.length} arg(s)`}
                              {(s.args ?? []).length > 0 && s.env && Object.keys(s.env).length > 0 && " · "}
                              {s.env && Object.keys(s.env).length > 0 && `${Object.keys(s.env).length} env var(s)`}
                              {s.timeout_ms != null && ` · timeout ${s.timeout_ms}ms`}
                            </p>
                          </div>
                          <span title={s.enabled !== false ? "Enabled — click to disable" : "Disabled — click to enable"}>
                            <Toggle checked={s.enabled !== false} onChange={(on) => toggleEnabled(s.name, on)} />
                          </span>
                          <button className="btn-ghost text-xs py-1 px-2" onClick={() => openEdit(s)}>
                            <Pencil size={12} /> Edit
                          </button>
                          <button className="btn-ghost text-xs py-1 px-2 text-accent-red" onClick={() => removeServer(s.name)}>
                            <Trash2 size={12} /> Remove
                          </button>
                        </div>
                        {editing && (
                          <div className="rounded-b border border-t-0 border-border bg-surface-1 p-4 space-y-4">
                            {formFields}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              ) : (
                <p className="text-sm text-dim mt-3">
                  No MCP servers configured.
                </p>
              )}

              {showForm && editId === null && (
                <div className="rounded border border-border bg-surface-1 p-4 mt-3 space-y-4">
                  {formFields}
                </div>
              )}

              {mcpPath && (
                <div className="flex flex-wrap items-center gap-2 mt-3 text-xs text-faint">
                  <span>
                    Saved to <code className="font-mono text-dim">{mcpPath}</code>
                  </span>
                  <button
                    className="btn-ghost text-xs py-1 px-2"
                    onClick={() => showPath(mcpPath, false)}
                  >
                    <ExternalLink size={12} />
                    Open file
                  </button>
                  <button
                    className="btn-ghost text-xs py-1 px-2"
                    onClick={() => showPath(mcpPath, true)}
                  >
                    <FolderOpen size={12} />
                    Show in folder
                  </button>
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

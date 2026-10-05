import { useEffect, useState } from "react";
import {
  Boxes,
  Cloud,
  Drama,
  ExternalLink,
  MessageCircleHeart,
  MessageSquare,
  Network,
  Pencil,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { commands } from "../bindings";
import type { AppConfig, CharacterCard, Provider, ServerStatus } from "../bindings";
import { call } from "../utils/ipc";
import { notifyConfigChanged, setProfileMirror } from "../utils/appSettings";
import { subscribeServerStatus } from "../utils/serverStatus";
import ModelPicker, { type PickerItem } from "../components/ModelPicker";

const splitList = (raw: string) =>
  raw.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);

export default function Mode({
  go,
}: {
  go: (t: "run" | "chat" | "talk" | "roleplay" | "webui") => void;
}) {
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [status, setStatus] = useState<ServerStatus>({ type: "stopped" });
  const [error, setError] = useState<string | null>(null);
  /// Provider id (or "editor") with a models fetch in flight.
  const [busy, setBusy] = useState<string | null>(null);
  // Inline provider editor.
  const [editing, setEditing] = useState<number | "new" | null>(null);
  const [roleplayCard, setRoleplayCard] = useState<CharacterCard | null>(null);
  const [pId, setPId] = useState("");
  const [pUrl, setPUrl] = useState("");
  const [pKey, setPKey] = useState("");
  const [pModels, setPModels] = useState("");
  const [pCtx, setPCtx] = useState("");
  const [pIn, setPIn] = useState("");
  const [pOut, setPOut] = useState("");

  const refresh = () =>
    call(commands.getConfig())
      .then(setAppConfig)
      .catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
    return subscribeServerStatus(setStatus);
  }, []);

  const mode = appConfig?.server_mode ?? "single";
  const profile = appConfig?.chat_profile ?? "agent";
  const running = status.type === "running" || status.type === "starting";
  const port = status.type === "running" ? status.port : null;
  const providers = appConfig?.providers ?? [];
  const target = appConfig?.external_target ?? "";
  const favorites = appConfig?.provider_favorites ?? [];
  const cut = target.indexOf(":");
  const providerId = cut > 0 ? target.slice(0, cut) : "";
  const modelId = cut > 0 ? target.slice(cut + 1) : "";

  const modelItems: PickerItem[] = providers.flatMap((p) =>
    (p.models ?? []).map((m) => ({ target: `${p.id}:${m}`, provider: p.id, model: m })),
  );
  const pickerItems =
    target && !modelItems.some((i) => i.target === target)
      ? [{ target, provider: providerId || "external", model: modelId || target, stale: true }, ...modelItems]
      : modelItems;

  const choose = async (next: "single" | "router" | "external") => {
    if (next === mode) return;
    setError(null);
    setAppConfig((c) => (c ? { ...c, server_mode: next } : c));
    try {
      await call(commands.setServerMode(next));
      // Router needs an orchestrator; seed it from the selected model.
      if (next === "router") {
        const c = await call(commands.getConfig());
        if (!c.harness_roles?.orchestrator && c.selected_model) {
          await call(commands.setHarnessRoles(c.selected_model, c.harness_roles?.worker ?? null));
        }
      }
      await refresh();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
      await refresh();
    }
  };

  useEffect(() => {
    const id = appConfig?.roleplay?.card_id;
    if (!id) {
      setRoleplayCard(null);
      return;
    }
    let alive = true;
    call(commands.roleplayGetCard(id))
      .then((c) => alive && setRoleplayCard(c))
      .catch(() => alive && setRoleplayCard(null));
    return () => {
      alive = false;
    };
  }, [appConfig?.roleplay?.card_id]);

  const chooseProfile = async (next: "agent" | "webui" | "roleplay" | "assistant") => {
    if (next === profile) return;
    setError(null);
    setProfileMirror(next);
    setAppConfig((c) => (c ? { ...c, chat_profile: next } : c));
    try {
      await call(commands.setChatProfile(next));
      await refresh();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
      await refresh();
      notifyConfigChanged();
    }
  };

  // External API mode has no local server, so the WebUI profile is moot.
  useEffect(() => {
    if (mode === "external" && profile === "webui") chooseProfile("agent");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, profile]);

  const selectTarget = async (id: string) => {
    setError(null);
    try {
      await call(commands.setExternalTarget(id));
      await refresh();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleFavorite = async (t: string) => {
    const next = favorites.includes(t) ? favorites.filter((f) => f !== t) : [...favorites, t];
    setAppConfig((c) => (c ? { ...c, provider_favorites: next } : c));
    try {
      await call(commands.setProviderFavorites(next));
    } catch (e) {
      setError(String(e));
      await refresh();
    }
  };

  const startEdit = (index: number | "new") => {
    const p = index === "new" ? null : providers[index];
    setPId(p?.id ?? "");
    setPUrl(p?.base_url ?? "");
    setPKey(p?.api_key ?? "");
    setPModels((p?.models ?? []).join(", "));
    setPCtx(p?.context_length != null ? String(p.context_length) : "");
    setPIn(p?.input_price != null ? String(p.input_price) : "");
    setPOut(p?.output_price != null ? String(p.output_price) : "");
    setEditing(index);
  };

  const persist = async (list: Provider[]) => {
    try {
      await call(commands.setProviders(list));
      setError(null);
      const c = await call(commands.getConfig());
      // A deleted or renamed provider orphans the target and its stars.
      const t = c.external_target ?? "";
      const tc = t.indexOf(":");
      if (tc > 0 && !(c.providers ?? []).some((p) => p.id === t.slice(0, tc))) {
        await call(commands.setExternalTarget(null));
      }
      const live = new Set((c.providers ?? []).map((p) => p.id));
      const kept = (c.provider_favorites ?? []).filter((f) => {
        const k = f.indexOf(":");
        return k > 0 && live.has(f.slice(0, k));
      });
      if (kept.length !== (c.provider_favorites ?? []).length) {
        await call(commands.setProviderFavorites(kept));
      }
      setEditing(null);
      await refresh();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const saveProvider = async () => {
    const ctx = pCtx.trim() === "" ? null : Number(pCtx);
    const inPrice = pIn.trim() === "" ? null : Number(pIn);
    const outPrice = pOut.trim() === "" ? null : Number(pOut);
    const entry: Provider = {
      id: pId.trim(),
      base_url: pUrl.trim().replace(/\/+$/, ""),
      api_key: pKey.trim() || null,
      models: splitList(pModels),
      context_length: ctx != null && !Number.isNaN(ctx) && ctx > 0 ? ctx : null,
      input_price: inPrice != null && Number.isFinite(inPrice) && inPrice >= 0 ? inPrice : null,
      output_price: outPrice != null && Number.isFinite(outPrice) && outPrice >= 0 ? outPrice : null,
    };
    const list =
      editing === "new"
        ? [...providers, entry]
        : providers.map((p, i) => (i === editing ? entry : p));
    await persist(list);
  };

  const removeProvider = async (index: number) => {
    if (!window.confirm(`Remove provider "${providers[index].id}"?`)) return;
    await persist(providers.filter((_, i) => i !== index));
  };

  /// Fetch a provider's models. `id` = provider id, or "editor" for the form.
  const fetchModels = async (id: string) => {
    const p =
      id === "editor"
        ? { base_url: pUrl.trim(), api_key: pKey.trim() || null }
        : providers.find((x) => x.id === id);
    if (!p) return;
    setBusy(id);
    setError(null);
    try {
      const ids = await call(commands.testProvider(p.base_url, p.api_key ?? null));
      if (id === "editor") {
        setPModels(ids.join(", "));
      } else {
        await persist(providers.map((x) => (x.id === id ? { ...x, models: ids } : x)));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const stopServer = async () => {
    setError(null);
    try {
      await call(commands.stopServer());
      setStatus({ type: "stopped" });
    } catch (e) {
      setError(String(e));
    }
  };

  const openWebUi = async () => {
    if (port == null) return;
    try {
      await openUrl(`http://127.0.0.1:${port}/`);
    } catch (e) {
      setError(String(e));
    }
  };

  const options = [
    {
      id: "single" as const,
      icon: Boxes,
      title: "Single model",
      desc: "One model is loaded on server start; the agent chats with it.",
      points: [
        "Pick a model and tune the llama.cpp launch options in Run",
        "All llama.cpp server-side tools (--tools), MCP, LSP, are available to configure",
        "The most basic way to run and test a model",
      ],
    },
    {
      id: "router" as const,
      icon: Network,
      title: "Router (orchestrator + worker)",
      desc: "Models load on demand. The orchestrator plans and builds, the worker executes delegated tasks.",
      points: [
        "Orchestrator loads at start, worker on first delegation",
        "Assign per-role Ctx / GPU layers / turn budget",
        "Mixed mode: pick an external API model for a role",
      ],
    },
    {
      id: "external" as const,
      icon: Cloud,
      title: "External API",
      desc: "No local server. Use an OpenAI-compatible endpoint with werk's system prompt, tools, sessions, and permissions.",
      points: [
        "Add endpoints and keys in the panel below",
        "Pick your favorite models",
        "The whole agent stack works: file tools, subagents, LSP, etc.",
      ],
    },
  ];

  const profiles = [
    {
      id: "agent" as const,
      icon: MessageSquare,
      title: "General / code",
      desc: "The full coding agent.",
      points: ["Chat with tools, projects, git, sessions", "Agent, Tools, and Bench tabs"],
      soon: false,
      beta: false,
    },
    {
      id: "webui" as const,
      icon: ExternalLink,
      title: "WebUI",
      desc: "Use llama-server's own chat UI.",
      points: ["Embeds the server's own chat UI", "Can use built-in tools and MCPs"],
      soon: false,
      beta: false,
    },
    {
      id: "roleplay" as const,
      icon: Drama,
      title: "Roleplay",
      desc: "Character cards with the Talk tab.",
      points: ["Per-character memory and greetings", "Compaction and distill for long stories"],
      soon: false,
      beta: true,
    },
    {
      id: "assistant" as const,
      icon: Sparkles,
      title: "Assistant",
      desc: "A personal assistant with personality.",
      points: ["Remembers you across conversations", "A multi-modal vision model is recommended"],
      soon: false,
      beta: true,
    },
  ];

  // Center of the active server card, for the flow line's drop.
  const serverIndex = Math.max(0, options.findIndex((o) => o.id === mode));

  // Disabled profiles disappear from the picker entirely.
  const webuiEnabled = appConfig?.webui_enabled ?? false;
  const visibleProfiles = profiles.filter((p) => p.id !== "webui" || webuiEnabled);
  const profileColW = 100 / visibleProfiles.length;

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-6xl mx-auto">
        <div>
          <h1 className="section-title">Mode</h1>
          <p className="section-desc">
            Choose how werk serves models.
          </p>
        </div>

        {error && (
          <div className="card border-accent-red/30 bg-accent-red/5">
            <p className="text-sm text-accent-red">{error}</p>
            <button
              className="text-xs text-accent-red/70 hover:text-accent-red mt-1"
              onClick={() => setError(null)}
            >
              Dismiss
            </button>
          </div>
        )}

        {/* Columns carry padding instead of a grid gap so the flow line's
            centers land on exact fractions. */}
        <div className="grid grid-cols-3">
          {options.map(({ id, icon: Icon, title, desc, points }) => {
            const active = mode === id;
            return (
              <div key={id} className="px-2">
                <button
                  onClick={() => choose(id)}
                  className={`card w-full h-full text-left transition-colors ${
                    active ? "card-active border-accent bg-accent/5" : "hover:bg-surface-2"
                  }`}
                >
                  <div className="flex items-center gap-2 mb-1">
                    <Icon size={15} className={active ? "text-accent" : "text-dim"} />
                    <span className="text-sm font-semibold text-ink">{title}</span>
                    {active && (
                      <span className="ml-auto badge-green text-[0.625rem] shrink-0">active</span>
                    )}
                  </div>
                  <p className="text-xs text-dim leading-snug">{desc}</p>
                  <ul className="mt-2 space-y-1">
                    {points.map((p) => (
                      <li key={p} className="text-[0.6875rem] text-faint flex gap-1.5">
                        <span className="text-faint/60">·</span>
                        {p}
                      </li>
                    ))}
                  </ul>
                </button>
              </div>
            );
          })}
        </div>

        {/* Flow: the active server mode feeds the chat profile row below. */}
        <div className="relative h-5">
          <div
            className="absolute top-0 h-[21px] w-px bg-border -translate-x-1/2 transition-all duration-300"
            style={{ left: `${((serverIndex + 0.5) / 3) * 100}%` }}
          />
        </div>
        <div className="relative">
          <div
            className="absolute top-0 h-px bg-border"
            style={{
              left: `calc(${profileColW / 2}% - 1px)`,
              right: `calc(${profileColW / 2}% - 1px)`,
            }}
          />
          <div
            className="grid pt-5"
            style={{ gridTemplateColumns: `repeat(${visibleProfiles.length}, minmax(0, 1fr))` }}
          >
            {visibleProfiles.map(({ id, icon: Icon, title, desc, points, soon, beta }) => {
              const disabled = soon || (id === "webui" && mode === "external");
              const active = !disabled && profile === id;
              return (
                <div key={id} className="relative px-2">
                  <div className="absolute -top-5 left-1/2 -translate-x-1/2 h-[21px] w-px bg-border" />
                  <button
                    onClick={() => !disabled && chooseProfile(id as "agent" | "webui" | "roleplay")}
                    disabled={disabled}
                    className={`card w-full h-full text-left transition-colors ${
                      active
                        ? "card-active border-accent bg-accent/5"
                        : disabled
                          ? "opacity-50 cursor-default"
                          : "hover:bg-surface-2"
                    }`}
                  >
                    <div className="flex items-center gap-2 mb-1">
                      <Icon size={15} className={active ? "text-accent" : "text-dim"} />
                      <span className="text-sm font-semibold text-ink">{title}</span>
                      {beta && (
                        <span className="badge-blue text-[0.5625rem] shrink-0">BETA</span>
                      )}
                      {active && (
                        <span className="ml-auto badge-green text-[0.625rem] shrink-0">active</span>
                      )}
                      {soon && (
                        <span className="ml-auto badge-gray text-[0.625rem] shrink-0">soon</span>
                      )}
                      {disabled && !soon && (
                        <span className="ml-auto badge-gray text-[0.625rem] shrink-0">
                          external
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-dim leading-snug">{desc}</p>
                    <ul className="mt-2 space-y-1">
                      {points.map((p) => (
                        <li key={p} className="text-[0.6875rem] text-faint flex gap-1.5">
                          <span className="text-faint/60">·</span>
                          {p}
                        </li>
                      ))}
                    </ul>
                  </button>
                </div>
              );
            })}
          </div>
        </div>

        {mode === "external" && (
          <div className="card">
            <div className="flex items-center justify-between mb-1">
              <h2 className="section-title mb-0">Providers</h2>
              {editing === null && (
                <button className="btn-secondary text-xs py-1 px-2" onClick={() => startEdit("new")}>
                  <Plus size={12} /> Add provider
                </button>
              )}
            </div>
            <p className="section-desc">
              OpenAI-compatible endpoints.
            </p>

            {providers.length > 0 && (
              <div className="mt-2">
                <label className="label">Model</label>
                <div className="mt-1">
                  <ModelPicker
                    items={pickerItems}
                    value={target}
                    favorites={favorites}
                    onSelect={selectTarget}
                    onToggleFavorite={toggleFavorite}
                  />
                </div>
              </div>
            )}

            <div className="space-y-1 mt-3">
              {providers.map((p, i) =>
                editing === i ? null : (
                  <div key={p.id} className="flex items-center gap-2 border border-border px-2.5 py-1.5">
                    <span className="font-mono text-xs text-ink shrink-0">{p.id}</span>
                    {p.api_key ? (
                      <span className="badge-green text-[0.5625rem] shrink-0">key</span>
                    ) : (
                      <span className="badge-gray text-[0.5625rem] shrink-0">open</span>
                    )}
                    <span className="font-mono text-[0.625rem] text-faint truncate flex-1" title={p.base_url}>
                      {p.base_url}
                    </span>
                    <span className="text-[0.625rem] text-dim shrink-0">
                      {p.models?.length ?? 0} model{(p.models?.length ?? 0) === 1 ? "" : "s"}
                    </span>
                    <button
                      className="text-faint hover:text-ink shrink-0"
                      onClick={() => fetchModels(p.id)}
                      disabled={busy !== null}
                      title="Fetch this provider's models"
                    >
                      <RefreshCw size={11} className={busy === p.id ? "animate-spin" : ""} />
                    </button>
                    <button className="text-faint hover:text-ink shrink-0" onClick={() => startEdit(i)} title="Edit">
                      <Pencil size={11} />
                    </button>
                    <button
                      className="text-faint hover:text-accent-red shrink-0"
                      onClick={() => removeProvider(i)}
                      title="Remove"
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                ),
              )}
              {providers.length === 0 && editing === null && (
                <p className="text-[0.6875rem] text-dim">
                  No providers yet. Add one to chat with a cloud model through the harness.
                </p>
              )}
            </div>

            {editing !== null && (
              <div className="border border-border p-2.5 space-y-1.5 mt-3">
                <div className="grid grid-cols-2 gap-2">
                  <input
                    className="input py-1 px-2 text-xs"
                    placeholder="Id (e.g. deepseek)"
                    value={pId}
                    onChange={(e) => setPId(e.target.value)}
                  />
                  <input
                    className="input py-1 px-2 text-xs"
                    placeholder="Context length (optional)"
                    value={pCtx}
                    onChange={(e) => setPCtx(e.target.value)}
                  />
                </div>
                <input
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder="Base URL — e.g. https://api.deepseek.com/v1"
                  value={pUrl}
                  onChange={(e) => setPUrl(e.target.value)}
                />
                <input
                  type="password"
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder="API key (optional for local servers)"
                  value={pKey}
                  onChange={(e) => setPKey(e.target.value)}
                />
                <input
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder="Models: model-a, model-b"
                  value={pModels}
                  onChange={(e) => setPModels(e.target.value)}
                />
                <div className="grid grid-cols-2 gap-2">
                  <input
                    className="input py-1 px-2 text-xs"
                    placeholder="Input $ / 1M tokens (optional)"
                    value={pIn}
                    onChange={(e) => setPIn(e.target.value)}
                  />
                  <input
                    className="input py-1 px-2 text-xs"
                    placeholder="Output $ / 1M tokens (optional)"
                    value={pOut}
                    onChange={(e) => setPOut(e.target.value)}
                  />
                </div>
                <div className="flex items-center justify-end gap-2">
                  <button
                    className="btn-ghost text-xs py-1 px-2 mr-auto"
                    disabled={!pUrl.trim() || busy !== null}
                    onClick={() => fetchModels("editor")}
                    title="GET /models and fill the model list"
                  >
                    <RefreshCw size={11} className={busy === "editor" ? "animate-spin" : ""} /> Fetch
                    models
                  </button>
                  <button className="btn-ghost text-xs py-1 px-2" onClick={() => setEditing(null)}>
                    Cancel
                  </button>
                  <button
                    className="btn-secondary text-xs py-1 px-2"
                    disabled={!pId.trim() || !pUrl.trim()}
                    onClick={saveProvider}
                  >
                    Save
                  </button>
                </div>
              </div>
            )}
          </div>
        )}

        {profile === "webui" && mode !== "external" && (
          <div className="card">
            <div className="flex items-center justify-between mb-1">
              <h2 className="section-title mb-0">Web UI</h2>
              <div className="flex items-center gap-1.5">
                <button
                  className="btn-primary text-xs py-1 px-2"
                  disabled={port == null}
                  onClick={() => go("webui")}
                  title="Open the embedded WebUI tab"
                >
                  <ExternalLink size={12} /> Open WebUI
                </button>
                <button
                  className="btn-ghost text-xs py-1 px-2"
                  disabled={port == null}
                  onClick={openWebUi}
                  title="Open in your browser instead"
                >
                  Browser
                </button>
              </div>
            </div>
            <div className="section-desc space-y-1">
              <p>Use this if you want to use llama-server's own chat UI.</p>
              <p>
                Note: The Web UI talks to the model with the server's own chat template —{" "}
                <b>not</b> werk's agent system prompt — this means werk's tools, subagents, LSP, are
                not available, only MCPs.
              </p>
            </div>
          </div>
        )}

        {profile === "roleplay" && (
          <div className="card">
            <div className="flex items-center justify-between gap-2 mb-1">
              <h2 className="section-title mb-0">Roleplay</h2>
              <div className="flex items-center gap-1.5 shrink-0">
                <button className="btn-secondary text-xs py-1 px-2" onClick={() => go("roleplay")}>
                  <Drama size={12} /> Roleplay page
                </button>
                <button className="btn-primary text-xs py-1 px-2" onClick={() => go("talk")}>
                  <MessageCircleHeart size={12} /> Open Talk
                </button>
              </div>
            </div>
            <div className="section-desc space-y-1">
              {roleplayCard ? (
                <p>
                  Active character: <b>{roleplayCard.name}</b> ({roleplayCard.spec},{" "}
                  {1 + roleplayCard.alternate_greetings.length} greeting
                  {roleplayCard.alternate_greetings.length === 0 ? "" : "s"}).
                </p>
              ) : (
                <p>No character card yet — import one on the Roleplay page.</p>
              )}
              <p>
                Talk keeps one continuous discussion per character; compact it when the window
                fills, or distill it into the character's memory.
              </p>
            </div>
          </div>
        )}

        <div className="card">
          {mode === "external" ? (
            <p className="text-xs text-dim">
              Tip:{" "}
              To mix a provider model with a local GGUF, switch to{" "}
              <b>Router mode</b> and pick an external favorite for a role.{" "}
              {running && (
                <>
                  The local server is still running, wasting VRAM —{" "}
                  <button className="text-accent hover:underline" onClick={stopServer}>
                    stop it
                  </button>
                  .{" "}
                </>
              )}
              <button
                className="text-accent hover:underline"
                onClick={() => go(profile === "roleplay" ? "talk" : "chat")}
              >
                Open {profile === "roleplay" ? "Talk" : "Chat"}
              </button>
              .
            </p>
          ) : (
            <p className="text-xs text-dim">
              {running
                ? "The server is running — changes apply the next time it starts."
                : "Changes apply the next time the server starts."}{" "}
              <button className="text-accent hover:underline" onClick={() => go("run")}>
                Go to Run
              </button>{" "}
              to configure and start it, or{" "}
              <button
                className="text-accent hover:underline"
                onClick={() => go(profile === "roleplay" ? "talk" : "chat")}
              >
                open {profile === "roleplay" ? "Talk" : "Chat"}
              </button>
              .
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

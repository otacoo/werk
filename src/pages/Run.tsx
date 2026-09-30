import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Brain,
  Cloud,
  Eye,
  FileCode,
  FolderOpen,
  MessageSquare,
  Plus,
  Projector,
  Rabbit,
  Search,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  AttachmentInfo,
  LaunchPreview,
  MemoryEstimate,
  ModelDto,
  ServerConfig,
  ServerStatus,
} from "../bindings";
import { call } from "../utils/ipc";
import { formatSize } from "../utils/format";
import { subscribeConfigChanged } from "../utils/appSettings";
import { subscribeServerStatus } from "../utils/serverStatus";
import { CommandCard, EstimateCard, LogCard } from "./run/cards";
import { DEFAULT_PRESET } from "./run/labels";
import { Field, Group, SliderField, statusDot } from "./run/fields";
import { indexSettings, type SettingMatch, type LaunchTab } from "./run/settings-search";
import { DEFAULT_EXTRA_ROWS, mergeExtra, splitExtra, type ExtraRow } from "./run/extra";
import { PresetCard } from "./run/preset-card";
import Toggle from "../components/Toggle";
import type { Tab } from "../App";

export const DEFAULT_CONFIG: ServerConfig = {
  model_path: "",
  mmproj_path: null,
  host: "127.0.0.1",
  port: 8080,
  n_ctx: 0,
  n_gpu_layers: -1,
  n_threads: null,
  flash_attn: "auto",
  cache_type_k: "f16",
  cache_type_v: "f16",
  temperature: 0.8,
  top_k: 40,
  min_p: 0.05,
  top_p: 0.95,
  n_predict: -1,
  disable_sampling: false,
  n_batch: 0,
  n_ubatch: 0,
  cont_batching: true,
  seed: null,
  rope_freq_scale: null,
  rope_freq_base: null,
  parallel: 1,
  working_dir: null,
  extra_params: {},
};


/// Built-in tuning used until the user saves anything: llama.cpp's own
/// defaults plus log verbosity 3; 0 batch sizes mean "let the server decide".

const CACHE_TYPES = ["f16", "bf16", "f32", "q8_0", "q5_1", "q5_0", "q4_1", "q4_0", "iq4_nl"];


const LAUNCH_TABS: { id: LaunchTab; label: string }[] = [
  { id: "context", label: "Context" },
  { id: "hardware", label: "Hardware" },
  { id: "sampling", label: "Sampling" },
  { id: "network", label: "Network" },
  { id: "chat", label: "Chat" },
  { id: "extra", label: "Extra" },
];




export default function Run({ go }: { go: (t: Tab) => void }) {
  const [status, setStatus] = useState<ServerStatus | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [models, setModels] = useState<ModelDto[]>([]);
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  // The Mode tab owns this; Run only renders the active mode's surface.
  const routerMode = appConfig?.server_mode === "router";
  const externalMode = appConfig?.server_mode === "external";
  const [selected, setSelected] = useState<string>("");
  const [config, setConfig] = useState<ServerConfig>(DEFAULT_CONFIG);
  const [fitOn, setFitOn] = useState(true);
  const [extraRows, setExtraRows] = useState<ExtraRow[]>(() => [...DEFAULT_EXTRA_ROWS]);
  const [rawArgs, setRawArgs] = useState("");
  const [estimate, setEstimate] = useState<MemoryEstimate | null>(null);
  const [preview, setPreview] = useState<LaunchPreview | null>(null);
  const [copiedLaunch, setCopiedLaunch] = useState(false);
  const [copiedLogs, setCopiedLogs] = useState(false);
  const [noTemplateAttach, setNoTemplateAttach] = useState(false);
  const [estimating, setEstimating] = useState(false);
  const [suggestionNotes, setSuggestionNotes] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [presets, setPresets] = useState<string[]>([]);
  const [activePreset, setActivePreset] = useState<string | null>(null);
  const [presetName, setPresetName] = useState("");
  const [presetFlash, setPresetFlash] = useState<{ text: string; tone: "ok" | "err" } | null>(null);
  const [query, setQuery] = useState("");
  const [launchTab, setLaunchTab] = useState<LaunchTab>("context");
  const [searchIdx, setSearchIdx] = useState(0);
  const [showResults, setShowResults] = useState(false);
  const [matches, setMatches] = useState<SettingMatch[]>([]);
  const settingsRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const flashTimer = useRef<number | undefined>(undefined);

  const flash = (text: string, tone: "ok" | "err") => {
    setPresetFlash({ text, tone });
    window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setPresetFlash(null), 1500);
  };

  const refreshModels = () =>
    call(commands.listInstalledModels())
      .then(setModels)
      .catch((e) => setError(String(e)));
  const refreshStatus = () =>
    call(commands.getServerStatus())
      .then(setStatus)
      .catch(() => setStatus({ type: "stopped" }));

  useEffect(() => {
    call(commands.getConfig())
      .then((c) => {
        setAppConfig(c);
        setSelected(c.selected_model ?? "");
        setConfig((prev) => ({ ...prev, model_path: c.selected_model ?? "" }));
        bootPresets(c.selected_model ?? "", c.last_preset ?? null);
      })
      .catch(() => {});
    refreshModels();
    call(commands.getServerLogs()).then(setLogs).catch(() => {});
    const unlistenLog = listen<string>("server_log", (e) => {
      setLogs((prev) => [...prev.slice(-399), e.payload]);
    });
    const unsubStatus = subscribeServerStatus(setStatus);
    return () => {
      unlistenLog.then((f) => f());
      unsubStatus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [logs]);

  // Other pages (e.g. Mode) mutate the backend config; keep our snapshot fresh.
  useEffect(
    () =>
      subscribeConfigChanged(() => {
        call(commands.getConfig()).then(setAppConfig).catch(() => {});
      }),
    [],
  );

  // Live estimate, debounced as the launch inputs change.
  useEffect(() => {
    const target = routerMode
      ? appConfig?.harness_roles?.orchestrator ?? ""
      : config.model_path || selected;
    if (!target || externalMode) {
      setEstimate(null);
      return;
    }
    const timer = setTimeout(() => {
      call(
        commands.estimateMemory(
          target,
          config.n_ctx,
          config.cache_type_k || "f16",
          config.cache_type_v || "f16",
          config.n_gpu_layers,
          fitOn,
        ),
      )
        .then(setEstimate)
        .catch(() => setEstimate(null));
    }, 300);
    return () => clearTimeout(timer);
  }, [
    config.model_path,
    config.n_ctx,
    config.n_gpu_layers,
    config.cache_type_k,
    config.cache_type_v,
    selected,
    models,
    routerMode,
    appConfig?.harness_roles?.orchestrator,
    externalMode,
    fitOn,
  ]);

  const pickModel = async (path: string) => {
    const next = selected === path ? "" : path;
    setSelected(next);
    setConfig((c) => ({ ...c, model_path: next }));
    try {
      await call(commands.setSelectedModel(next || null));
      if (next) {
        const name = await call(commands.getModelPreset(next));
        // Models without their own preset fall back to the default one.
        await applyPresetName(name ?? DEFAULT_PRESET, next);
      } else {
        setActivePreset(null);
      }
    } catch (e) {
      setError(String(e));
    }
  };

  // Preset files hold tuning only; the current model and folders carry over.
  // Applying one also remembers it for the model, so a relaunch reloads it.
  const applyPresetName = async (name: string, model?: string) => {
    const loaded = await call(commands.loadPreset(name));
    setActivePreset(name);
    setPresetName("");
    setConfig((prev) => ({
      ...loaded,
      model_path: prev.model_path,
      mmproj_path: prev.mmproj_path,
      working_dir: prev.working_dir,
    }));
    setFitOn((loaded.extra_params?.fit ?? "on") !== "off");
    setNoTemplateAttach(loaded.extra_params?.["no-chat-template"] !== undefined);
    const { rows, raw } = splitExtra(loaded.extra_params);
    setExtraRows(rows);
    setRawArgs(raw);
    const forModel = model ?? (config.model_path || selected);
    if (forModel) {
      await call(commands.setModelPreset(forModel, name)).catch(() => {});
    }
    // Remember globally too: router mode has no single selected model.
    await call(commands.setLastPreset(name)).catch(() => {});
  };

  const bootPresets = async (model: string, last: string | null) => {
    try {
      const names = await call(commands.listPresets());
      setPresets(names);
      const associated = model ? await call(commands.getModelPreset(model)) : null;
      // Single mode remembers per model; router mode keeps a global last used.
      const candidate = model ? associated : last;
      const wanted = candidate && names.includes(candidate)
        ? candidate
        : names.includes(DEFAULT_PRESET)
          ? DEFAULT_PRESET
          : null;
      if (wanted) await applyPresetName(wanted, model || undefined);
    } catch (e) {
      setError(String(e));
    }
  };

  // The full launch config: model + fit/extra knobs merged in. Router mode
  // launches with no model path; the backend builds the role preset.
  const buildConfig = (): ServerConfig => {
    const extra = mergeExtra(extraRows, rawArgs);
    if (!fitOn) extra.fit = "off";
    if (noTemplateAttach) extra["no-chat-template"] = "";
    if (routerMode) {
      return { ...config, model_path: "", mmproj_path: null, extra_params: extra };
    }
    return { ...config, model_path: config.model_path || selected, extra_params: extra };
  };

  // ── Agent config (moved from Settings) ────────────────────────────────

  const setRoles = async (orchestrator: string | null, worker: string | null) => {
    setAppConfig((c) => (c ? { ...c, harness_roles: { orchestrator, worker } } : c));
    try {
      await call(commands.setHarnessRoles(orchestrator, worker));
    } catch (e) {
      setError(String(e));
    }
  };

  const setRoleParams = async (
    role: "orchestrator" | "worker",
    ctxSize: number | null,
    nGpuLayers: number | null,
  ) => {
    const next = { ctx_size: ctxSize, n_gpu_layers: nGpuLayers };
    setAppConfig((c) =>
      c
        ? {
            ...c,
            harness_role_params:
              role === "worker"
                ? { ...c.harness_role_params, worker: next }
                : { ...c.harness_role_params, orchestrator: next },
          }
        : c,
    );
    try {
      await call(commands.setRoleParams(role, ctxSize, nGpuLayers));
    } catch (e) {
      setError(String(e));
    }
  };

  const numCtx = (v: string) => (v === "" ? null : Math.max(0, parseInt(v, 10) || 0));
  const numNgl = (v: string) => {
    if (v === "") return null;
    const n = parseInt(v, 10);
    return Number.isNaN(n) ? null : Math.max(-1, n);
  };

  const setTurns = async (role: "orchestrator" | "worker", value: number) => {
    const orch = role === "orchestrator" ? value : appConfig?.harness_max_turns ?? 40;
    const sub = role === "worker" ? value : appConfig?.harness_subagent_max_turns ?? 25;
    setAppConfig((c) =>
      c ? { ...c, harness_max_turns: orch, harness_subagent_max_turns: sub } : c,
    );
    try {
      await call(commands.setMaxTurns(orch, sub));
    } catch (e) {
      setError(String(e));
    }
  };

  // One attachment symbol: colored when on, grey when off.
  const attachmentIcon = (a: AttachmentInfo) => {
    const Icon = a.kind === "mmproj" ? Projector : a.kind === "chat-template" ? FileCode : Rabbit;
    const color =
      a.kind === "mmproj"
        ? "text-[#3B82F6]"
        : a.kind === "chat-template"
          ? "text-accent-yellow"
          : "text-accent-green";
    const cls = a.enabled ? color : "text-faint";
    const key = `${a.role ?? ""}:${a.kind}`;
    const where = a.role ? ` (${a.role})` : "";
    if (!a.auto) {
      return (
        <span key={key} className="inline-flex items-center" title={`${a.kind}${where}: ${a.path}`}>
          <Icon size={13} className={cls} />
        </span>
      );
    }
    return (
      <button
        key={key}
        className="inline-flex items-center hover:opacity-80"
        title={`${a.kind}${where}: ${a.path} — click to ${a.enabled ? "detach" : "attach"}`}
        onClick={() => toggleAttachment(a)}
      >
        <Icon size={13} className={cls} />
      </button>
    );
  };

  // One role column: model picker, companions, then turns/ctx/GPU overrides.
  const roleRow = (role: "orchestrator" | "worker") => {
    const path =
      role === "orchestrator"
        ? appConfig?.harness_roles?.orchestrator ?? ""
        : appConfig?.harness_roles?.worker ?? "";
    const params =
      role === "worker"
        ? appConfig?.harness_role_params?.worker
        : appConfig?.harness_role_params?.orchestrator;
    const model = models.find((m) => m.path === path);
    const update = (ctx: number | null, ngl: number | null) =>
      setRoleParams(role, ctx, ngl);
    const turns =
      role === "orchestrator"
        ? appConfig?.harness_max_turns ?? 40
        : appConfig?.harness_subagent_max_turns ?? 25;
    const roleAttachments = (preview?.attachments ?? []).filter((a) => a.role === role);
    return (
      <div key={role} className="space-y-1.5">
        <div className="flex items-center gap-2">
          <select
            className="input flex-1 min-w-0 py-1 px-2 text-xs"
            value={path}
            onChange={(e) =>
              setRoles(
                role === "orchestrator" ? e.target.value || null : appConfig?.harness_roles?.orchestrator ?? null,
                role === "worker" ? e.target.value || null : appConfig?.harness_roles?.worker ?? null,
              )
            }
          >
            <option value="">
              {role === "worker" ? "Same as orchestrator" : "Server default"}
            </option>
          {models.map((m) => (
            <option key={m.id} value={m.path}>
              {m.name}
            </option>
          ))}
        </select>
          {model && (
            <span className="text-[0.6875rem] font-mono text-dim shrink-0">
              ≈ {formatSize(model.size_bytes ?? 0)}
            </span>
          )}
        </div>
        {path && (
          <div className="flex flex-wrap items-center gap-3">
            {model && (model.is_vision || model.is_reasoning || roleAttachments.length > 0) && (
              <div className="flex items-center gap-2 shrink-0">
                {(model.is_vision || model.is_reasoning) && (
                  <div className="flex items-center gap-1.5">
                    {model.is_vision && (
                      <span title="Vision model">
                        <Eye size={13} className="text-[#3B82F6]" />
                      </span>
                    )}
                    {model.is_reasoning && (
                      <span title="Reasoning model">
                        <Brain size={13} className="text-[#E5484D]" />
                      </span>
                    )}
                  </div>
                )}
                {(model.is_vision || model.is_reasoning) && roleAttachments.length > 0 && (
                  <span className="w-px h-3.5 bg-border shrink-0" />
                )}
                {roleAttachments.length > 0 && (
                  <div className="flex items-center gap-1.5">
                    {roleAttachments.map(attachmentIcon)}
                  </div>
                )}
              </div>
            )}
            <label
              className="flex items-center gap-1.5 text-[0.6875rem] text-faint"
              title="Context size for this role's model in tokens. Empty uses the launch context size, then the model default."
            >
              <span>Ctx</span>
              <input
                type="number"
                min={0}
                step={1024}
                placeholder="auto"
                className="input w-20 py-0.5 px-1.5 text-[0.6875rem]"
                value={params?.ctx_size ?? ""}
                onChange={(e) => update(numCtx(e.target.value), params?.n_gpu_layers ?? null)}
              />
            </label>
            <label
              className="flex items-center gap-1.5 text-[0.6875rem] text-faint"
              title="How many layers of this role's model run on the GPU; the rest stay on the CPU. Empty = auto, -1 = all layers, 0 = CPU only."
            >
              <span>GPU</span>
              <input
                type="number"
                min={-1}
                placeholder="auto"
                title="Empty = auto, -1 = all on GPU, 0 = CPU only"
                className="input w-16 py-0.5 px-1.5 text-[0.6875rem]"
                value={params?.n_gpu_layers ?? ""}
                onChange={(e) => update(params?.ctx_size ?? null, numNgl(e.target.value))}
              />
            </label>
            <label
              className="flex items-center gap-1.5 text-[0.6875rem] text-faint"
              title="Max agent turns for this role: the orchestrator's main loop, or a subagent run."
            >
              <span>Turns</span>
              <input
                type="number"
                min={1}
                max={500}
                className="input w-16 py-0.5 px-1.5 text-[0.6875rem]"
                value={turns}
                onChange={(e) => setTurns(role, Math.max(1, parseInt(e.target.value || "1", 10)))}
              />
            </label>
          </div>
        )}
      </div>
    );
  };

  // Extra-param helpers: dedicated controls edit the matching rows.
  const extraValue = (key: string) => extraRows.find((r) => r.key === key)?.value ?? "";
  const hasExtra = (key: string) => extraRows.some((r) => r.key === key);
  const setExtraFlag = (key: string, on: boolean) => {
    setExtraRows((rows) => {
      const idx = rows.findIndex((r) => r.key === key);
      if (on) return idx >= 0 ? rows : [...rows, { key, value: "" }];
      return idx >= 0 ? rows.filter((_, i) => i !== idx) : rows;
    });
  };
  const templateFile = extraValue("chat-template-file");
  const templateKwargs = extraValue("chat-template-kwargs");
  const setExtraField = (key: string, value: string) => {
    setExtraRows((rows) => {
      const idx = rows.findIndex((r) => r.key === key);
      if (value === "") return idx >= 0 ? rows.filter((_, i) => i !== idx) : rows;
      if (idx >= 0) return rows.map((r, i) => (i === idx ? { ...r, value } : r));
      return [...rows, { key, value }];
    });
  };

  // Live command preview, debounced while the launch options change.
  useEffect(() => {
    const timer = setTimeout(() => {
      call(commands.previewServerArgs(buildConfig()))
        .then(setPreview)
        .catch(() => setPreview(null));
    }, 250);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config, extraRows, rawArgs, fitOn, noTemplateAttach, selected, appConfig?.harness_roles, appConfig?.harness_role_params, appConfig?.server_mode]);

  // Auto-attachments: empty mmproj path and the sentinels suppress them.
  // Router companions are stored per role and feed the preset file.
  const toggleAttachment = (a: AttachmentInfo) => {
    if (!a.auto) return;
    if (a.role === "orchestrator" || a.role === "worker") {
      const key =
        a.kind === "mmproj" ? "no_mmproj" : a.kind === "chat-template" ? "no_chat_template" : "no_draft";
      setAppConfig((c) => {
        if (!c?.harness_role_params) return c;
        const hp = c.harness_role_params;
        const params = { [key]: a.enabled };
        return {
          ...c,
          harness_role_params:
            a.role === "worker"
              ? { ...hp, worker: { ...hp.worker, ...params } }
              : { ...hp, orchestrator: { ...hp.orchestrator, ...params } },
        };
      });
      call(commands.setRoleAttachment(a.role, a.kind, !a.enabled)).catch((e) => setError(String(e)));
      return;
    }
    if (a.kind === "mmproj") set({ mmproj_path: a.enabled ? "" : null });
    else if (a.kind === "chat-template") setNoTemplateAttach(a.enabled);
    else if (a.kind === "draft model") setExtraFlag("spec-draft", a.enabled);
  };

  // ── Launch option search ──────────────────────────────────────────────

  const goToMatch = (m: SettingMatch) => {
    setShowResults(false);
    setQuery("");
    setLaunchTab(m.tab);
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        m.el.scrollIntoView({ behavior: "smooth", block: "center" });
        m.el.style.transition = "background 0.3s";
        m.el.style.background = "rgba(232, 161, 60, 0.3)";
        setTimeout(() => {
          m.el.style.background = "";
          m.el.style.transition = "";
        }, 1500);
      }),
    );
  };

  const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSearchIdx((i) => Math.min(i + 1, matches.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSearchIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const m = matches[searchIdx];
      if (m) goToMatch(m);
    } else if (e.key === "Escape") {
      setShowResults(false);
      setQuery("");
    }
  };

  useEffect(() => {
    const q = query.trim().toLowerCase();
    if (!q) {
      setMatches([]);
      setShowResults(false);
      setSearchIdx(0);
      return;
    }
    setMatches(settingsRef.current ? indexSettings(settingsRef.current, q) : []);
    setSearchIdx(0);
    setShowResults(true);
  }, [query]);

  const savePreset = async () => {
    // Empty name updates the selected preset (or the default one).
    const name = presetName.trim() || activePreset || DEFAULT_PRESET;
    try {
      await call(commands.savePreset(name, buildConfig()));
      setPresets(await call(commands.listPresets()));
      setActivePreset(name);
      setPresetName("");
      const model = config.model_path || selected;
      if (model) await call(commands.setModelPreset(model, name));
      flash("Saved", "ok");
    } catch (e) {
      setError(String(e));
    }
  };

  const deletePreset = async () => {
    if (!activePreset || activePreset === DEFAULT_PRESET) return;
    try {
      await call(commands.deletePreset(activePreset));
      setPresets(await call(commands.listPresets()));
      setActivePreset(null);
      setPresetName("");
      flash("Deleted", "err");
    } catch (e) {
      setError(String(e));
    }
  };

  // Store the current tuning as the fallback preset for models without one.
  const saveDefaults = async () => {
    try {
      await call(commands.savePreset(DEFAULT_PRESET, buildConfig()));
      setPresets(await call(commands.listPresets()));
      setPresetName("");
      setActivePreset(DEFAULT_PRESET);
      const model = config.model_path || selected;
      if (model) await call(commands.setModelPreset(model, DEFAULT_PRESET));
      flash("Saved", "ok");
    } catch (e) {
      setError(String(e));
    }
  };

  // Reset the default preset to factory defaults and apply them.
  const resetDefaults = async () => {
    try {
      await call(commands.resetDefaultPreset());
      await applyPresetName(DEFAULT_PRESET);
      flash("Reset", "ok");
    } catch (e) {
      setError(String(e));
    }
  };

  // Ask the estimator for GPU offload, cache, threads, and batch for this model.
  const autoEstimate = async () => {
    const target = config.model_path || selected;
    const model = models.find((m) => m.path === target);
    if (!model) {
      setError("Select a model first.");
      return;
    }
    setEstimating(true);
    setError(null);
    try {
      const sizeMb = Math.round((model.size_bytes ?? 0) / (1024 * 1024));
      const suggested = await call(commands.suggestServerConfig(target, sizeMb));
      const notes = await call(commands.suggestModelConfig(sizeMb));
      setSuggestionNotes(notes.notes);
      // Hardware-dependent knobs only; sampling and user preferences stay.
      setConfig((prev) => ({
        ...prev,
        n_ctx: suggested.n_ctx,
        n_gpu_layers: suggested.n_gpu_layers,
        n_threads: suggested.n_threads,
        n_batch: suggested.n_batch ?? prev.n_batch,
        n_ubatch: suggested.n_ubatch ?? prev.n_ubatch,
        flash_attn: suggested.flash_attn,
        cache_type_k: suggested.cache_type_k,
        cache_type_v: suggested.cache_type_v,
      }));
    } catch (e) {
      setError(String(e));
    } finally {
      setEstimating(false);
    }
  };

  const start = async () => {
    setError(null);
    setStarting(true);
    try {
      await call(commands.startServer(buildConfig()));
      await refreshStatus();
      const model = config.model_path || selected;
      if (model && activePreset) {
        await call(commands.setModelPreset(model, activePreset));
      }
      const fresh = await call(commands.getServerLogs());
      setLogs(fresh);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const stop = async () => {
    try {
      await call(commands.stopServer());
      await refreshStatus();
    } catch (e) {
      setError(String(e));
    }
  };

  const browseWorkingDir = async () => {
    try {
      const picked = await open({ directory: true, title: "Select server working directory" });
      if (typeof picked === "string" && picked) set({ working_dir: picked });
    } catch (e) {
      setError(String(e));
    }
  };

  const set = (patch: Partial<ServerConfig>) => setConfig((c) => ({ ...c, ...patch }));

  const running = status?.type === "running";
  const selectedModel = models.find((m) => m.path === (config.model_path || selected));
  const maxCtx = Math.max(selectedModel?.context_length ?? 131072, 131072);
  const overrideCtx = config.n_ctx !== 0;
  const dot = statusDot(status);
  const sampling = !config.disable_sampling;
  const launchCommand = preview
    ? `${preview.binary} ${preview.args.map((a) => (a.includes(" ") ? `"${a}"` : a)).join(" ")}`
    : "";
  const copyLaunch = async () => {
    if (!launchCommand) return;
    try {
      await navigator.clipboard.writeText(launchCommand);
      setCopiedLaunch(true);
      setTimeout(() => setCopiedLaunch(false), 1500);
    } catch {}
  };

  const copyLogs = async () => {
    if (logs.length === 0) return;
    try {
      await navigator.clipboard.writeText(logs.join("\n"));
      setCopiedLogs(true);
      setTimeout(() => setCopiedLogs(false), 1500);
    } catch {}
  };

  // External API mode ignores everything on this page — no local server.
  if (externalMode) {
    return (
      <div className="h-full overflow-y-auto">
        <div className="p-6 space-y-4 max-w-4xl">
          <div>
            <h1 className="section-title">Run</h1>
            <p className="section-desc">Local llama.cpp server launch.</p>
          </div>
          <div className="card">
            <div className="flex items-center gap-2 mb-1">
              <Cloud size={15} className="text-accent" />
              <h2 className="section-title mb-0">External API mode is active</h2>
            </div>
            <p className="text-xs text-dim leading-snug">
              The harness chats through the provider selected on the Mode tab, so nothing is
              loaded locally and these launch settings are ignored.
            </p>
            {appConfig?.external_target ? (
              <p className="text-xs text-faint mt-2">
                Current target: <span className="font-mono">{appConfig.external_target}</span>
              </p>
            ) : (
              <p className="text-xs text-accent-soft mt-2">
                No model selected yet — pick a provider and model on the Mode tab.
              </p>
            )}
            <div className="flex items-center gap-2 mt-3">
              <button className="btn-secondary text-xs py-1 px-2" onClick={() => go("mode")}>
                Change mode
              </button>
              <button className="btn-ghost text-xs py-1 px-2" onClick={() => go("chat")}>
                Open Chat
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto">
    <div className="p-6 space-y-4">
      <div>
        <h1 className="section-title">Run</h1>
        <p className="section-desc">Configure model and llama.cpp launch commands.</p>
      </div>

      {error && (
        <div className="card border-accent-red/30 bg-accent-red/5">
          <p className="text-sm text-accent-red">{error}</p>
          <button className="text-xs text-accent-red/70 hover:text-accent-red mt-1" onClick={() => setError(null)}>
            Dismiss
          </button>
        </div>
      )}

      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,2fr)] gap-4">
        <PresetCard
          activePreset={activePreset}
          presets={presets}
          presetName={presetName}
          presetFlash={presetFlash}
          setPresetName={setPresetName}
          applyPresetName={(n) => applyPresetName(n).catch((err) => setError(String(err)))}
          savePreset={savePreset}
          saveDefaults={saveDefaults}
          resetDefaults={resetDefaults}
          deletePreset={deletePreset}
        />

      <div className="card">
        <div className="flex items-center justify-between mb-3">
          <h2 className="section-title mb-0">Server</h2>
        </div>
        <div className="flex items-center gap-2.5 min-w-0">
          <span className={`w-2.5 h-2.5 rounded-full shrink-0 ${dot}`} />
          {status?.type === "running" && status.ready ? (
            <p className="text-sm font-medium text-accent-green">
              Running on port <span className="font-mono">{status.port}</span>
            </p>
          ) : status?.type === "running" || status?.type === "starting" ? (
            <p className="text-sm font-medium text-accent-yellow">Loading…</p>
          ) : status?.type === "error" ? (
            <p className="text-sm font-medium text-accent-red truncate" title={status.message}>
              {status.message}
            </p>
          ) : (
            <p className="text-sm font-medium text-ink">Stopped</p>
          )}
        </div>
        <p className="text-xs text-dim mt-1.5 truncate" title={routerMode ? appConfig?.harness_roles?.orchestrator ?? "" : config.model_path || selected}>
          {routerMode
            ? `Router — orchestrator: ${
                (appConfig?.harness_roles?.orchestrator ?? "").split(/[\\/]/).pop() ||
                "not set"
              }${appConfig?.harness_roles?.worker ? ` · worker: ${appConfig.harness_roles.worker.split(/[\\/]/).pop()}` : ""}`
            : selectedModel?.name ?? "No model selected"}
        </p>
        <div className="mt-3 flex items-center gap-2">
          {!running ? (
            <button
              className="btn-primary"
              disabled={starting || status?.type === "starting"}
              onClick={start}
            >
              {starting || status?.type === "starting" ? "Starting…" : "Start server"}
            </button>
          ) : (
            <>
              <button className="btn-danger" onClick={stop}>
                Stop server
              </button>
              {status?.type === "running" && status.ready && (
                <button
                  className="btn-secondary"
                  onClick={() => go("chat")}
                  title="Open the Chat tab"
                >
                  <MessageSquare size={12} /> Open Chat
                </button>
              )}
            </>
          )}
        </div>
      </div>
      </div>

      <div className="card">
        <div className="flex items-center justify-between mb-3">
          <h2 className="section-title mb-0">Agent</h2>
          <span className="text-[0.6875rem] text-faint">
            {routerMode ? "Router mode" : "Single-model mode"}
          </span>
        </div>

        {!routerMode ? (
          <div className="space-y-2">
            <p className="section-desc">Load a single model.</p>
            <div className="flex items-center gap-2">
              <select
                className="input flex-1 min-w-0"
                value={config.model_path || selected}
                onChange={(e) => pickModel(e.target.value)}
              >
                <option value="">Select a model…</option>
                {models.map((m) => (
                  <option key={m.id} value={m.path}>
                    {m.name}
                  </option>
                ))}
              </select>
              {selectedModel && (
                <>
                  <span className="text-xs font-mono text-dim shrink-0">
                    ≈ {formatSize(selectedModel.size_bytes ?? 0)}
                  </span>
                  <span className="w-px h-4 bg-border shrink-0" />
                  <div className="flex items-center gap-1.5 shrink-0">
                    {selectedModel.is_vision && (
                      <span title="Vision model — the projector attaches automatically">
                        <Eye size={13} className="text-[#3B82F6]" />
                      </span>
                    )}
                    {selectedModel.is_reasoning && (
                      <span title="Reasoning model">
                        <Brain size={13} className="text-[#E5484D]" />
                      </span>
                    )}
                  </div>
                  <span className="w-px h-4 bg-border shrink-0" />
                </>
              )}
              <div className="flex items-center gap-1.5 shrink-0">
                {preview?.attachments.filter((a) => !a.role).map(attachmentIcon)}
              </div>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            <p className="section-desc">
              One server; models load on demand. The orchestrator loads at start, the worker
              on first delegation.
            </p>
            <div className="grid grid-cols-2 divide-x divide-border items-start">
              <div className="pr-4">
                <p className="text-xs font-bold text-ink mb-2">Orchestrator</p>
                {roleRow("orchestrator")}
              </div>
              <div className="pl-4">
                <p className="text-xs font-bold text-ink mb-2">Worker</p>
                {roleRow("worker")}
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,2fr)] gap-4 items-start">
      <div className="col-start-2 row-start-1">
      <div ref={settingsRef} className="card">
        <h2 className="section-title mb-1">Launch options</h2>
        <p className="section-desc">Every value maps to a llama-server flag; empty optional fields are omitted. Hover options for more info.</p>

        <div className="relative mt-3">
          <Search
            size={14}
            className="absolute left-2.5 top-2.5 text-faint pointer-events-none"
          />
          <input
            ref={searchInputRef}
            type="search"
            value={query}
            placeholder="Search settings — e.g. temperature, GPU layers, cache, context…"
            className="input w-full pl-8 text-xs"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onSearchKeyDown}
            onFocus={() => {
              if (query.trim()) setShowResults(true);
            }}
          />
          {showResults && (
            <div className="absolute z-20 mt-1 w-full bg-surface-1 border border-border rounded shadow-xl max-h-72 overflow-y-auto">
              {matches.length === 0 ? (
                <div className="px-3 py-2 text-xs text-faint">
                  No settings match “{query.trim()}”
                </div>
              ) : (
                matches.slice(0, 30).map((m, i) => (
                  <button
                    key={`${m.text}:${i}`}
                    className={`w-full text-left px-3 py-2 text-xs ${
                      i === searchIdx ? "bg-surface-3 text-ink" : "text-dim"
                    }`}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => goToMatch(m)}
                  >
                    {m.text}
                  </button>
                ))
              )}
            </div>
          )}
        </div>

        <div className="flex gap-1 border-b border-border mt-3">
          {LAUNCH_TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setLaunchTab(t.id)}
              className={`-mb-px px-3 py-1.5 text-xs font-medium border-b-2 whitespace-nowrap transition-colors ${
                launchTab === t.id
                  ? "border-accent text-ink"
                  : "border-transparent text-dim hover:text-ink"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>

        <div data-launch-tab="context" className={launchTab === "context" ? "mt-3" : "hidden"}>
          <div className="space-y-3">
            <div>
              <p className="text-xs font-semibold text-ink">
                Context size{" "}
                <span className="font-mono text-[0.625rem] font-normal text-faint">--ctx-size</span>
              </p>
              <div className="mt-1.5">
                <Toggle
                  label="Override default"
                  checked={overrideCtx}
                  onChange={(v) => set({ n_ctx: v ? maxCtx : 0 })}
                />
              </div>
              {overrideCtx && (
                <div className="mt-2 flex items-center gap-3">
                  <input
                    type="range"
                    min={512}
                    max={maxCtx}
                    step={512}
                    value={config.n_ctx}
                    onChange={(e) => set({ n_ctx: Number(e.target.value) })}
                    className="flex-1 accent-accent"
                  />
                  <span className="text-xs text-ink font-mono w-20 text-right">
                    {config.n_ctx >= 1024 ? `${(config.n_ctx / 1024).toFixed(0)}K` : String(config.n_ctx)}
                  </span>
                </div>
              )}
            </div>
            <div className="grid grid-cols-2 gap-3 mt-3">
              <Field label="KV cache K" flag="--cache-type-k" hint="Key cache precision - lower saves VRAM.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={config.cache_type_k ?? "f16"}
                  onChange={(e) => set({ cache_type_k: e.target.value })}
                >
                  {CACHE_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="KV cache V" flag="--cache-type-v" hint="Value cache precision - lower saves VRAM.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={config.cache_type_v ?? "f16"}
                  onChange={(e) => set({ cache_type_v: e.target.value })}
                >
                  {CACHE_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {t}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Max tokens" flag="--n-predict" hint="-1 = unlimited.">
                <input
                  type="number"
                  min={-1}
                  className="input w-full py-1 px-2 text-xs"
                  value={config.n_predict}
                  onChange={(e) => set({ n_predict: Number(e.target.value) || -1 })}
                />
              </Field>
              <Field label="Keep tokens" flag="--keep" hint="Tokens to keep from the initial prompt (-1 = all).">
                <input
                  type="number"
                  min={-1}
                  placeholder="0"
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("keep")}
                  onChange={(e) => setExtraField("keep", e.target.value)}
                />
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              <Toggle
                label="Context shift"
                flag="--context-shift"
                hint="Shift the context window on infinite generation."
                checked={hasExtra("context-shift")}
                onChange={(v) => setExtraFlag("context-shift", v)}
              />
            </div>
          </div>
        </div>

        <div data-launch-tab="hardware" className={launchTab === "hardware" ? "mt-3" : "hidden"}>
          <Group title="GPU">
            <div className="grid grid-cols-2 gap-3">
              {!routerMode && (
                <Field label="GPU layers" flag="--ngl" hint="-1 = all layers on GPU, 0 = CPU only. Only applies while Fit is off.">
                  <input
                    type="number"
                    min={-1}
                    className="input w-full py-1 px-2 text-xs"
                    value={config.n_gpu_layers}
                    onChange={(e) => set({ n_gpu_layers: Number(e.target.value) || 0 })}
                  />
                </Field>
              )}
              <Field label="Flash attention" flag="--fa" hint="Kernel attention path; auto lets the build decide.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={config.flash_attn ?? "auto"}
                  onChange={(e) => set({ flash_attn: e.target.value })}
                >
                  <option value="auto">auto</option>
                  <option value="on">on</option>
                  <option value="off">off</option>
                </select>
              </Field>
              <Field label="Multi-GPU strategy" flag="--split-mode" hint="How layers split across GPUs; layer = server default.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("split-mode")}
                  onChange={(e) => setExtraField("split-mode", e.target.value)}
                >
                  <option value="">Layer (default)</option>
                  <option value="none">None (single GPU)</option>
                  <option value="row">Row</option>
                  <option value="tensor">Tensor</option>
                </select>
              </Field>
              <Field label="Offload tensors" flag="--ot" hint="Restrict which tensors go to the GPU; default offloads all.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("ot")}
                  onChange={(e) => setExtraField("ot", e.target.value)}
                >
                  <option value="">Default</option>
                  <option value="none">None</option>
                  <option value="ffn_up_cpu">ffn_up_cpu</option>
                  <option value="ffn_cpu">ffn_cpu</option>
                  <option value="exps_cpu">exps_cpu (MoE)</option>
                  <option value="attn_cpu">attn_cpu</option>
                </select>
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              {!routerMode && (
                <Toggle
                  label="Fit"
                  flag="--fit"
                  hint="llama.cpp auto sizes GPU layers to fit (weights + context) in VRAM. GPU layers (--ngl) value is ignored if this is on."
                  checked={fitOn}
                  onChange={setFitOn}
                />
              )}
            </div>
          </Group>
          <Group title="CPU">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Threads" flag="--threads" hint="CPU threads for generation; empty = server default.">
                <input
                  type="number"
                  min={0}
                  placeholder="auto"
                  className="input w-full py-1 px-2 text-xs"
                  value={config.n_threads ?? ""}
                  onChange={(e) =>
                    set({ n_threads: e.target.value === "" ? null : Number(e.target.value) })
                  }
                />
              </Field>
              <Field label="NUMA" flag="--numa" hint="NUMA optimization strategy; disabled = server default.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("numa")}
                  onChange={(e) => setExtraField("numa", e.target.value)}
                >
                  <option value="">Disabled</option>
                  <option value="distribute">Distribute</option>
                  <option value="isolate">Isolate</option>
                  <option value="numactl">numactl</option>
                </select>
              </Field>
              <Field label="N CPU MoE layers" flag="--n-cpu-moe" hint="Keep MoE experts of the first N layers on CPU.">
                <input
                  type="number"
                  min={0}
                  placeholder="0"
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("n-cpu-moe")}
                  onChange={(e) => setExtraField("n-cpu-moe", e.target.value)}
                />
              </Field>
              <Field label="N CPU FFN layers" flag="--n-cpu-ffn" hint="Keep dense FFN weights of the first N layers on CPU.">
                <input
                  type="number"
                  min={0}
                  placeholder="0"
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("n-cpu-ffn")}
                  onChange={(e) => setExtraField("n-cpu-ffn", e.target.value)}
                />
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              <Toggle
                label="CPU MoE"
                flag="--cpu-moe"
                hint="Keep all MoE weights on CPU."
                checked={hasExtra("cpu-moe")}
                onChange={(v) => setExtraFlag("cpu-moe", v)}
              />
            </div>
          </Group>
          <Group title="Memory">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Batch size" flag="--batch-size" hint="Logical batch size; 0 = server default.">
                <input
                  type="number"
                  min={0}
                  className="input w-full py-1 px-2 text-xs"
                  value={config.n_batch}
                  onChange={(e) => set({ n_batch: Number(e.target.value) || 0 })}
                />
              </Field>
              <Field label="Micro-batch" flag="--ubatch-size" hint="Physical batch size; 0 = server default.">
                <input
                  type="number"
                  min={0}
                  className="input w-full py-1 px-2 text-xs"
                  value={config.n_ubatch}
                  onChange={(e) => set({ n_ubatch: Number(e.target.value) || 0 })}
                />
              </Field>
              <Field label="Load mode" flag="--load-mode" hint="How the model file is loaded; auto = mmap.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("load-mode")}
                  onChange={(e) => setExtraField("load-mode", e.target.value)}
                >
                  <option value="">Auto (default)</option>
                  <option value="none">None</option>
                  <option value="mmap">mmap</option>
                  <option value="mlock">mlock</option>
                  <option value="mmap+mlock">mmap+mlock</option>
                  <option value="dio">DirectIO</option>
                </select>
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              <Toggle
                label="Continuous batching"
                flag="--cont-batching"
                hint="New requests join the running batch instead of waiting for a free slot."
                checked={config.cont_batching}
                onChange={(v) => set({ cont_batching: v })}
              />
            </div>
          </Group>
        </div>

        <div data-launch-tab="network" className={launchTab === "network" ? "mt-3" : "hidden"}>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Host" flag="--host" hint="Interface to bind.">
              <input
                className="input w-full py-1 px-2 text-xs font-mono"
                value={config.host}
                onChange={(e) => set({ host: e.target.value })}
              />
            </Field>
            <Field label="Port" flag="--port" hint="Listen port.">
              <input
                type="number"
                min={1}
                max={65535}
                className="input w-full py-1 px-2 text-xs"
                value={config.port}
                onChange={(e) =>
                  set({ port: Math.max(1, Math.min(65535, Number(e.target.value) || 1)) })
                }
              />
            </Field>
            <Field label="Parallel slots" flag="--parallel" hint="Concurrent request slots.">
              <input
                type="number"
                min={1}
                className="input w-full py-1 px-2 text-xs"
                value={config.parallel}
                onChange={(e) => set({ parallel: Math.max(1, Number(e.target.value) || 1) })}
              />
            </Field>
            <Field label="Working directory" flag="cwd" hint="Process working directory; empty = default.">
              <div className="flex gap-1.5">
                <input
                  className="input py-1 px-2 text-xs font-mono flex-1 min-w-0"
                  value={config.working_dir ?? ""}
                  placeholder="default"
                  onChange={(e) => set({ working_dir: e.target.value || null })}
                />
                <button
                  className="btn-ghost text-xs py-1 px-2 shrink-0"
                  title="Browse for a folder"
                  onClick={browseWorkingDir}
                >
                  <FolderOpen size={11} />
                </button>
              </div>
            </Field>
            <Field label="Timeout" flag="--timeout" hint="Read/write timeout in seconds (default 3600).">
              <input
                type="number"
                min={0}
                placeholder="3600"
                className="input w-full py-1 px-2 text-xs"
                value={extraValue("timeout")}
                onChange={(e) => setExtraField("timeout", e.target.value)}
              />
            </Field>
            <Field label="Verbosity" flag="--verbosity" hint="Log verbosity level.">
              <select
                className="input w-full py-1 px-2 text-xs"
                value={extraValue("verbosity")}
                onChange={(e) => setExtraField("verbosity", e.target.value)}
              >
                <option value="">Default</option>
                <option value="0">0 — generic</option>
                <option value="1">1 — errors</option>
                <option value="2">2 — warnings</option>
                <option value="3">3 — info</option>
                <option value="4">4 — debug</option>
              </select>
            </Field>
            <Field label="API key" flag="--api-key" hint="Comma-separated keys for auth.">
              <input
                type="password"
                className="input w-full py-1 px-2 text-xs font-mono"
                value={extraValue("api-key")}
                onChange={(e) => setExtraField("api-key", e.target.value)}
              />
            </Field>
            <Field label="API key file" flag="--api-key-file" hint="File with API keys, one per line.">
              <input
                className="input w-full py-1 px-2 text-xs font-mono"
                value={extraValue("api-key-file")}
                onChange={(e) => setExtraField("api-key-file", e.target.value)}
              />
            </Field>
          </div>
          <div className="mt-4 space-y-3">
            <Toggle
              label="Web UI"
              flag="--no-webui"
              hint="Serve llama.cpp's built-in web interface."
              checked={!hasExtra("no-webui")}
              onChange={(v) => setExtraFlag("no-webui", !v)}
            />
            <Toggle
              label="WebUI MCP proxy"
              flag="--webui-mcp-proxy"
              hint="Enable the experimental MCP CORS proxy."
              checked={hasExtra("webui-mcp-proxy")}
              onChange={(v) => setExtraFlag("webui-mcp-proxy", v)}
            />
            <Toggle
              label="Warmup"
              flag="--no-warmup"
              hint="Warm up the model on startup."
              checked={!hasExtra("no-warmup")}
              onChange={(v) => setExtraFlag("no-warmup", !v)}
            />
          </div>
        </div>

        <div data-launch-tab="sampling" className={launchTab === "sampling" ? "mt-3" : "hidden"}>
          <div className={sampling ? "" : "opacity-50"}>
            <Toggle
              label="Disable sampling"
              flag="no flags"
              hint="Send no sampling flags; server and harness defaults apply."
              checked={config.disable_sampling ?? false}
              onChange={(v) => set({ disable_sampling: v })}
            />
            <div className="grid grid-cols-2 gap-3 mt-3">
              <SliderField
                label="Temperature"
                flag="--temp"
                hint="Randomness; 0 = greedy."
                min={0}
                max={2}
                step={0.05}
                value={config.temperature ?? 0.8}
                disabled={!sampling}
                format={(v) => v.toFixed(2)}
                onChange={(v) => set({ temperature: v })}
              />
              <SliderField
                label="Top-K"
                flag="--top-k"
                hint="Keep the K most likely tokens."
                min={0}
                max={200}
                step={1}
                value={config.top_k}
                disabled={!sampling}
                onChange={(v) => set({ top_k: Math.round(v) })}
              />
              <SliderField
                label="Top-P"
                flag="--top-p"
                hint="Nucleus sampling cutoff."
                min={0}
                max={1}
                step={0.01}
                value={config.top_p ?? 0.95}
                disabled={!sampling}
                format={(v) => v.toFixed(2)}
                onChange={(v) => set({ top_p: v })}
              />
              <SliderField
                label="Min-P"
                flag="--min-p"
                hint="Drop tokens below this probability."
                min={0}
                max={1}
                step={0.01}
                value={config.min_p ?? 0.05}
                disabled={!sampling}
                format={(v) => v.toFixed(2)}
                onChange={(v) => set({ min_p: v })}
              />
              <Field
                label="Repeat penalty"
                flag="--repeat-penalty"
                hint="Penalize repeated tokens; 1.0 disables. Empty = server default."
              >
                <input
                  type="number"
                  min={0}
                  step={0.01}
                  placeholder="1.0"
                  disabled={!sampling}
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("repeat-penalty")}
                  onChange={(e) => setExtraField("repeat-penalty", e.target.value)}
                />
              </Field>
              <Field
                label="Presence penalty"
                flag="--presence-penalty"
                hint="Penalize tokens that already occurred; 0 disables, negative encourages reuse. Empty = server default."
              >
                <input
                  type="number"
                  min={-2}
                  max={2}
                  step={0.05}
                  placeholder="0.0"
                  disabled={!sampling}
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("presence-penalty")}
                  onChange={(e) => setExtraField("presence-penalty", e.target.value)}
                />
              </Field>
              <Field label="Seed" flag="--seed" hint="Empty = random.">
                <input
                  type="number"
                  min={0}
                  placeholder="random"
                  disabled={!sampling}
                  className="input w-full py-1 px-2 text-xs"
                  value={config.seed ?? ""}
                  onChange={(e) =>
                    set({ seed: e.target.value === "" ? null : Math.max(0, Number(e.target.value)) })
                  }
                />
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              <Toggle
                label="Ignore EoS"
                flag="--ignore-eos"
                hint="Keep generating past the end-of-stream token."
                checked={hasExtra("ignore-eos")}
                onChange={(v) => setExtraFlag("ignore-eos", v)}
              />
            </div>
          </div>
        </div>

        <div data-launch-tab="chat" className={launchTab === "chat" ? "mt-3" : "hidden"}>
          <Group title="Template">
            <div className="grid grid-cols-2 gap-3">
              <Field
                label="Template file"
                flag="--chat-template-file"
                hint="Explicit Jinja template file; empty keeps the auto-attached sibling."
              >
                <input
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder="auto (sibling .jinja)"
                  value={templateFile}
                  onChange={(e) => setExtraField("chat-template-file", e.target.value)}
                />
              </Field>
              <Field
                label="Template kwargs"
                flag="--chat-template-kwargs"
                hint="JSON object passed to the template."
              >
                <input
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder='{"key":"value"}'
                  value={templateKwargs}
                  onChange={(e) => setExtraField("chat-template-kwargs", e.target.value)}
                />
              </Field>
            </div>
            <div className="mt-4 space-y-3">
              <Toggle
                label="Jinja"
                flag="--no-jinja"
                hint="Use the Jinja template engine."
                checked={!hasExtra("no-jinja")}
                onChange={(v) => setExtraFlag("no-jinja", !v)}
              />
            </div>
          </Group>
          <Group title="Reasoning">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Reasoning" flag="--reasoning" hint="Enable model thinking.">
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("reasoning")}
                  onChange={(e) => setExtraField("reasoning", e.target.value)}
                >
                  <option value="">Auto (default)</option>
                  <option value="on">On</option>
                  <option value="off">Off</option>
                </select>
              </Field>
              <Field
                label="Reasoning format"
                flag="--reasoning-format"
                hint="How reasoning is parsed out of the output."
              >
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("reasoning-format")}
                  onChange={(e) => setExtraField("reasoning-format", e.target.value)}
                >
                  <option value="">Auto (default)</option>
                  <option value="none">None</option>
                  <option value="deepseek">DeepSeek</option>
                  <option value="deepseek-legacy">DeepSeek legacy</option>
                </select>
              </Field>
              <Field
                label="Reasoning budget"
                flag="--reasoning-budget"
                hint="-1 = unrestricted, 0 = immediate end."
              >
                <input
                  type="number"
                  min={-1}
                  placeholder="-1"
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("reasoning-budget")}
                  onChange={(e) => setExtraField("reasoning-budget", e.target.value)}
                />
              </Field>
              <Field
                label="Budget message"
                flag="--reasoning-budget-message"
                hint="Message injected when the budget is exhausted."
              >
                <input
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("reasoning-budget-message")}
                  onChange={(e) => setExtraField("reasoning-budget-message", e.target.value)}
                />
              </Field>
              <Field
                label="Reasoning effort"
                flag="--reasoning-effort"
                hint="Effort level requested from the chat template."
              >
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("reasoning-effort")}
                  onChange={(e) => setExtraField("reasoning-effort", e.target.value)}
                >
                  <option value="">Default</option>
                  <option value="minimal">Minimal</option>
                  <option value="low">Low</option>
                  <option value="medium">Medium</option>
                  <option value="high">High</option>
                  <option value="xhigh">XHigh</option>
                  <option value="max">Max</option>
                </select>
              </Field>
            </div>
          </Group>
          <Group title="Output">
            <div className="space-y-3">
              <Toggle
                label="Escape sequences"
                flag="--no-escape"
                hint={"Process \\n, \\t and similar escape sequences."}
                checked={!hasExtra("no-escape")}
                onChange={(v) => setExtraFlag("no-escape", !v)}
              />
              <Toggle
                label="Special tokens"
                flag="--special"
                hint="Output special tokens."
                checked={hasExtra("special")}
                onChange={(v) => setExtraFlag("special", v)}
              />
              <Toggle
                label="SPM infill"
                flag="--spm-infill"
                hint="Use the Suffix/Prefix/Middle infill pattern."
                checked={hasExtra("spm-infill")}
                onChange={(v) => setExtraFlag("spm-infill", v)}
              />
            </div>
          </Group>
        </div>

        <div data-launch-tab="extra" className={launchTab === "extra" ? "mt-3" : "hidden"}>
          <Group title="RoPE">
            <div className="grid grid-cols-2 gap-3">
              <Field label="RoPE base" flag="--rope-freq-base" hint="Frequency base; empty = model default.">
                <input
                  type="number"
                  step={100}
                  min={0}
                  placeholder="model default"
                  className="input w-full py-1 px-2 text-xs"
                  value={config.rope_freq_base ?? ""}
                  onChange={(e) =>
                    set({ rope_freq_base: e.target.value === "" ? null : Number(e.target.value) })
                  }
                />
              </Field>
              <Field label="RoPE scale" flag="--rope-freq-scale" hint="Frequency scaling; empty = model default.">
                <input
                  type="number"
                  step={0.1}
                  min={0}
                  placeholder="model default"
                  className="input w-full py-1 px-2 text-xs"
                  value={config.rope_freq_scale ?? ""}
                  onChange={(e) =>
                    set({ rope_freq_scale: e.target.value === "" ? null : Number(e.target.value) })
                  }
                />
              </Field>
            </div>
          </Group>
          {!routerMode && (
          <Group title="Speculative decoding">
            <div className="grid grid-cols-2 gap-3">
              <Field
                label="Spec type"
                flag="--spec-type"
                hint="Draft-model or n-gram speculation strategy."
              >
                <select
                  className="input w-full py-1 px-2 text-xs"
                  value={extraValue("spec-type")}
                  onChange={(e) => setExtraField("spec-type", e.target.value)}
                >
                  <option value="">Disabled</option>
                  <option value="draft-simple">draft-simple</option>
                  <option value="draft-mtp">draft-mtp</option>
                  <option value="draft-dspark">draft-dspark</option>
                  <option value="draft-eagle3">draft-eagle3</option>
                  <option value="ngram-cache">ngram-cache</option>
                  <option value="ngram-simple">ngram-simple</option>
                  <option value="ngram-mod">ngram-mod</option>
                </select>
              </Field>
              <Field
                label="Draft model"
                flag="--spec-draft-model"
                hint="Draft model used for speculation."
              >
                <input
                  className="input w-full py-1 px-2 text-xs font-mono"
                  placeholder="path to draft.gguf"
                  value={extraValue("spec-draft-model")}
                  onChange={(e) => setExtraField("spec-draft-model", e.target.value)}
                />
              </Field>
              <Field label="Draft max tokens" flag="--spec-draft-n-max" hint="Max draft tokens (default 16).">
                <input
                  type="number"
                  min={1}
                  className="input w-full py-1 px-2 text-xs"
                  placeholder="16"
                  value={extraValue("spec-draft-n-max")}
                  onChange={(e) => setExtraField("spec-draft-n-max", e.target.value)}
                />
              </Field>
              <Field label="Draft min tokens" flag="--spec-draft-n-min" hint="Min draft tokens (default 0).">
                <input
                  type="number"
                  min={0}
                  className="input w-full py-1 px-2 text-xs"
                  placeholder="0"
                  value={extraValue("spec-draft-n-min")}
                  onChange={(e) => setExtraField("spec-draft-n-min", e.target.value)}
                />
              </Field>
              <Field
                label="Draft min probability"
                flag="--spec-draft-p-min"
                hint="Min probability for greedy drafts (default 0.75)."
              >
                <input
                  type="number"
                  step={0.01}
                  min={0}
                  max={1}
                  className="input w-full py-1 px-2 text-xs"
                  placeholder="0.75"
                  value={extraValue("spec-draft-p-min")}
                  onChange={(e) => setExtraField("spec-draft-p-min", e.target.value)}
                />
              </Field>
            </div>
          </Group>
          )}
          <Group title="Flags">
          <p className="text-[0.6875rem] text-faint mb-2">
            Named flags pass as <span className="font-mono">--name value</span>; empty values send
            the flag alone. Raw arguments are appended verbatim.
          </p>
          <div className="space-y-1.5">
            {extraRows.map((row, i) => (
              <div key={i} className="flex items-center gap-1.5">
                <input
                  className="input py-1 px-2 text-xs font-mono w-40 shrink-0"
                  placeholder="flag"
                  value={row.key}
                  onChange={(e) =>
                    setExtraRows((rows) =>
                      rows.map((r, j) => (j === i ? { ...r, key: e.target.value } : r)),
                    )
                  }
                />
                <input
                  className="input py-1 px-2 text-xs font-mono flex-1 min-w-0"
                  placeholder="value (optional)"
                  value={row.value}
                  onChange={(e) =>
                    setExtraRows((rows) =>
                      rows.map((r, j) => (j === i ? { ...r, value: e.target.value } : r)),
                    )
                  }
                />
                <button
                  className="text-faint hover:text-accent-red shrink-0"
                  title="Remove flag"
                  onClick={() => setExtraRows((rows) => rows.filter((_, j) => j !== i))}
                >
                  <X size={11} />
                </button>
              </div>
            ))}
            <button
              className="btn-ghost text-xs py-1 px-2"
              onClick={() => setExtraRows((rows) => [...rows, { key: "", value: "" }])}
            >
              <Plus size={11} /> Add flag
            </button>
          </div>
          <div className="mt-3">
            <label className="label">
              Raw arguments <span className="font-mono text-[0.625rem] text-faint">verbatim</span>
            </label>
            <textarea
              className="input w-full mt-1 font-mono text-xs min-h-[64px]"
              placeholder="--some-flag value"
              value={rawArgs}
              onChange={(e) => setRawArgs(e.target.value)}
            />
          </div>
          </Group>
        </div>
      </div>

      </div>
      <div className="col-start-1 row-start-1 space-y-4">
        <EstimateCard
          estimate={estimate}
          notes={suggestionNotes ?? []}
          estimating={estimating}
          canEstimate={!externalMode && !!(config.model_path || selected)}
          onEstimate={autoEstimate}
        />

        <CommandCard
          preview={preview}
          command={launchCommand}
          copied={copiedLaunch}
          onCopy={copyLaunch}
        />

        <LogCard logs={logs} logRef={logRef} copied={copiedLogs} onCopy={copyLogs} />
      </div>
      </div>
    </div>
    </div>
  );
}

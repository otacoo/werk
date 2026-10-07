import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  Cpu,
  Database,
  MemoryStick,
  Monitor,
  RefreshCw,
  Search,
  Zap,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  FoundBinaryDto,
  HfFileDto,
  HfModel,
  ModelDto,
  ReleaseDto,
  RuntimeInfo,
  ServerStatus,
  SystemInfoDto,
} from "../bindings";
import { call } from "../utils/ipc";
import { t } from "../utils/i18n";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { RuntimeTab } from "./dashboard/runtime-tab";
import { ModelsTab } from "./dashboard/models-tab";
import { BrowseTab } from "./dashboard/browse-tab";
import { baseOf, MODEL_SORTERS, type DashTab, type HfSort, type ModelSortCol } from "./dashboard/shared";
import {
  formatSize,
  isDsparkFile,
  isMmprojFile,
  mbToGb,
  mmprojSaveName,
  parseSplitSuffix,
  shortCpuName,
  shortGpuName,
} from "../utils/format";
import type { Tab } from "../App";
import type { ActiveDl } from "./dashboard/shared";

interface Bounded<T> {
  what: string;
  value?: T;
  error?: string;
}

export default function Dashboard({ go }: { go: (t: Tab) => void }) {
  const [system, setSystem] = useState<SystemInfoDto | null>(null);
  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [models, setModels] = useState<ModelDto[]>([]);
  const [status, setStatus] = useState<ServerStatus>(getServerStatus());
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Runtimes.
  const [release, setRelease] = useState<ReleaseDto | null>(null);
  const [selectedAsset, setSelectedAsset] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [rtBusy, setRtBusy] = useState(false);
  const [rtProgress, setRtProgress] = useState<{ downloaded: number; total: number | null } | null>(null);
  const [customBuilds, setCustomBuilds] = useState<FoundBinaryDto[] | null>(null);
  const [scanning, setScanning] = useState(false);

  // Installed models + folders.
  const [nameFilter, setNameFilter] = useState("");
  const [modelSortCol, setModelSortCol] = useState<ModelSortCol>("name");
  const [modelSortDir, setModelSortDir] = useState<"asc" | "desc">("asc");
  const [configModel, setConfigModel] = useState<ModelDto | null>(null);
  const [dashTab, setDashTab] = useState<DashTab>("models");
  const [modelDirs, setModelDirs] = useState<string[]>([]);
  const [downloadDir, setDownloadDir] = useState("");

  // HuggingFace browser.
  const [searchQuery, setSearchQuery] = useState("");
  const [sortBy, setSortBy] = useState<HfSort>("downloads");
  const [searching, setSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<HfModel[]>([]);
  const [expandedRepo, setExpandedRepo] = useState<string | null>(null);
  const [repoFiles, setRepoFiles] = useState<Record<string, HfFileDto[]>>({});
  const [active, setActive] = useState<Record<string, ActiveDl>>({});
  const [progress, setProgress] = useState<Record<string, { downloaded: number; total: number | null }>>({});
  const [paused, setPaused] = useState<Record<string, boolean>>({});
  const [companion, setCompanion] = useState<{
    repoId: string;
    file: HfFileDto;
    mmproj: HfFileDto[];
    dspark: HfFileDto[];
    parts: HfFileDto[];
  } | null>(null);

  /// Bounds a load call so a stalled backend can't pin the page on "Loading".
  const withTimeout = <T,>(p: Promise<T>, what: string): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(t("{what} timed out — the backend may be busy", { what }))),
        15000,
      );
      p.then(
        (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        (e) => {
          clearTimeout(timer);
          reject(e);
        },
      );
    });

  const bounded = async <T,>(p: Promise<T>, what: string): Promise<Bounded<T>> => {
    try {
      return { what, value: await withTimeout(p, what) };
    } catch (e) {
      return { what, error: String(e) };
    }
  };



  const loadData = async () => {
    setLoading(true);
    // Every call is bounded and reported individually: with Promise.all a
    // single timeout would always surface as "System info" (first in line).
    const fetchers = (): [
      Promise<Bounded<SystemInfoDto>>,
      Promise<Bounded<RuntimeInfo>>,
      Promise<Bounded<ModelDto[]>>,
      Promise<Bounded<ServerStatus>>,
      Promise<Bounded<AppConfig>>,
    ] => [
      bounded(call(commands.getSystemInfo()), t("System info")),
      bounded(call(commands.getRuntimeInfo()), t("Runtime info")),
      bounded(call(commands.listInstalledModels()), t("Model scan")),
      bounded(call(commands.getServerStatus()), t("Server status")),
      bounded(call(commands.getConfig()), t("Config")),
    ];
    try {
      const run = async () => {
        const [sys, rt, mdls, srv, cfg] = await Promise.all(fetchers());
        return { sys, rt, mdls, srv, cfg };
      };
      let r = await run();
      if ([r.sys, r.rt, r.mdls, r.srv, r.cfg].some((x) => x.error)) {
        // One quiet retry: transient stalls (busy disk, cold caches) recover.
        await new Promise((res) => setTimeout(res, 800));
        r = await run();
      }
      const { sys, rt, mdls, srv, cfg } = r;
      if (sys.value !== undefined) setSystem(sys.value);
      if (rt.value !== undefined) setRuntime(rt.value);
      if (mdls.value !== undefined) setModels(mdls.value);
      if (srv.value !== undefined) setStatus(srv.value);
      if (cfg.value !== undefined) setAppConfig(cfg.value);
      const failed = [sys, rt, mdls, srv, cfg].filter((x) => x.error);
      if (failed.length > 0) {
        setError(
          t("Could not load the dashboard: {list}", {
            list: failed.map((f) => `${f.what}: ${f.error}`).join(" · "),
          }),
        );
      } else {
        setError(null);
      }
    } catch (e) {
      setError(t("Could not load the dashboard: {error}", { error: String(e) }));
    } finally {
      setLoading(false);
    }
  };

  const reloadDirs = async () => {
    try {
      const info = await call(commands.getModelDirs());
      setModelDirs(info.dirs);
      setDownloadDir(info.download_dir);
    } catch {}
  };

  useEffect(() => {
    loadData();
    reloadDirs();
    const unsubStatus = subscribeServerStatus(setStatus);
    const unlisten = listen<{ id: string; downloaded: number; total?: number | null }>(
      "download_progress",
      (e) => {
        const p = e.payload;
        if (p.id.startsWith("runtime:")) {
          setRtProgress({ downloaded: p.downloaded, total: p.total ?? null });
        } else {
          setProgress((prev) => ({
            ...prev,
            [p.id]: { downloaded: p.downloaded, total: p.total ?? null },
          }));
        }
      },
    );
    return () => {
      unsubStatus();
      unlisten.then((f) => f());
    };
  }, []);

  const openInBrowser = (url: string) => {
    openUrl(url).catch((e) => setError(String(e)));
  };

  // ── Server ────────────────────────────────────────────────────────

  const running = status.type === "running" || status.type === "starting";

  const stop = async () => {
    setStopping(true);
    try {
      await call(commands.stopServer());
      setStatus({ type: "stopped" });
    } catch (e) {
      setError(String(e));
    } finally {
      setStopping(false);
    }
  };

  // Mutations share one error surface; most also re-read the dashboard.
  const run = async (action: () => Promise<unknown>, reload = true) => {
    try {
      await action();
      if (reload) await loadData();
    } catch (e) {
      setError(String(e));
    }
  };

  const pickFolder = async (title: string): Promise<string | null> => {
    try {
      const picked = await open({ directory: true, title });
      return typeof picked === "string" && picked ? picked : null;
    } catch (e) {
      setError(String(e));
      return null;
    }
  };

  const deleteModel = (path: string) => run(() => call(commands.deleteModel(path)));

  const setAutoDeleteOld = async (enabled: boolean) => {
    setAppConfig((c) => (c ? { ...c, auto_delete_old_runtimes: enabled } : c));
    try {
      await call(commands.setAutoDeleteRuntimes(enabled));
    } catch (e) {
      setError(String(e));
      await loadData();
    }
  };

  // ── Runtimes ──────────────────────────────────────────────────────

  const checkForBuilds = async () => {
    setChecking(true);
    setError(null);
    try {
      const rel = await call(commands.checkRelease());
      setRelease(rel);
      setSelectedAsset(rel.assets[0]?.name ?? null);
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const installAsset = async (assetName: string) => {
    setError(null);
    setRtBusy(true);
    setRtProgress({ downloaded: 0, total: null });
    try {
      await call(commands.downloadRuntimeAsset(assetName));
      // Installed: collapse the list instead of leaving a stale picker.
      setRelease(null);
      setSelectedAsset(null);
      await loadData();
    } catch (e) {
      if (!/cancel/i.test(String(e))) setError(String(e));
    } finally {
      setRtProgress(null);
      setRtBusy(false);
    }
  };

  const cancelInstall = () => {
    if (!selectedAsset) return;
    call(commands.cancelDownload(`runtime:${selectedAsset}`)).catch(() => {});
  };

  const registerBuild = (binaryPath: string) =>
    run(async () => {
      await call(commands.addCustomRuntime(binaryPath));
      setCustomBuilds(null);
    });

  const browseCustom = async () => {
    const picked = await pickFolder(t("Select llama.cpp directory"));
    if (!picked) return;
    setScanning(true);
    setError(null);
    try {
      const builds = await call(commands.scanCustomBinaries(picked));
      if (builds.length === 0) {
        setCustomBuilds(null);
        setError(t("No llama-server binary found in the selected directory."));
      } else if (builds.length === 1) {
        await registerBuild(builds[0].binary_path);
      } else {
        setCustomBuilds(builds);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setScanning(false);
    }
  };

  const activateManaged = (build: number, backendId: string) =>
    run(() => call(commands.setActiveRuntime({ type: "managed", build, backend_id: backendId })));

  const activateCustom = (index: number) =>
    run(() => call(commands.setActiveRuntime({ type: "custom", index })));

  const deleteManaged = (build: number, backendId: string) =>
    run(() => call(commands.deleteManagedRuntime(build, backendId)));

  const removeCustom = (index: number) => run(() => call(commands.removeCustomRuntime(index)));

  const deleteOldVersions = async () => {
    const current = runtime?.active;
    const stale = (runtime?.managed ?? []).filter(
      (r) =>
        !(
          current?.type === "managed" &&
          current.build === r.build &&
          (current.backend_id ?? "") === r.backend_id
        ),
    );
    if (stale.length === 0) return;
    const confirmText =
      stale.length === 1
        ? t("Delete 1 older build?")
        : t("Delete {n} older builds?", { n: stale.length });
    if (!window.confirm(confirmText)) return;
    await run(async () => {
      for (const r of stale) await call(commands.deleteManagedRuntime(r.build, r.backend_id));
    });
  };

  const managed = runtime?.managed ?? [];
  const custom = runtime?.custom ?? [];
  const activeRt = runtime?.active;
  const assets = release?.assets ?? [];
  const latestInstalled = managed.reduce((max, r) => Math.max(max, r.build), 0);
  const newBuildAvailable = release != null && managed.length > 0 && release.build > latestInstalled;

  // ── Installed models + folders ────────────────────────────────────

  const filteredModels = useMemo(() => {
    const needle = nameFilter.trim().toLowerCase();
    const matches = needle
      ? models.filter(
          (m) =>
            m.name.toLowerCase().includes(needle) || m.filename.toLowerCase().includes(needle),
        )
      : models;
    const order = modelSortDir === "asc" ? 1 : -1;
    return [...matches].sort((a, b) => order * MODEL_SORTERS[modelSortCol](a, b));
  }, [models, nameFilter, modelSortCol, modelSortDir]);

  const toggleModelSort = (col: ModelSortCol) => {
    if (modelSortCol === col) setModelSortDir((d) => (d === "asc" ? "desc" : "asc"));
    else {
      setModelSortCol(col);
      setModelSortDir("asc");
    }
  };

  const addModelDir = async () => {
    const picked = await pickFolder(t("Add GGUF storage directory"));
    if (!picked) return;
    await run(async () => {
      await call(commands.addModelDir(picked));
      await reloadDirs();
    });
  };

  const removeModelDir = (path: string) =>
    run(async () => {
      await call(commands.removeModelDir(path));
      await reloadDirs();
    });

  const changeDownloadDir = async () => {
    const picked = await pickFolder(t("Select default download directory"));
    if (!picked) return;
    await run(async () => {
      await call(commands.setDownloadDir(picked));
      await reloadDirs();
    }, false);
  };

  // ── HuggingFace browser ───────────────────────────────────────────

  const doSearch = async (sort: HfSort = sortBy) => {
    if (!searchQuery.trim()) return;
    setSearching(true);
    setError(null);
    try {
      setSearchResults(await call(commands.searchHfModels(searchQuery.trim(), sort)));
    } catch (e) {
      setError(String(e));
    } finally {
      setSearching(false);
    }
  };

  const changeSortBy = (sort: HfSort) => {
    setSortBy(sort);
    if (searchResults.length > 0) void doSearch(sort);
  };

  const loadRepoFiles = async (repoId: string) => {
    try {
      const files = await call(commands.getHfRepoFiles(repoId));
      setRepoFiles((prev) => ({ ...prev, [repoId]: files }));
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleRepo = (repoId: string) => {
    if (expandedRepo === repoId) {
      setExpandedRepo(null);
      return;
    }
    setExpandedRepo(repoId);
    if (!repoFiles[repoId]) void loadRepoFiles(repoId);
  };

  const startDownload = async (repoId: string, filename: string, saveAs: string | null, split: string[] | null) => {
    const id = filename;
    setActive((prev) => ({ ...prev, [id]: { repoId, filename, saveAs, split } }));
    setPaused((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    try {
      await call(commands.downloadModel(repoId, filename, split, saveAs));
      await loadData();
    } catch (e) {
      if (!/cancel/i.test(String(e))) setError(String(e));
    } finally {
      setActive((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setProgress((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setPaused((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    }
  };

  const pauseDownload = async (id: string) => {
    setPaused((prev) => ({ ...prev, [id]: true }));
    try {
      await call(commands.pauseDownload(id));
    } catch (e) {
      setError(String(e));
    }
  };

  const resumeDownload = async (id: string) => {
    const a = active[id];
    if (!a) return;
    await startDownload(a.repoId, a.filename, a.saveAs, a.split);
  };

  const cancelDownload = async (id: string) => {
    const a = active[id];
    if (!a) return;
    try {
      await call(commands.discardDownload(a.repoId, a.filename, a.split, a.saveAs));
    } catch (e) {
      setError(String(e));
    }
    setActive((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setProgress((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setPaused((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  const dirOf = (path: string) => path.slice(0, path.lastIndexOf("/") + 1);

  /// Split siblings of `path` (same folder, same base), part 1 first.
  const splitParts = (repoId: string, path: string): HfFileDto[] => {
    const info = parseSplitSuffix(baseOf(path));
    if (!info) return [];
    const dir = dirOf(path);
    return (repoFiles[repoId] ?? [])
      .filter((f) => {
        const s = parseSplitSuffix(baseOf(f.path));
        return s != null && s.base === info.base && s.total === info.total && dirOf(f.path) === dir;
      })
      .sort(
        (a, b) =>
          (parseSplitSuffix(baseOf(a.path))?.index ?? 0) -
          (parseSplitSuffix(baseOf(b.path))?.index ?? 0),
      );
  };

  /// Companion saved beside the model, named after its split base.
  const mmprojSaveNameFor = (modelPath: string, mmprojPath: string) => {
    const stem = parseSplitSuffix(baseOf(modelPath))?.base ?? baseOf(modelPath);
    return dirOf(modelPath) + mmprojSaveName(stem, baseOf(mmprojPath));
  };

  const downloadClick = (repoId: string, file: HfFileDto) => {
    const files = repoFiles[repoId] ?? [];
    if (isMmprojFile(file.path) || isDsparkFile(file.path)) {
      startDownload(repoId, file.path, null, null);
      return;
    }
    const parts = splitParts(repoId, file.path);
    const mmproj = files.filter((f) => isMmprojFile(f.path));
    const dspark = files.filter((f) => isDsparkFile(f.path));
    if (parts.length > 1 || mmproj.length > 0 || dspark.length > 0) {
      setCompanion({ repoId, file, mmproj, dspark, parts });
      return;
    }
    startDownload(repoId, file.path, null, null);
  };

  /// `allParts` enqueues the remaining split parts after the clicked one.
  const companionChoice = (allParts: boolean, mp: HfFileDto | null, dp: HfFileDto | null) => {
    if (!companion) return;
    const { repoId, file, parts } = companion;
    setCompanion(null);
    const rest = allParts ? parts.filter((p) => p.path !== file.path).map((p) => p.path) : [];
    startDownload(repoId, file.path, null, rest.length > 0 ? rest : null);
    if (mp) {
      startDownload(repoId, mp.path, mmprojSaveNameFor(file.path, mp.path), null);
    }
    if (dp) {
      startDownload(repoId, dp.path, null, null);
    }
  };

  if (loading) {
    return (
      <div className="h-full overflow-y-auto flex items-center justify-center text-dim text-sm">
        {t("Loading system info…")}
      </div>
    );
  }

  const totalVram = system?.gpus.reduce((s, g) => s + g.vram_mb, 0) ?? 0;
  const totalRam = system?.total_ram_mb ?? 0;
  const freeRam = system?.available_ram_mb ?? 0;
  const usedRam = Math.max(0, totalRam - freeRam);
  const gpus = system?.gpus ?? [];
  const selectedModelName =
    (appConfig?.selected_model ?? "").split(/[\\/]/).pop() || t("None selected");
  const recommendedBackend = system?.backends.find(
    (b) => b.id === system.recommended_backend,
  );
  const runtimeLabel = (() => {
    if (activeRt?.type === "managed") {
      const hit = managed.find(
        (r) => r.build === activeRt.build && (r.backend_id ?? "") === (activeRt.backend_id ?? ""),
      );
      return `b${activeRt.build}${hit ? ` · ${hit.backend_label}` : ""}`;
    }
    if (activeRt?.type === "custom") {
      return custom[activeRt.index]?.label ?? t("Custom build");
    }
    return t("None");
  })();
  const installedNames = new Set(models.map((m) => m.filename));
  const dashTabs: { id: DashTab; label: string; desc: string; icon: typeof Cpu }[] = [
    {
      id: "runtime",
      label: t("Runtime ({n})", { n: managed.length + custom.length }),
      desc: t("llama.cpp builds used to serve models."),
      icon: Cpu,
    },
    {
      id: "models",
      label: t("Models ({n})", { n: models.length }),
      desc: t("GGUF files found across your model folders."),
      icon: Database,
    },
    {
      id: "browse",
      label: t("Browse HuggingFace"),
      desc: t("Search and download GGUF models from HuggingFace."),
      icon: Search,
    },
  ];
  const currentTab = dashTabs.find((tab) => tab.id === dashTab) ?? dashTabs[0];

  const comp = companion
    ? (() => {
        const split = companion.parts.length > 1;
        const info = parseSplitSuffix(baseOf(companion.file.path));
        const modelSize = split
          ? companion.parts.reduce((s, f) => s + (f.size_bytes ?? 0), 0)
          : (companion.file.size_bytes ?? 0);
        return {
          split,
          index: info?.index ?? 1,
          modelSize,
          label: split
            ? t("All {n} parts", { n: companion.parts.length })
            : t("Model"),
        };
      })()
    : null;

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-6xl mx-auto">
        <div>
          <h1 className="section-title">{t("Dashboard")}</h1>
          <p className="section-desc">
            {t("Your machine, the local server, llama.cpp builds, and models.")}
          </p>
        </div>

        {error && (
          <div className="card border-accent-red/30 bg-accent-red/5">
            <p className="text-sm text-accent-red">{error}</p>
            <div className="flex items-center gap-3 mt-1">
              <button
                className="text-xs text-accent hover:underline"
                onClick={() => {
                  setError(null);
                  loadData();
                }}
              >
                {t("Retry")}
              </button>
              <button
                className="text-xs text-accent-red/70 hover:text-accent-red"
                onClick={() => setError(null)}
              >
                {t("Dismiss")}
              </button>
            </div>
          </div>
        )}

        {/* Server and machine. */}
        <div className="grid gap-4 lg:grid-cols-5 items-stretch">
          <div className="card lg:col-span-2 flex flex-col">
            <div className="flex items-center gap-2">
              <span
                className={`w-2.5 h-2.5 rounded-full shrink-0 ${
                  status.type === "running"
                    ? status.ready
                      ? "bg-accent-green"
                      : "bg-accent-yellow animate-pulse"
                    : status.type === "starting"
                      ? "bg-accent-yellow animate-pulse"
                      : status.type === "error"
                        ? "bg-accent-red"
                        : "bg-surface-3"
                }`}
              />
              <h2 className="section-title mb-0">{t("Server")}</h2>
              {status.type === "running" && status.ready && (
                <span className="badge-green text-[0.625rem] ml-auto">{t("ready")}</span>
              )}
              {running && !(status.type === "running" && status.ready) && (
                <span className="badge-yellow text-[0.625rem] ml-auto">{t("loading")}</span>
              )}
            </div>
            <p
              className={`mt-3 text-lg font-semibold ${
                status.type === "running" && status.ready
                  ? "text-accent-green"
                  : running
                    ? "text-accent-yellow"
                    : status.type === "error"
                      ? "text-accent-red"
                      : "text-ink"
              }`}
            >
              {status.type === "running" && status.ready ? (
                <>
                  {t("Running")} <span className="font-mono text-base">:{status.port}</span>
                </>
              ) : running ? (
                t("Loading the model…")
              ) : status.type === "error" ? (
                t("Server error")
              ) : (
                t("Stopped")
              )}
            </p>
            <p className="section-desc mt-0.5">
              {status.type === "error"
                ? status.message
                : status.type === "running" && status.ready
                  ? t("Ready for chat.")
                  : t("Server starts on demand when you send a message.")}
            </p>
            <div className="mt-4 space-y-1.5">
              <DetailRow label={t("Model")} value={selectedModelName} mono />
              <DetailRow label={t("Runtime")} value={runtimeLabel} />
              <DetailRow
                label={t("Backend")}
                value={recommendedBackend?.label ?? "—"}
              />
            </div>
            <div className="mt-auto pt-4 flex items-center gap-2">
              {running ? (
                <>
                  {status.type === "running" && (
                    <button className="btn-secondary text-xs" onClick={() => go("chat")}>
                      {t("Chat")}
                    </button>
                  )}
                  <button className="btn-danger text-xs" onClick={stop} disabled={stopping}>
                    {t("Stop")}
                  </button>
                </>
              ) : (
                <>
                  <button className="btn-primary text-xs" onClick={() => go("chat")}>
                    {t("Chat")}
                  </button>
                  <button className="btn-ghost text-xs" onClick={() => go("run")}>
                    {t("Configure launch")}
                  </button>
                </>
              )}
            </div>
          </div>

          <div className="card lg:col-span-3">
            <div className="flex items-baseline justify-between gap-3 mb-3">
              <h2 className="section-title mb-0">{t("This machine")}</h2>
              <span className="text-[0.625rem] font-mono text-faint truncate">
                {system ? `${system.os} · ${system.arch}` : ""}
              </span>
            </div>
            <div className="grid grid-cols-2 gap-2.5">
              <MachineStat
                icon={Cpu}
                label={t("CPU")}
                value={shortCpuName(system?.cpu_name ?? t("Unknown"))}
                detail={t("{cores} cores · {threads} threads", {
                  cores: system?.cpu_cores ?? 0,
                  threads: system?.cpu_threads ?? 0,
                })}
              />
              <MachineStat
                icon={MemoryStick}
                label={t("Memory")}
                value={
                  totalRam > 0 ? `${mbToGb(usedRam)} / ${mbToGb(totalRam)}` : t("Unknown")
                }
                detail={t("{gb} free", { gb: mbToGb(freeRam) })}
                fraction={totalRam > 0 ? usedRam / totalRam : undefined}
              />
              <MachineStat
                icon={Monitor}
                label={t("GPU")}
                value={
                  gpus.length > 0
                    ? `${shortGpuName(gpus[0].name)}${gpus.length > 1 ? ` +${gpus.length - 1}` : ""}`
                    : t("None")
                }
                detail={
                  gpus.length === 0
                    ? t("No GPU detected")
                    : totalVram > 0
                      ? t("{gb} VRAM", { gb: mbToGb(totalVram) })
                      : t("shared memory")
                }
              />
              <MachineStat
                icon={Zap}
                label={t("Backend")}
                value={recommendedBackend?.label ?? "CPU"}
                detail={
                  recommendedBackend?.version
                    ? `v${recommendedBackend.version}`
                    : t("Recommended for this machine")
                }
              />
            </div>
          </div>
        </div>

        <div>
          <div className="flex items-center gap-0.5">
            {dashTabs.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                onClick={() => setDashTab(id)}
                className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium transition-colors ${
                  dashTab === id
                    ? "bg-accent/20 text-ink"
                    : "text-dim hover:text-ink hover:bg-accent/10"
                }`}
              >
                <Icon size={13} />
                {label}
              </button>
            ))}
          </div>
          <p className="section-desc mt-2">{currentTab.desc}</p>

          <div className="mt-3">

        {dashTab === "runtime" && (
          <RuntimeTab
            runtime={runtime}
            managed={managed}
            custom={custom}
            activeRt={activeRt}
            appConfig={appConfig}
            release={release}
            assets={assets}
            selectedAsset={selectedAsset}
            rtBusy={rtBusy}
            rtProgress={rtProgress}
            customBuilds={customBuilds}
            scanning={scanning}
            checking={checking}
            newBuildAvailable={newBuildAvailable}
            setSelectedAsset={setSelectedAsset}
            installAsset={installAsset}
            cancelInstall={cancelInstall}
            browseCustom={browseCustom}
            registerBuild={registerBuild}
            activateManaged={activateManaged}
            activateCustom={activateCustom}
            deleteManaged={deleteManaged}
            removeCustom={removeCustom}
            deleteOldVersions={deleteOldVersions}
            checkForBuilds={checkForBuilds}
            setAutoDeleteOld={setAutoDeleteOld}
          />
        )}

        {dashTab === "models" && (
          <ModelsTab
            models={models}
            filteredModels={filteredModels}
            modelDirs={modelDirs}
            downloadDir={downloadDir}
            nameFilter={nameFilter}
            modelSortCol={modelSortCol}
            modelSortDir={modelSortDir}
            setNameFilter={setNameFilter}
            toggleModelSort={toggleModelSort}
            setConfigModel={setConfigModel}
            addModelDir={addModelDir}
            removeModelDir={removeModelDir}
            changeDownloadDir={changeDownloadDir}
            deleteModel={deleteModel}
            openInBrowser={openInBrowser}
          />
        )}

        {dashTab === "browse" && (
          <BrowseTab
            active={active}
            expandedRepo={expandedRepo}
            paused={paused}
            progress={progress}
            repoFiles={repoFiles}
            searching={searching}
            searchQuery={searchQuery}
            searchResults={searchResults}
            sortBy={sortBy}
            installedNames={installedNames}
            cancelDownload={cancelDownload}
            changeSortBy={changeSortBy}
            doSearch={() => doSearch()}
            downloadClick={downloadClick}
            openInBrowser={openInBrowser}
            pauseDownload={pauseDownload}
            resumeDownload={resumeDownload}
            setSearchQuery={setSearchQuery}
            toggleRepo={toggleRepo}
          />
        )}
          </div>
        </div>
      </div>

      {scanning && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="card flex items-center gap-3 px-6 py-4">
            <RefreshCw size={16} className="text-accent animate-spin" />
            <span className="text-sm text-ink">{t("Searching for server runtimes…")}</span>
          </div>
        </div>
      )}

      {configModel && (
        <div
          className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
          onClick={() => setConfigModel(null)}
        >
          <div
            className="card max-w-lg w-full mx-4 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-sm font-semibold text-ink">{t("Model config")}</h3>
            <pre className="text-[0.6875rem] font-mono text-dim bg-surface-0 border border-border rounded p-3 overflow-auto max-h-96">
              {JSON.stringify(
                {
                  id: configModel.id,
                  name: configModel.name,
                  filename: configModel.filename,
                  path: configModel.path,
                  architecture: configModel.architecture,
                  params_b: configModel.params_b,
                  quant: configModel.quant,
                  context_length: configModel.context_length,
                  size_bytes: configModel.size_bytes,
                  is_vision: configModel.is_vision,
                  is_reasoning: configModel.is_reasoning,
                  mmproj_path: configModel.mmproj_path,
                  hf_repo: configModel.hf_repo,
                },
                null,
                2,
              )}
            </pre>
            <button className="btn-ghost text-xs w-full" onClick={() => setConfigModel(null)}>
              {t("Close")}
            </button>
          </div>
        </div>
      )}

      {companion && comp && (
        <div
          className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
          onClick={() => setCompanion(null)}
        >
          <div className="card max-w-md w-full mx-4 space-y-3" onClick={(e) => e.stopPropagation()}>
            <h3 className="text-sm font-semibold text-ink">{t("Download options")}</h3>
            <p className="text-xs text-dim">
              {comp.split
                ? t("This model ships in {n} parts{companions}. Pick what to download.", {
                    n: companion.parts.length,
                    companions:
                      companion.mmproj.length + companion.dspark.length > 0
                        ? t(", along with companion files")
                        : t(" that are all needed to load it"),
                  })
                : t("This repo ships auxiliary files alongside the model. Pick what to download.")}
            </p>
            <div className="space-y-2">
              <div className="space-y-1.5">
                {comp.split && (
                  <p className="text-[0.625rem] uppercase tracking-wider text-dim">
                    {t("Model parts")}
                  </p>
                )}
                {comp.split && (
                  <button
                    className="w-full text-left px-3 py-2 rounded border border-accent/50 hover:bg-surface-2 text-sm text-ink"
                    onClick={() => companionChoice(true, null, null)}
                  >
                    {t("All {n} parts", { n: companion.parts.length })}
                    <span className="text-xs text-dim ml-2">{formatSize(comp.modelSize)}</span>
                  </button>
                )}
                <button
                  className="w-full text-left px-3 py-2 rounded hover:bg-surface-2 text-sm text-ink"
                  onClick={() => companionChoice(false, null, null)}
                >
                  {comp.split
                    ? t("Just part {index} of {n}", {
                        index: comp.index,
                        n: companion.parts.length,
                      })
                    : t("Just the model")}
                  <span className="text-xs text-dim ml-2">
                    {formatSize(comp.split ? (companion.file.size_bytes ?? 0) : comp.modelSize)}
                  </span>
                </button>
              </div>
              {companion.mmproj.length > 0 && (
                <div className="space-y-1.5">
                  <p className="text-[0.625rem] uppercase tracking-wider text-dim">
                    {t("Vision projection (mmproj)")}
                  </p>
                  {companion.mmproj.map((mp) => (
                    <button
                      key={mp.path}
                      className="w-full text-left px-3 py-2 rounded hover:bg-surface-2 text-sm text-ink"
                      onClick={() => companionChoice(comp.split, mp, null)}
                    >
                      {comp.label} + {baseOf(mp.path)}
                      <span className="text-xs text-dim ml-2">
                        {formatSize(comp.modelSize + (mp.size_bytes ?? 0))}
                      </span>
                    </button>
                  ))}
                </div>
              )}
              {companion.dspark.length > 0 && (
                <div className="space-y-1.5">
                  <p className="text-[0.625rem] uppercase tracking-wider text-dim">
                    {t("Speculative draft (DSpark)")}
                  </p>
                  {companion.dspark.map((dp) => (
                    <button
                      key={dp.path}
                      className="w-full text-left px-3 py-2 rounded hover:bg-surface-2 text-sm text-ink"
                      onClick={() => companionChoice(comp.split, null, dp)}
                    >
                      {comp.label} + {baseOf(dp.path)}
                      <span className="text-xs text-dim ml-2">
                        {formatSize(comp.modelSize + (dp.size_bytes ?? 0))}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </div>
            <button className="btn-ghost text-xs w-full" onClick={() => setCompanion(null)}>
              {t("Cancel")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function DetailRow({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-2 min-w-0">
      <span className="w-14 shrink-0 text-[0.625rem] uppercase tracking-wider text-faint">
        {label}
      </span>
      <span
        className={`text-xs truncate ${mono ? "font-mono text-dim" : "text-ink"}`}
        title={value}
      >
        {value}
      </span>
    </div>
  );
}

function MachineStat({
  icon: Icon,
  label,
  value,
  detail,
  fraction,
}: {
  icon: typeof Cpu;
  label: string;
  value: string;
  detail?: string;
  fraction?: number;
}) {
  return (
    <div className="rounded border border-border bg-surface-2 p-2.5 min-w-0">
      <div className="flex items-center gap-1.5 text-faint">
        <Icon size={12} className="shrink-0" />
        <span className="text-[0.625rem] uppercase tracking-wider">{label}</span>
      </div>
      <p className="mt-1 text-xs font-medium text-ink truncate" title={value}>
        {value}
      </p>
      {detail && (
        <p className="mt-0.5 text-[0.625rem] text-faint truncate" title={detail}>
          {detail}
        </p>
      )}
      {fraction != null && (
        <div className="mt-1.5 h-0.5 rounded bg-surface-3 overflow-hidden">
          <div
            className="h-full bg-accent transition-all"
            style={{ width: `${Math.min(100, Math.max(0, fraction * 100))}%` }}
          />
        </div>
      )}
    </div>
  );
}


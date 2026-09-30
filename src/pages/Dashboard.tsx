import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowDown,
  ArrowUp,
  Brain,
  Check,
  ChevronDown,
  ChevronUp,
  Cpu,
  Database,
  Download,
  ExternalLink,
  Eye,
  FileJson,
  Filter,
  FolderOpen,
  FolderPlus,
  MemoryStick,
  Monitor,
  RefreshCw,
  Search,
  Trash2,
  X,
  Zap,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  AppConfig,
  AssetDto,
  FoundBinaryDto,
  HfFileDto,
  HfModel,
  KnownOwnerDto,
  ModelDto,
  ReleaseDto,
  RuntimeInfo,
  ServerStatus,
  SystemInfoDto,
} from "../bindings";
import { call, fmtMB } from "../utils/ipc";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import {
  formatSize,
  isDsparkFile,
  isMmprojFile,
  mbToGb,
  mmprojSaveName,
  parseSplitSuffix,
  quantColor,
  quantFromName,
  quantSortKey,
  shortCpuName,
  shortGpuName,
} from "../utils/format";
import type { Tab } from "../App";

type HfSort = "downloads" | "likes" | "lastModified";
type ModelSortCol = "name" | "params" | "quant" | "size" | "ctx";
type DashTab = "runtime" | "models" | "browse";

interface Bounded<T> {
  what: string;
  value?: T;
  error?: string;
}

interface ActiveDl {
  repoId: string;
  filename: string;
  saveAs: string | null;
  split: string[] | null;
}

const baseOf = (path: string) => path.split("/").pop() ?? path;

const pathJoin = (base: string, name: string) =>
  `${base}${base.includes("\\") ? "\\" : "/"}${name}`;

const MODEL_SORTERS: Record<ModelSortCol, (a: ModelDto, b: ModelDto) => number> = {
  name: (a, b) => a.name.localeCompare(b.name),
  params: (a, b) => parseFloat(a.params_b ?? "0") - parseFloat(b.params_b ?? "0"),
  quant: (a, b) => quantSortKey(a.quant ?? null) - quantSortKey(b.quant ?? null),
  size: (a, b) => (a.size_bytes ?? 0) - (b.size_bytes ?? 0),
  ctx: (a, b) => (a.context_length ?? 0) - (b.context_length ?? 0),
};

/// Repo owner for a scanned model: the folder above the quant folder when the
/// file sits in `<owner>/<model>/<quant>/file.gguf`, else the parent folder.
function ownerOf(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  const file = parts.pop() ?? "";
  const parent = parts.pop() ?? "";
  const grandparent = parts.pop() ?? "";
  const head = (s: string) => s.split(/[-_.]/)[0]?.toLowerCase() ?? "";
  const stem = file.replace(/\.gguf$/i, "");
  return parent && grandparent && head(parent) === head(stem) ? grandparent : parent;
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
  const [owners, setOwners] = useState<KnownOwnerDto[]>([]);
  const [selectedOwner, setSelectedOwner] = useState("");
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
  const lastProgress = useRef(0);

  /// Bounds a load call so a stalled backend can't pin the page on "Loading".
  const withTimeout = <T,>(p: Promise<T>, what: string): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`${what} timed out — the backend may be busy`)),
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
      bounded(call(commands.getSystemInfo()), "System info"),
      bounded(call(commands.getRuntimeInfo()), "Runtime info"),
      bounded(call(commands.listInstalledModels()), "Model scan"),
      bounded(call(commands.getServerStatus()), "Server status"),
      bounded(call(commands.getConfig()), "Config"),
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
          `Could not load the dashboard: ${failed
            .map((f) => `${f.what}: ${f.error}`)
            .join(" · ")}`,
        );
      } else {
        setError(null);
      }
    } catch (e) {
      setError(`Could not load the dashboard: ${e}`);
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
    call(commands.getKnownOwners()).then(setOwners).catch(() => {});
    const unsubStatus = subscribeServerStatus(setStatus);
    const unlisten = listen<{ id: string; downloaded: number; total?: number | null }>(
      "download_progress",
      (e) => {
        const now = Date.now();
        if (now - lastProgress.current < 120) return;
        lastProgress.current = now;
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
    const picked = await pickFolder("Select llama.cpp directory");
    if (!picked) return;
    setScanning(true);
    setError(null);
    try {
      const builds = await call(commands.scanCustomBinaries(picked));
      if (builds.length === 0) {
        setCustomBuilds(null);
        setError("No llama-server binary found in the selected directory.");
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
    const plural = stale.length === 1 ? "" : "s";
    if (!window.confirm(`Delete ${stale.length} older build${plural}?`)) return;
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

  const modelSortIcon = (col: ModelSortCol) =>
    modelSortCol === col ? (
      modelSortDir === "asc" ? <ArrowUp size={10} /> : <ArrowDown size={10} />
    ) : (
      <ArrowUp size={10} className="opacity-0 group-hover:opacity-30" />
    );

  const addModelDir = async () => {
    const picked = await pickFolder("Add GGUF storage directory");
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
    const picked = await pickFolder("Select default download directory");
    if (!picked) return;
    await run(async () => {
      await call(commands.setDownloadDir(picked));
      await reloadDirs();
    }, false);
  };

  // ── HuggingFace browser ───────────────────────────────────────────

  const doSearch = async (sort: HfSort = sortBy) => {
    if (!searchQuery.trim() && !selectedOwner) return;
    setSearching(true);
    setError(null);
    try {
      setSearchResults(
        await call(commands.searchHfModels(searchQuery.trim(), selectedOwner || null, sort)),
      );
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
        Loading system info…
      </div>
    );
  }

  const totalVram = system?.gpus.reduce((s, g) => s + g.vram_mb, 0) ?? 0;
  const totalRam = system?.available_ram_mb ?? 0;
  const installedNames = new Set(models.map((m) => m.filename));

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
          label: split ? `All ${companion.parts.length} parts` : "Model",
        };
      })()
    : null;

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4">
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
                Retry
              </button>
              <button
                className="text-xs text-accent-red/70 hover:text-accent-red"
                onClick={() => setError(null)}
              >
                Dismiss
              </button>
            </div>
          </div>
        )}

        {/* Machine cards. */}
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div className="card flex items-center gap-2">
            <Cpu size={16} className="text-accent-soft shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-dim">CPU</p>
              <p className="text-xs font-medium text-ink truncate">
                {shortCpuName(system?.cpu_name ?? "Unknown")}
              </p>
              <p className="text-[0.625rem] text-faint">
                {system?.cpu_cores ? `${system.cpu_cores}c / ${system.cpu_threads}t` : "—"}
              </p>
            </div>
          </div>
          <div className="card flex items-center gap-2">
            <MemoryStick size={16} className="text-accent-green shrink-0" />
            <div>
              <p className="text-xs text-dim">RAM</p>
              <p className="text-xs font-medium text-ink">{mbToGb(system?.total_ram_mb ?? 0)} total</p>
              <p className="text-[0.625rem] text-faint">{mbToGb(system?.available_ram_mb ?? 0)} free</p>
            </div>
          </div>
          <div className="card flex items-center gap-2">
            <Monitor size={16} className="text-accent-yellow shrink-0" />
            <div className="min-w-0">
              <p className="text-xs text-dim">GPU</p>
              {system && system.gpus.length > 0 ? (
                <>
                  <p className="text-xs font-medium text-ink truncate">
                    {shortGpuName(system.gpus[0].name)}
                    {system.gpus.length > 1 ? ` +${system.gpus.length - 1}` : ""}
                  </p>
                  <p className="text-[0.625rem] text-faint">
                    {totalVram > 0 ? `${mbToGb(totalVram)} VRAM` : "shared memory"}
                  </p>
                </>
              ) : (
                <p className="text-xs text-dim">None</p>
              )}
            </div>
          </div>
          <div className="card flex items-center gap-2">
            <Zap size={16} className="text-accent-soft shrink-0" />
            <div>
              <p className="text-xs text-dim">Backend</p>
              <p className="text-xs font-medium text-ink uppercase">
                {system?.recommended_backend ?? "CPU"}
              </p>
              <p className="text-[0.625rem] text-faint">
                {totalVram > 0 ? `${mbToGb(totalVram)} VRAM + ` : ""}
                {mbToGb(totalRam)} RAM
              </p>
            </div>
          </div>
        </div>

        {/* Server status. */}

          <div className="card">
            <div className="flex items-center gap-3">
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
              <div className="flex-1 min-w-0">
                {status.type === "running" && status.ready ? (
                  <>
                    <p className="text-sm font-medium text-accent-green">
                      Running <span className="font-mono">:{status.port}</span>
                    </p>
                    <p className="text-xs text-dim truncate">
                      {(appConfig?.selected_model ?? "").split(/[\\/]/).pop() || "server"}
                    </p>
                  </>
                ) : status.type === "running" || status.type === "starting" ? (
                  <p className="text-sm font-medium text-accent-yellow">
                    Loading the model — not ready yet…
                  </p>
                ) : status.type === "error" ? (
                  <p className="text-sm font-medium text-accent-red truncate">{status.message}</p>
                ) : (
                  <p className="text-sm font-medium text-ink">Server stopped</p>
                )}
              </div>
              {running ? (
                <div className="flex items-center gap-2 shrink-0">
                  {status.type === "running" && (
                    <button className="btn-secondary text-xs" onClick={() => go("chat")}>
                      Chat
                    </button>
                  )}
                  <button className="btn-danger text-xs" onClick={stop} disabled={stopping}>
                    Stop
                  </button>
                </div>
              ) : (
                <button className="btn-ghost text-xs shrink-0" onClick={() => go("run")}>
                  Configure launch
                </button>
              )}
            </div>
          </div>

        <div>
          <div className="flex items-end gap-0.5 border-b border-border">
            {(
              [
                { id: "runtime", label: `Runtime (${managed.length + custom.length})`, icon: Cpu },
                { id: "models", label: `Models (${models.length})`, icon: Database },
                { id: "browse", label: "Browse HuggingFace", icon: Search },
              ] as { id: DashTab; label: string; icon: typeof Cpu }[]
            ).map(({ id, label, icon: Icon }) => {
              const active = dashTab === id;
              return (
                <button
                  key={id}
                  onClick={() => setDashTab(id)}
                  className={`relative -mb-px flex items-center gap-1.5 px-4 py-1.5 text-sm font-medium rounded-t border transition-colors ${
                    active
                      ? "border-border bg-surface-1 text-ink"
                      : "border-transparent text-dim hover:text-ink hover:bg-accent/10"
                  }`}
                >
                  {active && (
                    <span className="absolute inset-0 rounded-t bg-accent/20 pointer-events-none" />
                  )}
                  <Icon size={14} className="relative" />
                  <span className="relative">{label}</span>
                </button>
              );
            })}
          </div>

        {dashTab === "runtime" && (
          <div className="card rounded-t-none border-t-0">
            <div className="flex items-center justify-between mb-1">
              <p className="section-desc">llama.cpp builds used to serve models.</p>
              <div className="flex items-center gap-3">
                {(managed.length > 0 || custom.length > 0) && (
                  <label
                    className="flex items-center gap-1.5 text-[0.6875rem] text-dim cursor-pointer"
                    title="Delete older builds of the same backend after installing a new one"
                  >
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={appConfig?.auto_delete_old_runtimes ?? false}
                      onChange={(e) => setAutoDeleteOld(e.target.checked)}
                    />
                    Auto-delete old
                  </label>
                )}
                {managed.length > 1 && (
                  <button
                    className="btn-ghost text-[0.6875rem]"
                    title="Delete every managed build except the active one"
                    onClick={deleteOldVersions}
                  >
                    <Trash2 size={11} /> Old versions
                  </button>
                )}
                <button
                  className="btn-ghost text-[0.6875rem]"
                  onClick={browseCustom}
                  title="Register a local llama.cpp build"
                >
                  <FolderOpen size={11} /> Browse…
                </button>
              </div>
            </div>
            <div className="mt-2 flex items-center gap-2">
              <button
                className="btn-secondary text-xs"
                onClick={checkForBuilds}
                disabled={checking}
              >
                {checking ? (
                  <><RefreshCw size={12} className="animate-spin" /> Checking…</>
                ) : (
                  <><RefreshCw size={12} /> Check for builds</>
                )}
              </button>
              {newBuildAvailable && (
                <span className="inline-flex items-center rounded px-1.5 py-px text-[0.625rem] font-medium bg-accent/20 text-accent">
                  New version
                </span>
              )}
            </div>

            {/* Installed builds sit above the download list so it never buries them. */}
            {(managed.length > 0 || custom.length > 0) && (
              <div className="mt-3 pt-3 border-t border-border space-y-1">
                {managed.map((r) => {
                  const isActive =
                    activeRt?.type === "managed" &&
                    activeRt.build === r.build &&
                    (activeRt.backend_id ?? "") === r.backend_id;
                  const dirPath = runtime?.base_dir ? pathJoin(runtime.base_dir, r.dir_name) : r.dir_name;
                  return (
                    <div key={`${r.build}-${r.backend_id}`} className="flex items-center gap-2 text-xs">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-1.5">
                          {isActive && <Check size={12} className="text-accent-green shrink-0" />}
                          <span className={`font-mono ${isActive ? "text-ink" : "text-dim"}`}>b{r.build}</span>
                          <span className="text-faint uppercase">{r.backend_label}</span>
                        </div>
                        <p className="text-[0.625rem] font-mono text-faint truncate" title={dirPath}>
                          {dirPath}
                        </p>
                      </div>
                      {isActive && <span className="badge-green text-[0.625rem] shrink-0">active</span>}
                      {!isActive && (
                        <>
                          <button className="btn-ghost text-[0.6875rem]" onClick={() => activateManaged(r.build, r.backend_id)}>
                            Use
                          </button>
                          <button
                            className="text-faint hover:text-accent-red"
                            title="Delete build"
                            onClick={() => deleteManaged(r.build, r.backend_id)}
                          >
                            <Trash2 size={11} />
                          </button>
                        </>
                      )}
                    </div>
                  );
                })}
                {custom.map((c, i) => {
                  const isActive = activeRt?.type === "custom" && activeRt.index === i;
                  return (
                    <div key={c.binary_path} className="flex items-center gap-2 text-xs">
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-1.5">
                          {isActive && <Check size={12} className="text-accent-green shrink-0" />}
                          <span className={`truncate ${isActive ? "text-ink" : "text-dim"}`}>{c.label}</span>
                        </div>
                        <p className="text-[0.625rem] font-mono text-faint truncate" title={c.binary_path}>
                          {c.binary_path}
                        </p>
                      </div>
                      {isActive && <span className="badge-green text-[0.625rem] shrink-0">active</span>}
                      {!isActive && (
                        <>
                          <button className="btn-ghost text-[0.6875rem]" onClick={() => activateCustom(i)}>
                            Use
                          </button>
                          <button
                            className="text-faint hover:text-accent-red"
                            title="Remove runtime"
                            onClick={() => removeCustom(i)}
                          >
                            <Trash2 size={11} />
                          </button>
                        </>
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            {release && (
              <div className="mt-3 pt-3 border-t border-border">
                <p className="text-[0.6875rem] text-dim mb-1.5">
                  Checked {release.tag_name} — pick a build to download.
                </p>
                <div className="space-y-1">
                  {assets.map((asset) => (
                    <AssetRow
                      key={asset.name}
                      asset={asset}
                      selected={selectedAsset === asset.name}
                      onSelect={() => setSelectedAsset(asset.name)}
                    />
                  ))}
                </div>
                <div className="flex items-center gap-2 mt-2">
                  <button
                    className="btn-primary text-xs"
                    disabled={!selectedAsset || rtBusy}
                    onClick={() => selectedAsset && installAsset(selectedAsset)}
                  >
                    <Download size={12} /> {rtBusy ? "Downloading…" : "Download"}
                  </button>
                  {rtProgress && (
                    <button className="btn-ghost text-xs" onClick={cancelInstall}>
                      Cancel
                    </button>
                  )}
                </div>
                {rtProgress && (
                  <div className="mt-2">
                    <div className="flex items-center justify-between text-[0.6875rem] text-dim mb-1">
                      <span>Downloading…</span>
                      <span className="font-mono">
                        {rtProgress.total
                          ? `${((rtProgress.downloaded / rtProgress.total) * 100).toFixed(1)}% — ${fmtMB(rtProgress.downloaded / 1024 / 1024)} / ${fmtMB(rtProgress.total / 1024 / 1024)}`
                          : fmtMB(rtProgress.downloaded / 1024 / 1024)}
                      </span>
                    </div>
                    <div className="h-1.5 bg-surface-3 rounded overflow-hidden">
                      <div
                        className="h-full bg-accent transition-all"
                        style={{
                          width: rtProgress.total
                            ? `${(rtProgress.downloaded / rtProgress.total) * 100}%`
                            : "100%",
                        }}
                      />
                    </div>
                  </div>
                )}
              </div>
            )}

            {customBuilds && customBuilds.length > 1 && (
              <div className="mt-3 pt-3 border-t border-border">
                <p className="label mb-1.5">Multiple builds found — pick one to register</p>
                <div className="space-y-1">
                  {customBuilds.map((b) => (
                    <button
                      key={b.binary_path}
                      className="w-full text-left px-2.5 py-1.5 border rounded border-border hover:bg-surface-2 transition-colors"
                      onClick={() => registerBuild(b.binary_path)}
                    >
                      <span className="text-xs font-mono text-ink truncate block">{b.label}</span>
                      <span className="text-[0.625rem] font-mono text-faint truncate block">{b.binary_path}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
            {scanning && (
              <p className="text-xs text-dim mt-2 flex items-center gap-2">
                <RefreshCw size={12} className="animate-spin" /> Searching for server runtimes…
              </p>
            )}
          </div>
        )}

        {dashTab === "models" && (
          <div className="card rounded-t-none border-t-0">
            <div className="flex items-center justify-between gap-3 mb-2">
              <div className="flex items-baseline gap-2 shrink-0">
                <p className="section-desc">List of models available to launch.</p>
              </div>
              <div className="flex items-center gap-2 min-w-0">
                <span className="text-[0.6875rem] font-mono text-dim truncate max-w-72" title={downloadDir}>
                  {downloadDir || "Not set"}
                </span>
                <button className="btn-ghost text-[0.6875rem] py-1 shrink-0" onClick={changeDownloadDir}>
                  <FolderOpen size={11} /> Change…
                </button>
                <button
                  className="btn-ghost text-[0.6875rem] py-1 shrink-0"
                  onClick={addModelDir}
                  title="Scan another folder too"
                >
                  <FolderPlus size={11} /> Add
                </button>
              </div>
            </div>
            {modelDirs.filter((d) => d !== downloadDir).length > 0 && (
              <div className="flex flex-col items-end gap-0.5 mb-2">
                {modelDirs
                  .filter((d) => d !== downloadDir)
                  .map((d) => (
                    <div key={d} className="flex items-center gap-1.5 text-[0.6875rem]">
                      <span className="font-mono text-faint truncate max-w-96" title={d}>{d}</span>
                      {modelDirs.length > 1 && (
                        <button
                          className="text-faint hover:text-accent-red shrink-0"
                          title="Stop scanning this folder"
                          onClick={() => removeModelDir(d)}
                        >
                          <X size={10} />
                        </button>
                      )}
                    </div>
                  ))}
              </div>
            )}
            <div className="flex items-center gap-3 mb-2">
              <div className="flex items-center gap-1.5 flex-1 min-w-0">
                <Filter size={12} className="text-faint shrink-0" />
                <input
                  className="input flex-1 py-1 px-2 text-xs min-w-0"
                  placeholder="Filter by name…"
                  value={nameFilter}
                  onChange={(e) => setNameFilter(e.target.value)}
                />
              </div>
              <span className="text-[0.6875rem] text-faint shrink-0">
                {filteredModels.length} model{filteredModels.length === 1 ? "" : "s"}
              </span>
            </div>
            {models.length === 0 ? (
              <div className="text-center py-6">
                <p className="text-xs text-dim">Nothing downloaded yet.</p>
              </div>
            ) : (
              <div>
                <div className="flex items-center gap-2 px-3 py-2 border-b border-border text-[0.625rem] font-semibold text-dim uppercase tracking-wider select-none">
                  <button
                    className="flex-1 flex items-center gap-1 group text-left"
                    onClick={() => toggleModelSort("name")}
                  >
                    Model {modelSortIcon("name")}
                  </button>
                  <button
                    className="w-16 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("params")}
                  >
                    Params {modelSortIcon("params")}
                  </button>
                  <button
                    className="w-20 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("quant")}
                  >
                    Quant {modelSortIcon("quant")}
                  </button>
                  <button
                    className="w-16 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("ctx")}
                  >
                    Ctx {modelSortIcon("ctx")}
                  </button>
                  <button
                    className="w-20 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("size")}
                  >
                    Size {modelSortIcon("size")}
                  </button>
                  <span className="w-14 text-center normal-case">Config</span>
                  <span className="w-8" />
                  <span className="w-8" />
                </div>
                {filteredModels.map((m) => {
                  const owner = ownerOf(m.path);
                  return (
                    <div
                      key={m.id}
                      className="flex items-center gap-2 px-3 py-2 border-b border-border hover:bg-surface-3 transition-colors"
                    >
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-1.5 min-w-0">
                          <p className="text-sm text-ink truncate">{m.name}</p>
                          {m.is_vision && (
                            <span title="Vision model">
                              <Eye size={12} className="text-[#3B82F6] shrink-0" />
                            </span>
                          )}
                          {m.is_reasoning && (
                            <span title="Reasoning model">
                              <Brain size={12} className="text-[#E5484D] shrink-0" />
                            </span>
                          )}
                        </div>
                        <p className="text-[0.625rem] text-faint truncate font-mono">
                          {owner ? owner + "/" : ""}{m.filename}
                        </p>
                      </div>
                      <span className="w-16 text-right text-xs text-dim">{m.params_b ?? "—"}</span>
                      <span className="w-20 text-right">
                        {m.quant ? (
                          <span className={`${quantColor(m.quant)} text-[0.625rem]`}>{m.quant}</span>
                        ) : (
                          <span className="text-xs text-faint">—</span>
                        )}
                      </span>
                      <span className="w-16 text-right text-xs text-faint">
                        {m.context_length ? `${(m.context_length / 1024).toFixed(0)}K` : "—"}
                      </span>
                      <span className="w-20 text-right text-xs text-dim font-mono">
                        {formatSize(m.size_bytes ?? 0)}
                      </span>
                      <span className="w-14 flex justify-center items-center">
                        <button
                          className="text-faint hover:text-ink"
                          title="Model config (JSON)"
                          onClick={() => setConfigModel(m)}
                        >
                          <FileJson size={12} />
                        </button>
                      </span>
                      <span className="w-8 flex justify-center items-center">
                        {m.hf_repo && (
                          <button
                            className="text-faint hover:text-ink"
                            title="Open on HuggingFace"
                            onClick={() => openInBrowser(`https://huggingface.co/${m.hf_repo}`)}
                          >
                            <ExternalLink size={11} />
                          </button>
                        )}
                      </span>
                      <button
                        className="w-8 flex justify-center text-faint hover:text-accent-red"
                        title="Delete model"
                        onClick={() => deleteModel(m.path)}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  );
                })}
                {filteredModels.length === 0 && (
                  <p className="text-sm text-dim py-6 text-center">No models match the filter.</p>
                )}
              </div>
            )}
          </div>
        )}

        {dashTab === "browse" && (
          <div className="card rounded-t-none border-t-0">
            <div className="flex items-baseline justify-between mb-2">
              <p className="section-desc">Search and download models from HuggingFace.</p>
            </div>
            <div className="flex gap-2 mb-2">
              <div className="flex-1 relative min-w-0">
                <Search
                  size={14}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-faint pointer-events-none"
                />
                <input
                  className="input pl-9 w-full text-xs"
                  placeholder="Search models (e.g. llama, mistral, qwen)"
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") doSearch();
                  }}
                />
              </div>
              <select
                className="input py-1 px-2 text-xs shrink-0"
                value={sortBy}
                onChange={(e) => changeSortBy(e.target.value as HfSort)}
              >
                <option value="downloads">By downloads</option>
                <option value="likes">By stars</option>
                <option value="lastModified">Newest</option>
              </select>
              <select
                className="input py-1 px-2 text-xs shrink-0 max-w-40"
                value={selectedOwner}
                onChange={(e) => setSelectedOwner(e.target.value)}
              >
                <option value="">Any owner</option>
                {owners.map((o) => (
                  <option key={o.id} value={o.id} title={o.description}>
                    {o.id}
                  </option>
                ))}
              </select>
              <button className="btn-primary text-xs shrink-0" onClick={() => doSearch()} disabled={searching}>
                {searching ? "…" : "Search"}
              </button>
            </div>

            <ActiveDownloads
              active={active}
              progress={progress}
              paused={paused}
              onPause={pauseDownload}
              onResume={resumeDownload}
              onCancel={cancelDownload}
            />

            {searchResults.length > 0 ? (
              <div className="space-y-2">
                {searchResults.map((model) => {
                  const files = repoFiles[model.repo_id];
                  return (
                    <div key={model.repo_id} className="rounded border border-border bg-surface-2 p-4">
                      <div
                        role="button"
                        tabIndex={0}
                        className="w-full flex items-start gap-3 text-left min-w-0 cursor-pointer"
                        onClick={() => toggleRepo(model.repo_id)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.preventDefault();
                            toggleRepo(model.repo_id);
                          }
                        }}
                      >
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="text-sm font-medium text-ink truncate">{model.name}</span>
                            <button
                              className="text-faint hover:text-dim shrink-0"
                              onClick={(e) => {
                                e.stopPropagation();
                                openInBrowser(`https://huggingface.co/${model.repo_id}`);
                              }}
                              title="Open on HuggingFace"
                            >
                              <ExternalLink size={12} />
                            </button>
                            <span className="text-xs text-dim shrink-0">by {model.author}</span>
                          </div>
                          <div className="flex gap-3 mt-1 text-xs text-dim">
                            <span>↓ {(model.downloads / 1000).toFixed(0)}K</span>
                            <span>♥ {model.likes}</span>
                          </div>
                        </div>
                        {expandedRepo === model.repo_id ? (
                          <ChevronUp size={14} className="text-dim mt-0.5 shrink-0" />
                        ) : (
                          <ChevronDown size={14} className="text-dim mt-0.5 shrink-0" />
                        )}
                      </div>

                      {expandedRepo === model.repo_id && (
                        <div className="mt-3 pt-3 border-t border-border space-y-1">
                          {files ? (
                            files.filter((f) => !isMmprojFile(f.path) && !isDsparkFile(f.path)).length > 0 ? (
                              files
                                .filter((f) => !isMmprojFile(f.path) && !isDsparkFile(f.path))
                                .map((f) => {
                                  const id = f.path;
                                  const done = installedNames.has(baseOf(f.path));
                                  const dl = active[id];
                                  const prog = progress[id];
                                  const quant = quantFromName(f.path);
                                  const split = parseSplitSuffix(baseOf(f.path));
                                  return (
                                    <div key={f.path} className="px-2 py-2 hover:bg-surface-2 rounded">
                                      <div className="flex items-center gap-3">
                                        <div className="flex-1 min-w-0">
                                          <span className="text-xs text-ink font-mono truncate block">
                                            {f.path}
                                          </span>
                                          <div className="flex items-center gap-1.5 mt-0.5">
                                            {quant && (
                                              <span className={`${quantColor(quant)} text-[0.625rem]`}>
                                                {quant}
                                              </span>
                                            )}
                                            {split && (
                                              <span className="badge-gray text-[0.625rem]">
                                                {split.total} parts
                                              </span>
                                            )}
                                          </div>
                                        </div>
                                        <span className="text-xs text-dim shrink-0">
                                          {formatSize(f.size_bytes ?? 0)}
                                        </span>
                                        {done ? (
                                          <span className="badge-green text-[0.625rem] shrink-0">
                                            Installed
                                          </span>
                                        ) : dl ? (
                                          <span className="text-[0.625rem] text-dim shrink-0">
                                            {prog?.total
                                              ? `${((prog.downloaded / prog.total) * 100).toFixed(0)}%`
                                              : fmtMB((prog?.downloaded ?? 0) / 1024 / 1024)}
                                          </span>
                                        ) : (
                                          <button
                                            className="btn-secondary text-xs shrink-0"
                                            onClick={() => downloadClick(model.repo_id, f)}
                                          >
                                            <Download size={11} />
                                          </button>
                                        )}
                                      </div>
                                      {dl && (
                                        <DownloadRow
                                          prog={prog}
                                          paused={!!paused[id]}
                                          onPause={() => pauseDownload(id)}
                                          onResume={() => resumeDownload(id)}
                                          onCancel={() => cancelDownload(id)}
                                        />
                                      )}
                                    </div>
                                  );
                                })
                            ) : (
                              <p className="text-xs text-dim px-2">No GGUF files in this repo.</p>
                            )
                          ) : (
                            <p className="text-xs text-dim px-2">Loading files…</p>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            ) : (
              !searching && (
                <div className="text-center py-10 text-dim">
                  <Search size={26} className="mx-auto mb-3 opacity-30" />
                  <p className="text-sm">Search for GGUF models on HuggingFace.</p>
                  <p className="text-xs mt-1 text-faint">
                    Try "llama 3", "mistral", or pick an owner to browse.
                  </p>
                </div>
              )
            )}
          </div>
        )}
        </div>
      </div>

      {scanning && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="card flex items-center gap-3 px-6 py-4">
            <RefreshCw size={16} className="text-accent animate-spin" />
            <span className="text-sm text-ink">Searching for server runtimes…</span>
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
            <h3 className="text-sm font-semibold text-ink">Model config</h3>
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
              Close
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
            <h3 className="text-sm font-semibold text-ink">Download options</h3>
            <p className="text-xs text-dim">
              {comp.split
                ? `This model ships in ${companion.parts.length} parts${
                    companion.mmproj.length + companion.dspark.length > 0
                      ? ", along with companion files."
                      : " that are all needed to load it."
                  } Pick what to download.`
                : "This repo ships auxiliary files alongside the model. Pick what to download."}
            </p>
            <div className="space-y-2">
              <div className="space-y-1.5">
                {comp.split && (
                  <p className="text-[0.625rem] uppercase tracking-wider text-dim">Model parts</p>
                )}
                {comp.split && (
                  <button
                    className="w-full text-left px-3 py-2 rounded border border-accent/50 hover:bg-surface-2 text-sm text-ink"
                    onClick={() => companionChoice(true, null, null)}
                  >
                    All {companion.parts.length} parts
                    <span className="text-xs text-dim ml-2">{formatSize(comp.modelSize)}</span>
                  </button>
                )}
                <button
                  className="w-full text-left px-3 py-2 rounded hover:bg-surface-2 text-sm text-ink"
                  onClick={() => companionChoice(false, null, null)}
                >
                  {comp.split
                    ? `Just part ${comp.index} of ${companion.parts.length}`
                    : "Just the model"}
                  <span className="text-xs text-dim ml-2">
                    {formatSize(comp.split ? (companion.file.size_bytes ?? 0) : comp.modelSize)}
                  </span>
                </button>
              </div>
              {companion.mmproj.length > 0 && (
                <div className="space-y-1.5">
                  <p className="text-[0.625rem] uppercase tracking-wider text-dim">
                    Vision projection (mmproj)
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
                    Speculative draft (DSpark)
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
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function AssetRow({
  asset,
  selected,
  onSelect,
}: {
  asset: AssetDto;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      className={`w-full flex items-center gap-3 px-3 py-2 border rounded text-left transition-colors ${
        selected ? "border-accent/60 bg-accent/10" : "border-border hover:bg-surface-2"
      }`}
      onClick={onSelect}
    >
      <div
        className={`w-3 h-3 rounded-full border-2 shrink-0 ${
          selected ? "border-accent bg-accent" : "border-faint"
        }`}
      />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-ink truncate">{asset.name}</span>
          {asset.score >= 90 && (
            <span className="badge-green text-[0.625rem] shrink-0">Recommended</span>
          )}
        </div>
        <div className="flex gap-3 mt-0.5">
          <span className="text-xs text-dim">{asset.backend_label}</span>
          <span className="text-xs text-faint">{fmtMB(asset.size_mb ?? 0)}</span>
        </div>
      </div>
    </button>
  );
}

function DownloadRow({ prog, paused, onPause, onResume, onCancel }: {
  prog?: { downloaded: number; total: number | null };
  paused: boolean;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="mt-2">
      <div className="flex justify-between items-center text-xs text-dim mb-1">
        <span>{paused ? "Paused" : "Downloading…"}</span>
        <span>
          {prog?.total ? `${((prog.downloaded / prog.total) * 100).toFixed(1)}%` : fmtMB((prog?.downloaded ?? 0) / 1024 / 1024)}
        </span>
      </div>
      <div className="h-1.5 bg-surface-3 overflow-hidden">
        <div
          className="h-full bg-accent transition-all"
          style={{ width: prog?.total ? `${(prog.downloaded / prog.total) * 100}%` : "100%" }}
        />
      </div>
      <div className="flex gap-2 mt-2">
        {paused ? (
          <>
            <button className="btn-primary text-xs" onClick={onResume}>
              Resume
            </button>
            <button className="btn-danger text-xs" onClick={onCancel}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button className="btn-ghost text-xs" onClick={onPause}>
              Pause
            </button>
            <button className="btn-danger text-xs" onClick={onCancel}>
              Cancel
            </button>
          </>
        )}
      </div>
    </div>
  );
}

function ActiveDownloads({ active, progress, paused, onPause, onResume, onCancel }: {
  active: Record<string, ActiveDl>;
  progress: Record<string, { downloaded: number; total: number | null }>;
  paused: Record<string, boolean>;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onCancel: (id: string) => void;
}) {
  const ids = Object.keys(active);
  if (ids.length === 0) return null;
  return (
    <div className="mb-2 rounded border border-border bg-surface-2 p-4">
      <h3 className="text-xs font-medium text-dim mb-2">Downloading</h3>
      <div className="space-y-3">
        {ids.map((id) => {
          const dl = progress[id];
          const isPaused = !!paused[id];
          return (
            <div key={id}>
              <div className="flex items-center justify-between mb-1 gap-2">
                <span className="text-xs text-ink truncate mr-3 font-mono">{baseOf(id)}</span>
                <div className="flex items-center gap-2 shrink-0">
                  {isPaused ? (
                    <>
                      <span className="text-[0.625rem] text-accent-yellow">Paused</span>
                      <button className="btn-primary text-[0.625rem] py-0.5 px-1.5" onClick={() => onResume(id)}>
                        Resume
                      </button>
                      <button
                        className="btn-danger text-[0.625rem] py-0.5 px-1.5"
                        onClick={() => onCancel(id)}
                        title="Discard the partial download"
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <>
                      <span className="text-[0.625rem] font-mono text-dim">
                        {dl?.total ? `${((dl.downloaded / dl.total) * 100).toFixed(1)}%` : fmtMB((dl?.downloaded ?? 0) / 1024 / 1024)}
                      </span>
                      <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={() => onPause(id)}>
                        Pause
                      </button>
                      <button
                        className="btn-danger text-[0.625rem] py-0.5 px-1.5"
                        onClick={() => onCancel(id)}
                        title="Discard the partial download"
                      >
                        Cancel
                      </button>
                    </>
                  )}
                </div>
              </div>
              {!isPaused && (
                <div className="h-1 bg-surface-3 overflow-hidden">
                  <div
                    className="h-full bg-accent transition-all"
                    style={{ width: dl?.total ? `${(dl.downloaded / dl.total) * 100}%` : "100%" }}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

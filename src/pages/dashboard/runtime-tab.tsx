import { Check, Download, FolderOpen, RefreshCw, Trash2 } from "lucide-react";
import type {
  ActiveRuntime,
  AppConfig,
  AssetDto,
  CustomRuntimeDto,
  FoundBinaryDto,
  ManagedRuntime,
  ReleaseDto,
  RuntimeInfo,
} from "../../bindings";
import { fmtMB } from "../../utils/ipc";
import { t } from "../../utils/i18n";

const pathJoin = (base: string, name: string) =>
  `${base}${base.includes("\\") ? "\\" : "/"}${name}`;

type RuntimeTabProps = {
  runtime: RuntimeInfo | null;
  managed: ManagedRuntime[];
  custom: CustomRuntimeDto[];
  activeRt?: ActiveRuntime;
  appConfig: AppConfig | null;
  release: ReleaseDto | null;
  assets: AssetDto[];
  selectedAsset: string | null;
  rtBusy: boolean;
  rtProgress: { downloaded: number; total: number | null } | null;
  customBuilds: FoundBinaryDto[] | null;
  scanning: boolean;
  checking: boolean;
  newBuildAvailable: boolean;
  setSelectedAsset: (name: string) => void;
  installAsset: (name: string) => void;
  cancelInstall: () => void;
  browseCustom: () => void;
  registerBuild: (path: string) => void;
  activateManaged: (build: number, backendId: string) => void;
  activateCustom: (index: number) => void;
  deleteManaged: (build: number, backendId: string) => void;
  removeCustom: (index: number) => void;
  deleteOldVersions: () => void;
  checkForBuilds: () => void;
  setAutoDeleteOld: (v: boolean) => void;
};

export function RuntimeTab(props: RuntimeTabProps) {
  const {
    runtime,
    managed,
    custom,
    activeRt,
    appConfig,
    release,
    assets,
    selectedAsset,
    rtBusy,
    rtProgress,
    customBuilds,
    scanning,
    checking,
    newBuildAvailable,
    setSelectedAsset,
    installAsset,
    cancelInstall,
    browseCustom,
    registerBuild,
    activateManaged,
    activateCustom,
    deleteManaged,
    removeCustom,
    deleteOldVersions,
    checkForBuilds,
    setAutoDeleteOld,
  } = props;
  return (          <div className="card">
            <div className="flex items-center justify-end gap-3">
              {(managed.length > 0 || custom.length > 0) && (
                  <label
                    className="flex items-center gap-1.5 text-[0.6875rem] text-dim cursor-pointer"
                    title={t("Delete older builds of the same backend after installing a new one")}
                  >
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={appConfig?.auto_delete_old_runtimes ?? false}
                      onChange={(e) => setAutoDeleteOld(e.target.checked)}
                    />
                    {t("Auto-delete old")}
                  </label>
                )}
                {managed.length > 1 && (
                  <button
                    className="btn-ghost text-[0.6875rem]"
                    title={t("Delete every managed build except the active one")}
                    onClick={deleteOldVersions}
                  >
                    <Trash2 size={11} /> {t("Old versions")}
                  </button>
                )}
                <button
                  className="btn-ghost text-[0.6875rem]"
                  onClick={browseCustom}
                  title={t("Register a local llama.cpp build")}
                >
                  <FolderOpen size={11} /> {t("Browse…")}
                </button>
            </div>
            <div className="mt-2 flex items-center gap-2">
              <button
                className="btn-secondary text-xs"
                onClick={checkForBuilds}
                disabled={checking}
              >
                {checking ? (
                  <><RefreshCw size={12} className="animate-spin" /> {t("Checking…")}</>
                ) : (
                  <><RefreshCw size={12} /> {t("Check for builds")}</>
                )}
              </button>
              {newBuildAvailable && (
                <span className="inline-flex items-center rounded px-1.5 py-px text-[0.625rem] font-medium bg-accent/20 text-accent">
                  {t("New version")}
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
                      {isActive && <span className="badge-green text-[0.625rem] shrink-0">{t("active")}</span>}
                      {!isActive && (
                        <>
                          <button className="btn-ghost text-[0.6875rem]" onClick={() => activateManaged(r.build, r.backend_id)}>
                            {t("Use")}
                          </button>
                          <button
                            className="text-faint hover:text-accent-red"
                            title={t("Delete build")}
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
                      {isActive && <span className="badge-green text-[0.625rem] shrink-0">{t("active")}</span>}
                      {!isActive && (
                        <>
                          <button className="btn-ghost text-[0.6875rem]" onClick={() => activateCustom(i)}>
                            {t("Use")}
                          </button>
                          <button
                            className="text-faint hover:text-accent-red"
                            title={t("Remove runtime")}
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
                  {t("Checked {tag} — pick a build to download.", { tag: release.tag_name })}
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
                    <Download size={12} /> {rtBusy ? t("Downloading…") : t("Download")}
                  </button>
                  {rtProgress && (
                    <button className="btn-ghost text-xs" onClick={cancelInstall}>
                      {t("Cancel")}
                    </button>
                  )}
                </div>
                {rtProgress && (
                  <div className="mt-2">
                    <div className="flex items-center justify-between text-[0.6875rem] text-dim mb-1">
                      <span>{t("Downloading…")}</span>
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
                <p className="label mb-1.5">{t("Multiple builds found — pick one to register")}</p>
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
                <RefreshCw size={12} className="animate-spin" /> {t("Searching for server runtimes…")}
              </p>
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
            <span className="badge-green text-[0.625rem] shrink-0">{t("Recommended")}</span>
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

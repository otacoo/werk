import { ChevronDown, ChevronUp, Download, ExternalLink, Search } from "lucide-react";
import type { HfFileDto, HfModel } from "../../bindings";
import { fmtMB } from "../../utils/ipc";
import { t } from "../../utils/i18n";
import {
  formatSize,
  isDsparkFile,
  isMmprojFile,
  parseSplitSuffix,
  quantColor,
  quantFromName,
} from "../../utils/format";
import { ActiveDownloads, DownloadRow } from "./downloads";
import { baseOf, type ActiveDl, type HfSort } from "./shared";

type BrowseTabProps = {
  active: Record<string, ActiveDl>;
  expandedRepo: string | null;
  paused: Record<string, boolean>;
  progress: Record<string, { downloaded: number; total: number | null }>;
  repoFiles: Record<string, HfFileDto[]>;
  searching: boolean;
  searchQuery: string;
  searchResults: HfModel[];
  sortBy: HfSort;
  installedNames: Set<string>;
  cancelDownload: (id: string) => void;
  changeSortBy: (sort: HfSort) => void;
  doSearch: () => void;
  downloadClick: (repoId: string, file: HfFileDto) => void;
  openInBrowser: (url: string) => void;
  pauseDownload: (id: string) => void;
  resumeDownload: (id: string) => void;
  setSearchQuery: (v: string) => void;
  toggleRepo: (repoId: string) => void;
};

export function BrowseTab(props: BrowseTabProps) {
  const {
    active,
    expandedRepo,
    paused,
    progress,
    repoFiles,
    searching,
    searchQuery,
    searchResults,
    sortBy,
    installedNames,
    cancelDownload,
    changeSortBy,
    doSearch,
    downloadClick,
    openInBrowser,
    pauseDownload,
    resumeDownload,
    setSearchQuery,
    toggleRepo,
  } = props;
  return (          <div className="card">
            <div className="flex gap-2 mb-2">
              <div className="flex-1 relative min-w-0">
                <Search
                  size={14}
                  className="absolute left-3 top-1/2 -translate-y-1/2 text-faint pointer-events-none"
                />
                <input
                  className="input pl-9 w-full text-xs"
                  placeholder={t("Search models (e.g. llama, mistral, qwen)")}
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
                <option value="downloads">{t("By downloads")}</option>
                <option value="likes">{t("By stars")}</option>
                <option value="lastModified">{t("Newest")}</option>
              </select>
              <button className="btn-primary text-xs shrink-0" onClick={() => doSearch()} disabled={searching}>
                {searching ? "…" : t("Search")}
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
                              title={t("Open on HuggingFace")}
                            >
                              <ExternalLink size={12} />
                            </button>
                            <span className="text-xs text-dim shrink-0">
                              {t("by {author}", { author: model.author })}
                            </span>
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
                                                {t("{n} parts", { n: split.total })}
                                              </span>
                                            )}
                                          </div>
                                        </div>
                                        <span className="text-xs text-dim shrink-0">
                                          {formatSize(f.size_bytes ?? 0)}
                                        </span>
                                        {done ? (
                                          <span className="badge-green text-[0.625rem] shrink-0">
                                            {t("Installed")}
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
                              <p className="text-xs text-dim px-2">{t("No GGUF files in this repo.")}</p>
                            )
                          ) : (
                            <p className="text-xs text-dim px-2">{t("Loading files…")}</p>
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
                  <p className="text-sm">{t("Search for GGUF models on HuggingFace.")}</p>
                  <p className="text-xs mt-1 text-faint">
                    {t('Try "llama 3", "mistral", or a quant like "Q4_K_M".')}
                  </p>
                </div>
              )
            )}
          </div>
  );
}

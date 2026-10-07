import {
  ArrowDown,
  ArrowUp,
  Brain,
  ExternalLink,
  Eye,
  FileJson,
  Filter,
  FolderOpen,
  FolderPlus,
  Trash2,
  X,
} from "lucide-react";
import type { ModelDto } from "../../bindings";
import { formatSize, quantColor } from "../../utils/format";
import { t } from "../../utils/i18n";
import type { ModelSortCol } from "./shared";

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

type ModelsTabProps = {
  models: ModelDto[];
  filteredModels: ModelDto[];
  modelDirs: string[];
  downloadDir: string;
  nameFilter: string;
  modelSortCol: ModelSortCol;
  modelSortDir: "asc" | "desc";
  setNameFilter: (v: string) => void;
  toggleModelSort: (col: ModelSortCol) => void;
  setConfigModel: (m: ModelDto) => void;
  addModelDir: () => void;
  removeModelDir: (path: string) => void;
  changeDownloadDir: () => void;
  deleteModel: (path: string) => void;
  openInBrowser: (url: string) => void;
};

export function ModelsTab(props: ModelsTabProps) {
  const {
    models,
    filteredModels,
    modelDirs,
    downloadDir,
    nameFilter,
    modelSortCol,
    modelSortDir,
    setNameFilter,
    toggleModelSort,
    setConfigModel,
    addModelDir,
    removeModelDir,
    changeDownloadDir,
    deleteModel,
    openInBrowser,
  } = props;

  const modelSortIcon = (col: ModelSortCol) =>
    modelSortCol === col ? (
      modelSortDir === "asc" ? <ArrowUp size={10} /> : <ArrowDown size={10} />
    ) : (
      <ArrowUp size={10} className="opacity-0 group-hover:opacity-30" />
    );

  return (          <div className="card">
            <div className="flex items-center justify-end gap-3 mb-2">
              <div className="flex items-center gap-2 min-w-0">
                <span className="text-[0.6875rem] font-mono text-dim truncate max-w-72" title={downloadDir}>
                  {downloadDir || t("Not set")}
                </span>
                <button className="btn-ghost text-[0.6875rem] py-1 shrink-0" onClick={changeDownloadDir}>
                  <FolderOpen size={11} /> {t("Change…")}
                </button>
                <button
                  className="btn-ghost text-[0.6875rem] py-1 shrink-0"
                  onClick={addModelDir}
                  title={t("Scan another folder too")}
                >
                  <FolderPlus size={11} /> {t("Add")}
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
                          title={t("Stop scanning this folder")}
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
                  placeholder={t("Filter by name…")}
                  value={nameFilter}
                  onChange={(e) => setNameFilter(e.target.value)}
                />
              </div>
              <span className="text-[0.6875rem] text-faint shrink-0">
                {filteredModels.length === 1
                  ? t("1 model")
                  : t("{n} models", { n: filteredModels.length })}
              </span>
            </div>
            {models.length === 0 ? (
              <div className="text-center py-6">
                <p className="text-xs text-dim">{t("Nothing downloaded yet.")}</p>
              </div>
            ) : (
              <div>
                <div className="flex items-center gap-2 px-3 py-2 border-b border-border text-[0.625rem] font-semibold text-dim uppercase tracking-wider select-none">
                  <button
                    className="flex-1 flex items-center gap-1 group text-left"
                    onClick={() => toggleModelSort("name")}
                  >
                    {t("Model")} {modelSortIcon("name")}
                  </button>
                  <button
                    className="w-16 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("params")}
                  >
                    {t("Params")} {modelSortIcon("params")}
                  </button>
                  <button
                    className="w-20 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("quant")}
                  >
                    {t("Quant")} {modelSortIcon("quant")}
                  </button>
                  <button
                    className="w-16 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("ctx")}
                  >
                    {t("Ctx")} {modelSortIcon("ctx")}
                  </button>
                  <button
                    className="w-20 flex items-center gap-1 group justify-end"
                    onClick={() => toggleModelSort("size")}
                  >
                    {t("Size")} {modelSortIcon("size")}
                  </button>
                  <span className="w-14 text-center normal-case">{t("Config")}</span>
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
                            <span title={t("Vision model")}>
                              <Eye size={12} className="text-[#3B82F6] shrink-0" />
                            </span>
                          )}
                          {m.is_reasoning && (
                            <span title={t("Reasoning model")}>
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
                          title={t("Model config (JSON)")}
                          onClick={() => setConfigModel(m)}
                        >
                          <FileJson size={12} />
                        </button>
                      </span>
                      <span className="w-8 flex justify-center items-center">
                        {m.hf_repo && (
                          <button
                            className="text-faint hover:text-ink"
                            title={t("Open on HuggingFace")}
                            onClick={() => openInBrowser(`https://huggingface.co/${m.hf_repo}`)}
                          >
                            <ExternalLink size={11} />
                          </button>
                        )}
                      </span>
                      <button
                        className="w-8 flex justify-center text-faint hover:text-accent-red"
                        title={t("Delete model")}
                        onClick={() => deleteModel(m.path)}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  );
                })}
                {filteredModels.length === 0 && (
                  <p className="text-sm text-dim py-6 text-center">
                    {t("No models match the filter.")}
                  </p>
                )}
              </div>
            )}
          </div>
  );
}

import { Pin, RotateCcw, Save, Trash2 } from "lucide-react";
import { DEFAULT_PRESET, presetLabel } from "./labels";

type PresetCardProps = {
  activePreset: string | null;
  presets: string[];
  presetName: string;
  presetFlash: { text: string; tone: "ok" | "err" } | null;
  setPresetName: (v: string) => void;
  applyPresetName: (name: string) => void;
  savePreset: () => void;
  saveDefaults: () => void;
  resetDefaults: () => void;
  deletePreset: () => void;
};

export function PresetCard(props: PresetCardProps) {
  const {
    activePreset,
    presets,
    presetName,
    presetFlash,
    setPresetName,
    applyPresetName,
    savePreset,
    saveDefaults,
    resetDefaults,
    deletePreset,
  } = props;
  return (
    <div className="card">
      <div className="flex items-center justify-between mb-1">
        <h2 className="section-title mb-0">
          Preset{activePreset ? ` — ${presetLabel(activePreset)}` : ""}
        </h2>
      </div>
      <p className="section-desc">Model and folders stay per-session, presets remembered per model.</p>
      <div className="mt-2 flex items-center gap-2">
        <select
          className="input flex-1 min-w-0 py-1 px-2 text-xs"
          value={activePreset ?? DEFAULT_PRESET}
          onChange={(e) => applyPresetName(e.target.value)}
        >
          <option value={DEFAULT_PRESET}>{presetLabel(DEFAULT_PRESET)}</option>
          {presets
            .filter((p) => p !== DEFAULT_PRESET)
            .map((p) => (
              <option key={p} value={p}>
                {presetLabel(p)}
              </option>
            ))}
        </select>
      </div>
      <div className="flex items-center gap-2 mt-2">
        <input
          className="input flex-1 min-w-0 py-1 px-2 text-xs"
          placeholder="Preset name"
          value={presetName}
          onChange={(e) => setPresetName(e.target.value)}
        />
        <button
          className="btn-secondary text-xs py-1 px-2 shrink-0"
          onClick={savePreset}
          title={
            presetName.trim()
              ? `Save as "${presetName.trim()}"`
              : `Update preset "${presetLabel(activePreset ?? DEFAULT_PRESET)}"`
          }
        >
          <Save size={12} />
        </button>
      </div>
      <div className="mt-3 pt-3 border-t border-border flex items-center gap-1">
        <button
          className="btn-ghost text-[0.6875rem] py-1 px-1.5"
          onClick={saveDefaults}
          title="Save the current settings as the default preset"
        >
          <Pin size={12} />
        </button>
        <button
          className="btn-ghost text-[0.6875rem] py-1 px-1.5"
          onClick={resetDefaults}
          title="Reset the default preset to factory defaults"
        >
          <RotateCcw size={12} />
        </button>
        {presetFlash && (
          <span
            className={`text-xs font-medium ml-2 ${
              presetFlash.tone === "ok" ? "text-accent-green" : "text-accent-red"
            }`}
          >
            {presetFlash.text}
          </span>
        )}
        {activePreset && activePreset !== DEFAULT_PRESET && (
          <button
            className="btn-ghost text-[0.6875rem] py-1 px-1.5 ml-auto text-accent-red"
            onClick={deletePreset}
            title={`Delete preset "${presetLabel(activePreset)}"`}
          >
            <Trash2 size={12} />
          </button>
        )}
      </div>
    </div>
  );
}

import { Brain } from "lucide-react";

export const EFFORT_LABELS: Record<string, string> = {
  none: "Off",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  max: "Max",
  xhigh: "X-High",
};

/// OpenAI-style effort levels for provider models.
const EXTERNAL_EFFORT_LEVELS = ["minimal", "low", "medium", "high"];

/// External API mode controls below the composer: the favorite-model
/// switcher (stars come from the Mode tab) plus the provider reasoning level.
export function ExternalChatControls({
  target,
  favorites,
  effort,
  onTarget,
  onEffort,
  disabled,
}: {
  target: string;
  favorites: string[];
  effort: string;
  onTarget: (t: string) => void;
  onEffort: (e: string) => void;
  disabled: boolean;
}) {
  const options = favorites.includes(target) || !target ? favorites : [...favorites, target];
  const modelOf = (t: string) => {
    const cut = t.indexOf(":");
    return cut > 0 ? t.slice(cut + 1) : t;
  };
  return (
    <div className="flex items-center gap-2 mt-2">
      {options.length > 0 ? (
        <select
          className="input py-1 px-2 text-xs font-mono max-w-[16rem]"
          value={target}
          onChange={(e) => onTarget(e.target.value)}
          disabled={disabled}
          title="Favorite model"
        >
          {!target && <option value="">Select model…</option>}
          {options.map((t) => (
            <option key={t} value={t}>
              {modelOf(t)}
            </option>
          ))}
        </select>
      ) : (
        <span className="text-[0.625rem] text-faint">
          Star models on the Mode tab to switch between them here
        </span>
      )}
      <div className="flex items-center gap-1" title="Reasoning effort sent to the provider">
        <Brain size={12} className="text-[#E5484D] shrink-0" />
        <select
          className="input py-1 px-1 text-[0.625rem] w-20"
          value={EXTERNAL_EFFORT_LEVELS.includes(effort) ? effort : ""}
          onChange={(e) => onEffort(e.target.value)}
          disabled={disabled}
        >
          <option value="">Default</option>
          {EXTERNAL_EFFORT_LEVELS.map((level) => (
            <option key={level} value={level}>
              {EFFORT_LABELS[level] ?? level}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}


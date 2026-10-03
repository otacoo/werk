import { useEffect, useRef, useState } from "react";
import { ChevronDown, Star } from "lucide-react";

export type PickerItem = { target: string; provider: string; model: string; stale?: boolean };

const modelOf = (target: string) => {
  const cut = target.indexOf(":");
  return cut > 0 ? target.slice(cut + 1) : target;
};

/// Searchable `provider:model` picker; starred targets pin to the top and
/// also render as one-click chips under the field.
export default function ModelPicker({
  items,
  value,
  favorites,
  onSelect,
  onToggleFavorite,
  dropUp = false,
}: {
  items: PickerItem[];
  value: string;
  favorites: string[];
  onSelect: (target: string) => void;
  onToggleFavorite: (target: string) => void;
  /// Open upward (composer at the window bottom).
  dropUp?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!boxRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const q = query.trim().toLowerCase();
  const sorted = items
    .filter((i) => !q || i.target.toLowerCase().includes(q))
    .sort((a, b) => {
      const fa = favorites.includes(a.target) ? 0 : 1;
      const fb = favorites.includes(b.target) ? 0 : 1;
      return fa !== fb ? fa - fb : a.target.localeCompare(b.target);
    });

  return (
    <div ref={boxRef} className="relative">
      <button
        type="button"
        className="input w-full py-1 px-2 text-xs flex items-center justify-between gap-2 text-left"
        onClick={() => {
          setOpen((o) => !o);
          setQuery("");
        }}
      >
        <span className={`truncate font-mono ${value ? "text-ink" : "text-faint"}`}>
          {value || "Select model…"}
        </span>
        <ChevronDown size={12} className="text-faint shrink-0" />
      </button>
      {open && (
        <div
          className={`absolute z-20 w-full bg-surface-1 border border-border rounded shadow-lg ${
            dropUp ? "bottom-full mb-1" : "mt-1"
          }`}
        >
          <input
            className="w-full bg-transparent py-1.5 px-2 text-xs border-0 border-b border-border outline-none"
            placeholder="Search models…"
            value={query}
            autoFocus
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setOpen(false);
            }}
          />
          <div className="max-h-64 overflow-y-auto py-1">
            {sorted.length === 0 && (
              <p className="px-2 py-1.5 text-[0.6875rem] text-dim">No matching models.</p>
            )}
            {sorted.map((i) => {
              const fav = favorites.includes(i.target);
              return (
                <div
                  key={i.target}
                  className={`flex items-center gap-2 px-2 py-1 cursor-pointer hover:bg-surface-2 ${
                    i.target === value ? "bg-accent/10" : ""
                  }`}
                  onClick={() => {
                    onSelect(i.target);
                    setOpen(false);
                  }}
                >
                  <button
                    type="button"
                    className={`shrink-0 ${fav ? "text-accent-yellow" : "text-faint hover:text-ink"}`}
                    title={fav ? "Unstar" : "Star"}
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleFavorite(i.target);
                    }}
                  >
                    <Star size={11} fill={fav ? "currentColor" : "none"} />
                  </button>
                  <span className="text-xs font-mono text-ink truncate flex-1">{i.model}</span>
                  {i.stale ? (
                    <span className="text-[0.625rem] text-faint shrink-0">not listed</span>
                  ) : (
                    <span className="text-[0.625rem] text-faint shrink-0">{i.provider}</span>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
      {favorites.length > 0 && (
        <div className="flex flex-wrap items-center gap-1 mt-1.5">
          {favorites.map((t) => (
            <button
              key={t}
              type="button"
              title={t}
              className={`px-1.5 py-0.5 rounded border text-[0.625rem] font-mono transition-colors ${
                t === value
                  ? "border-accent/60 bg-accent/10 text-ink"
                  : "border-border text-dim hover:text-ink hover:bg-surface-2"
              }`}
              onClick={() => onSelect(t)}
            >
              {modelOf(t)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

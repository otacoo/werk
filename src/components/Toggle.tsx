export default function Toggle({ label, hint, flag, checked, onChange }: {
  label?: string;
  hint?: string;
  /** The llama-server flag this toggle maps to. */
  flag?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-start gap-3">
      <button
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={`relative shrink-0 w-8 h-4 rounded-full transition-colors mt-0.5 ${
          checked ? "bg-accent" : "bg-surface-3"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 w-3 h-3 rounded-full bg-white shadow transition-transform ${
            checked ? "translate-x-4" : ""
          }`}
        />
      </button>
      {(label || hint) && (
        <div>
          {label && (
            <p className="text-xs font-medium text-ink">
              {label}
              {flag && <span className="font-mono text-[0.625rem] text-faint ml-1.5">{flag}</span>}
            </p>
          )}
          {hint && <p className="text-xs text-faint">{hint}</p>}
        </div>
      )}
    </div>
  );
}

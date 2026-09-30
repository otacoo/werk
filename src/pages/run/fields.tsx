import type { ServerStatus } from "../../bindings";

export function statusDot(status: ServerStatus | null): string {
  if (status?.type === "running") {
    return status.ready ? "bg-accent-green" : "bg-accent-yellow animate-pulse";
  }
  if (status?.type === "starting") return "bg-accent-yellow animate-pulse";
  if (status?.type === "error") return "bg-accent-red";
  return "bg-surface-3";
}

export function Field({ label, flag, hint, children }: {
  label: string;
  flag?: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="label flex flex-col gap-1 min-w-0">
      <span className="flex items-baseline gap-1.5 min-w-0">
        <span title={hint}>{label}</span>
        {flag && <span className="font-mono text-[0.625rem] text-faint">{flag}</span>}
      </span>
      {children}
    </label>
  );
}

export function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="mt-4 first:mt-0">
      <p className="text-[0.8125rem] font-bold uppercase tracking-wider text-ink mb-3">{title}</p>
      {children}
    </div>
  );
}

export function SliderField({ label, flag, hint, min, max, step, value, disabled, format, onChange }: {
  label: string;
  flag?: string;
  hint?: string;
  min: number;
  max: number;
  step: number;
  value: number;
  disabled?: boolean;
  format?: (v: number) => string;
  onChange: (v: number) => void;
}) {
  return (
    <Field label={label} flag={flag} hint={hint}>
      <div className="flex items-center gap-2">
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          disabled={disabled}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          className="flex-1 accent-accent"
        />
        <span className="text-xs font-mono text-ink w-12 text-right">
          {format ? format(value) : String(value)}
        </span>
      </div>
    </Field>
  );
}


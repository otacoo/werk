export const DEFAULT_EXTRA_ROWS: ExtraRow[] = [{ key: "verbosity", value: "3" }];

export interface ExtraRow {
  key: string;
  value: string;
}

export function splitExtra(extra: { [key: string]: string } | undefined): {
  rows: ExtraRow[];
  raw: string;
} {
  const rows: ExtraRow[] = [];
  let raw = "";
  for (const [k, v] of Object.entries(extra ?? {})) {
    if (k === "fit" || k === "tools" || k === "no-chat-template") continue;
    if (k === "__raw__") {
      raw = v;
      continue;
    }
    rows.push({ key: k, value: v });
  }
  rows.sort((a, b) => a.key.localeCompare(b.key));
  return { rows, raw };
}

export function mergeExtra(rows: ExtraRow[], raw: string): { [key: string]: string } {
  const extra: { [key: string]: string } = {};
  for (const r of rows) {
    const key = r.key.trim();
    if (key && key !== "fit" && key !== "__raw__" && key !== "tools") extra[key] = r.value;
  }
  if (raw.trim()) extra.__raw__ = raw;
  return extra;
}


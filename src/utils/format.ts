export function formatElapsed(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const totalSec = Math.round(ms / 1000);
  if (totalSec < 60) return `${(ms / 1000).toFixed(1)}s`;
  const totalMin = Math.floor(totalSec / 60);
  if (totalMin < 60) {
    const s = totalSec % 60;
    return s === 0 ? `${totalMin}m` : `${totalMin}m ${s}s`;
  }
  const totalHr = Math.floor(totalMin / 60);
  if (totalHr < 24) {
    const m = totalMin % 60;
    return m === 0 ? `${totalHr}h` : `${totalHr}h ${m}m`;
  }
  const d = Math.floor(totalHr / 24);
  const h = totalHr % 24;
  return h === 0 ? `${d}d` : `${d}d ${h}h`;
}

export function shortCpuName(raw: string): string {
  const clean = raw.replace(/\(R\)|\(TM\)/g, "").replace(/\s+/g, " ");
  const m =
    clean.match(/i[3579]-\w+/) ||
    clean.match(/Ryzen \d+ \w+/) ||
    clean.match(/Core Ultra \d+ \w+/) ||
    clean.match(/Xeon \w[\w-]*/) ||
    clean.match(/EPYC \w+/) ||
    clean.match(/Apple M\d\w*/);
  return m ? m[0] : clean.replace(/\s+CPU.*/, "").trim();
}

export function shortGpuName(raw: string): string {
  const m =
    raw.match(/RTX \w+(\s*Ti)?(\s*SUPER)?/) ||
    raw.match(/GTX \w+(\s*Ti)?/) ||
    raw.match(/RX \w+(\s*XT)?(\s*X)?/) ||
    raw.match(/Arc \w+/) ||
    raw.match(/Apple M\d\w*/) ||
    raw.match(/Radeon Pro \w+/) ||
    raw.match(/A\d{3,4}\b/);
  return m ? m[0] : raw.replace(/NVIDIA |GeForce |AMD |Intel /g, "").trim();
}

export function formatSize(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${(bytes / 1024 ** 2).toFixed(2)} MB`;
}

export function quantColor(quant: string): string {
  const q = quant.toUpperCase().replace(/^IQ/, "Q").replace(/^MXFP/, "Q");
  if (q === "F16" || q === "BF16" || q === "F32") return "badge-blue";
  if (q.startsWith("Q8")) return "badge-blue";
  if (q.startsWith("Q7")) return "badge-blue";
  if (q.startsWith("Q6")) return "badge-cyan";
  if (q.startsWith("Q5")) return "badge-green";
  if (q.startsWith("Q4")) return "badge-yellow";
  if (q.startsWith("Q3")) return "badge-orange";
  if (q.startsWith("Q2")) return "badge-red";
  if (q.startsWith("Q1")) return "badge-red-dark";
  return "badge-gray";
}

export function quantSortKey(quant: string | null): number {
  if (!quant) return 999;
  const q = quant.toUpperCase().replace(/^IQ/, "Q").replace(/^MXFP/, "Q");
  if (q === "F32") return 0;
  if (q === "BF16") return 1;
  if (q === "F16") return 2;
  const m = q.match(/^Q(\d)/);
  if (m) return 10 - parseInt(m[1], 10);
  return 50;
}

/// Quant tag from a filename stem (`model-Q4_K_M` -> `Q4_K_M`).
export function quantFromName(filename: string): string | null {
  const stem = filename.replace(/\.gguf$/i, "");
  const m = stem.match(/[-._]((?:Q|IQ)\d[A-Z0-9_]*|MXFP\d+|F16|F32|BF16)$/i);
  return m ? m[1].toUpperCase() : null;
}

export function isImatrixFile(filename: string): boolean {
  const lower = filename.toLowerCase();
  return lower.includes("imatrix") || lower.includes("importance_matrix");
}

export function isDsparkFile(filename: string): boolean {
  return filename.toLowerCase().includes("dspark");
}

export function isMmprojFile(filename: string): boolean {
  return filename.toLowerCase().includes("mmproj");
}

/// Split-GGUF suffix (`model-00001-of-00005.gguf` -> parts), else null.
export function parseSplitSuffix(filename: string): { base: string; index: number; total: number } | null {
  const stem = filename.replace(/\.gguf$/i, "");
  const of = stem.match(/^(.*)-(\d+)-of-(\d+)$/);
  if (!of) return null;
  const total = parseInt(of[3], 10);
  const index = parseInt(of[2], 10);
  if (total < 2 || index === 0 || index > total) return null;
  return { base: of[1], index, total };
}

/// Companion sidecar name: model stem + sidecar stem (`Qwen-mmproj-f16.gguf`).
export function mmprojSaveName(modelFile: string, mmprojFile: string): string {
  const stem = (s: string) => s.replace(/\.gguf$/i, "");
  return `${stem(modelFile)}-${stem(mmprojFile)}.gguf`;
}

export function mbToGb(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.max(1, Math.round(mb))} MB`;
}

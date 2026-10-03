import type { ModelDto } from "../../bindings";
import { quantSortKey } from "../../utils/format";

export type HfSort = "downloads" | "likes" | "lastModified";
export type ModelSortCol = "name" | "params" | "quant" | "size" | "ctx";
export type DashTab = "runtime" | "models" | "browse";

export interface ActiveDl {
  repoId: string;
  filename: string;
  saveAs: string | null;
  split: string[] | null;
}

export const baseOf = (path: string) => path.split("/").pop() ?? path;

export const MODEL_SORTERS: Record<ModelSortCol, (a: ModelDto, b: ModelDto) => number> = {
  name: (a, b) => a.name.localeCompare(b.name),
  params: (a, b) => parseFloat(a.params_b ?? "0") - parseFloat(b.params_b ?? "0"),
  quant: (a, b) => quantSortKey(a.quant ?? null) - quantSortKey(b.quant ?? null),
  size: (a, b) => (a.size_bytes ?? 0) - (b.size_bytes ?? 0),
  ctx: (a, b) => (a.context_length ?? 0) - (b.context_length ?? 0),
};

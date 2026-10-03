//! File tools (mirrors llama.cpp `--tools`).

export interface ToolDef {
  name: string;
  label: string;
  hint: string;
  dangerous?: boolean;
}

// Only cross-build tools are listed - unknown names fail server startup.
export const KNOWN_TOOLS: ToolDef[] = [
  { name: "read_file", label: "Read File", hint: "Read text files" },
  { name: "grep_search", label: "Grep Search", hint: "Search file contents with regex" },
  { name: "file_glob_search", label: "File Glob Search", hint: "List files matching a glob pattern" },
  { name: "get_info", label: "Get Info", hint: "Query file and folder metadata" },
  { name: "write_file", label: "Write File", hint: "Create or overwrite files" },
  { name: "edit_file", label: "Edit File", hint: "Apply line-range edits to files" },
  { name: "exec_shell_command", label: "Shell Command", hint: "Run arbitrary shell commands", dangerous: true },
];

// "all" expands to every known tool; unknown names are dropped (they fail startup).
export function effectiveTools(value: string): Set<string> {
  const sel = new Set<string>();
  if (!value) return sel;
  if (value.trim().toLowerCase() === "all") {
    for (const t of KNOWN_TOOLS) sel.add(t.name);
    return sel;
  }
  for (const name of value.split(",")) {
    const n = name.trim();
    if (n && KNOWN_TOOLS.some((t) => t.name === n)) sel.add(n);
  }
  return sel;
}

export function toolsArgValue(value: string): string {
  const sel = effectiveTools(value);
  if (sel.size === 0) return "";
  if (sel.size === KNOWN_TOOLS.length) return "all";
  return [...sel].sort().join(",");
}

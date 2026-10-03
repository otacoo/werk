import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { commands } from "../../bindings";
import type { AppConfig, SystemPromptPreset } from "../../bindings";
import { call } from "../../utils/ipc";

export function SystemPromptCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  const [promptDraft, setPromptDraft] = useState<string | null>(null);  const [builtInPrompt, setBuiltInPrompt] = useState("");
  const [renaming, setRenaming] = useState<{ name: string; value: string } | null>(null);

  useEffect(() => {
    call(commands.getHarnessSystemPromptDefault()).then(setBuiltInPrompt).catch(() => {});
    setPromptDraft(null);
  }, []);

  const storedPrompt = appConfig?.harness_system_prompt ?? "";
  const shownBase = storedPrompt !== "" ? storedPrompt : builtInPrompt;
  const promptDirty = promptDraft !== null && promptDraft.trim() !== shownBase.trim();
  const presets = appConfig?.system_prompt_presets ?? [];

  // The active preset follows the stored prompt: the default, a preset whose
  // prompt matches, or a custom override (null).
  const activePreset = (() => {
    if (storedPrompt === "") return "default";
    const hit = presets.find((p) => p.prompt.trim() === storedPrompt.trim());
    return hit ? hit.name : null;
  })();

  const savePresets = (next: SystemPromptPreset[]) => {
    setAppConfig((c) => (c ? { ...c, system_prompt_presets: next } : c));
    call(commands.setSystemPromptPresets(next)).catch(() => {});
  };

  // Clicking a preset activates it right away; Save only stores edits.
  const activate = (prompt: string) => {
    setPromptDraft(null);
    setAppConfig((c) => (c ? { ...c, harness_system_prompt: prompt.trim() || null } : c));
    call(commands.setSystemPrompt(prompt)).catch(() => {});
  };

  const addPreset = () => {
    if (presets.length >= 5) return;
    const name = `Preset ${presets.length + 1}`;
    const prompt = (promptDraft ?? shownBase).trim();
    savePresets([...presets, { name, prompt }]);
    activate(prompt);
  };

  const renamePreset = (oldName: string, newName: string) => {
    const trimmed = newName.trim();
    if (!trimmed || trimmed === oldName) return;
    savePresets(presets.map((p) => (p.name === oldName ? { ...p, name: trimmed } : p)));
  };

  const deletePreset = (name: string) => {
    savePresets(presets.filter((p) => p.name !== name));
  };

  const presetButton = (p: SystemPromptPreset) => (
    <button
      key={p.name}
      className={`px-2.5 py-1 text-xs rounded transition-colors ${
        activePreset === p.name ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
      }`}
      onClick={(e) => {
        if (e.shiftKey) {
          deletePreset(p.name);
          return;
        }
        activate(p.prompt);
      }}
      onDoubleClick={() => setRenaming({ name: p.name, value: p.name })}
      title={`${p.name} — click to activate, double-click to rename, shift-click to delete`}
    >
      {p.name}
    </button>
  );

  return (
    <div className="card">
      <h2 className="section-title mb-1">System prompt</h2>
      <p className="section-desc">
        Werk ships a built-in system prompt that is updated dynamically for your OS, mode, and the
        tools available; the project directory and saved memory are appended to custom prompts and
        presets as well.
        <br />
        Click a preset to activate it; edit and Save to update it. Clearing and saving restores the
        built-in default.
      </p>
      <div className="flex flex-wrap items-center gap-1.5 mt-3">
        <button
          className={`px-2.5 py-1 text-xs rounded transition-colors ${
            activePreset === "default" ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
          }`}
          onClick={() => activate("")}
          title="Use the built-in default prompt"
        >
          Default
        </button>
        {presets.map((p) =>
          renaming?.name === p.name ? (
            <input
              key={p.name}
              autoFocus
              className="input px-2 py-1 text-xs w-28"
              value={renaming.value}
              onChange={(e) => setRenaming({ name: p.name, value: e.target.value })}
              onBlur={() => {
                renamePreset(p.name, renaming.value);
                setRenaming(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                if (e.key === "Escape") setRenaming(null);
              }}
            />
          ) : (
            presetButton(p)
          ),
        )}
        {presets.length < 5 && (
          <button
            className="px-2 py-1 text-dim hover:text-ink hover:bg-accent/10 rounded transition-colors"
            onClick={addPreset}
            title="Add a preset from the current prompt (max 5)"
          >
            <Plus size={12} />
          </button>
        )}
      </div>
      <textarea
        className="input w-full mt-3 font-mono text-xs leading-relaxed"
        rows={8}
        value={promptDraft ?? shownBase}
        onChange={(e) => setPromptDraft(e.target.value)}
      />
      <div className="flex items-center gap-2 mt-2">
        <button
          className="btn-primary text-xs"
          disabled={!promptDirty}
          onClick={() => {
            const v = (promptDraft ?? "").trim();
            if (activePreset && activePreset !== "default") {
              // Update the active preset in place and keep it active.
              savePresets(presets.map((p) => (p.name === activePreset ? { ...p, prompt: v } : p)));
              activate(v);
              return;
            }
            const next = v === "" || v === builtInPrompt.trim() ? "" : v;
            activate(next);
          }}
        >
          Save
        </button>
        {activePreset === "default" && !promptDirty && (
          <span className="text-[0.6875rem] text-faint">Using built-in default</span>
        )}
        {activePreset === null && (
          <span className="text-[0.6875rem] text-dim">Custom prompt active</span>
        )}
        {activePreset && activePreset !== "default" && (
          <span className="text-[0.6875rem] text-dim">Prompt "{activePreset}" active</span>
        )}
        <span
          className="ml-auto text-[0.6875rem] text-faint"
          title="Rough estimate from the character count; the model's tokenizer may differ."
        >
          ~{Math.ceil((promptDraft ?? shownBase).length / 4)} tokens
        </span>
      </div>
    </div>
  );
}

/// Visibility of the agent's own instruction files for every tool.

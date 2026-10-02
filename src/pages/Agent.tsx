import { useState } from "react";
import { useAppConfig } from "../utils/useAppConfig";
import { BehaviorCard, ServerLifecycleCard, UtilityModelCard } from "./agent/behavior";
import { SystemPromptCard } from "./agent/prompt";
import { AgentFilesCard, SensitiveShieldingCard } from "./agent/access";
import { MemoryCard, SkillsCard } from "./agent/memory";

type AgentTab = "behavior" | "prompt" | "access" | "memory";

const TABS: { id: AgentTab; label: string; desc: string }[] = [
  { id: "behavior", label: "Behavior", desc: "How the agent runs: verification, turns, housekeeping model, and the local server lifecycle." },
  { id: "prompt", label: "Prompt", desc: "The system prompt and its presets." },
  { id: "access", label: "Access", desc: "What the agent's tools may read or write: agent files and sensitive shielding." },
  { id: "memory", label: "Memory", desc: "Saved facts and learned skills, editable by hand." },
];

/// Agent settings surface: everything that shapes the harness, split into
/// focused sub-tabs (moved out of the Settings menu).
export default function Agent({ active = true }: { active?: boolean }) {
  const [appConfig, setAppConfig, , refreshConfig] = useAppConfig(true);
  const [tab, setTab] = useState<AgentTab>("behavior");
  const current = TABS.find((t) => t.id === tab) ?? TABS[0];

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-6xl mx-auto">
        <div>
          <h1 className="section-title">Agent</h1>
          <p className="section-desc">Everything that shapes the chat agent.</p>
        </div>

        <div className="flex items-center gap-0.5">
          {TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`px-2.5 py-1 rounded text-xs font-medium transition-colors ${
                tab === t.id ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>

        <p className="section-desc">{current.desc}</p>

        {/* Only the active sub-tab's cards mount, so their data is fresh
            each visit (memory and skills change during runs). */}
        {active && tab === "behavior" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <div className="space-y-4">
              <BehaviorCard appConfig={appConfig} setAppConfig={setAppConfig} />
            </div>
            <div className="space-y-4">
              <UtilityModelCard appConfig={appConfig} setAppConfig={setAppConfig} />
              <ServerLifecycleCard appConfig={appConfig} refresh={refreshConfig} />
            </div>
          </div>
        )}

        {active && tab === "prompt" && (
          <div className="max-w-3xl">
            <SystemPromptCard appConfig={appConfig} setAppConfig={setAppConfig} />
          </div>
        )}

        {active && tab === "access" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <AgentFilesCard appConfig={appConfig} setAppConfig={setAppConfig} refresh={refreshConfig} />
            <SensitiveShieldingCard appConfig={appConfig} refresh={refreshConfig} />
          </div>
        )}

        {active && tab === "memory" && (
          <div className="grid grid-cols-2 gap-4 items-start">
            <SkillsCard />
            <MemoryCard />
          </div>
        )}
      </div>
    </div>
  );
}

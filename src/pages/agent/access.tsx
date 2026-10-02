import { useEffect, useState } from "react";
import { commands } from "../../bindings";
import type { AppConfig } from "../../bindings";
import { call } from "../../utils/ipc";
import Toggle from "../../components/Toggle";

export function AgentFilesCard({ appConfig, setAppConfig, refresh }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const hidden = appConfig?.agent_files_hidden ?? false;

  const setVisible = async (visible: boolean) => {
    setAppConfig((c) => (c ? { ...c, agent_files_hidden: !visible } : c));
    try {
      await call(commands.setAgentFilesHidden(!visible));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await refresh();
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Agent files</h2>
      <p className="section-desc">
        <span className="font-mono">AGENTS.md</span> and <span className="font-mono">.agent*</span>{" "}
        files carry the agent's own instructions. When hidden, every tool — reads, writes, edits,
        globs, and content search — skips them for the next run.
      </p>
      <div className="mt-3">
        <Toggle label="Visible to the agent" checked={!hidden} onChange={setVisible} />
      </div>
      {error && <p className="text-[0.625rem] text-accent-red mt-2 break-words">{error}</p>}
    </div>
  );
}

/// Sensitive-file policy: built-ins + .gitignore, plus the user lists.
export function SensitiveShieldingCard({ appConfig, refresh }: {
  appConfig: AppConfig | null;
  refresh: () => Promise<void>;
}) {
  const [draft, setDraft] = useState({ patterns: "", allow: "" });
  const [flash, setFlash] = useState<{ text: string; tone: "ok" | "err" } | null>(null);

  useEffect(() => {
    setDraft({
      patterns: (appConfig?.sensitive_patterns ?? []).join(", "),
      allow: (appConfig?.sensitive_allow ?? []).join(", "),
    });
  }, [appConfig?.sensitive_patterns, appConfig?.sensitive_allow]);

  const save = async () => {
    const split = (s: string) => s.split(/[\n,]+/).map((x) => x.trim()).filter(Boolean);
    setFlash(null);
    try {
      await call(commands.setSensitiveShielding(split(draft.patterns), split(draft.allow)));
      await refresh();
      setFlash({ text: "Saved", tone: "ok" });
    } catch (e) {
      setFlash({ text: String(e), tone: "err" });
    }
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Sensitive shielding</h2>
      <p className="section-desc">
        Add patterns or files to keep away from the model, or add an exception to the general rule: credential paths are blocked from every agent tool by default with built-in patterns (<span className="font-mono">.env*</span>,{" "}
        <span className="font-mono">*.pem</span>, <span className="font-mono">.ssh/</span>,{" "}
        <span className="font-mono">*.sqlite</span>, …) and everything matched by the project's{" "}
        <span className="font-mono">.gitignore</span>. The rules here will win over git repo rules.
      </p>
      <label className="label flex flex-col gap-1 mt-3">
        <span title="Extra globs blocked on top of the built-ins; a pattern with a slash matches the project path.">
          Extra patterns
        </span>
        <textarea
          className="input w-full font-mono text-xs"
          rows={2}
          placeholder="*.secret, config/private/*"
          value={draft.patterns}
          onChange={(e) => setDraft((d) => ({ ...d, patterns: e.target.value }))}
        />
      </label>
      <label className="label flex flex-col gap-1 mt-2">
        <span title="Globs exempted from the policy, e.g. fixtures named *.pem.">Exceptions</span>
        <textarea
          className="input w-full font-mono text-xs"
          rows={2}
          placeholder="test/fixtures/*.pem"
          value={draft.allow}
          onChange={(e) => setDraft((d) => ({ ...d, allow: e.target.value }))}
        />
      </label>
      <div className="flex items-center gap-2 mt-2">
        <button className="btn-primary text-xs" onClick={save}>
          Save
        </button>
        {flash && (
          <span
            className={`text-[0.6875rem] ${flash.tone === "ok" ? "text-accent-green" : "text-accent-red"}`}
          >
            {flash.text}
          </span>
        )}
      </div>
    </div>
  );
}

/// Local server lifecycle: auto-start on send and idle unload.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen, ImagePlus, Plus, Sparkles, Trash2, UserRound, X } from "lucide-react";
import { commands } from "../bindings";
import type { CardSummary, CharacterCard, MemoryFileDto, ReasoningOptions } from "../bindings";
import { call } from "../utils/ipc";
import { notifyConfigChanged } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { useAppConfig } from "../utils/useAppConfig";
import { EFFORT_LABELS } from "./chat/external-controls";
import type { Tab } from "../App";

type RP = {
  card_id: string | null;
  greeting: number;
  user_name: string;
  user_description: string;
  temperature: number | null;
  top_p: number | null;
  repeat_penalty: number | null;
  reasoning_effort: string | null;
  system_prompt: string | null;
};

const EMPTY: RP = {
  card_id: null,
  greeting: 0,
  user_name: "",
  user_description: "",
  temperature: null,
  top_p: null,
  repeat_penalty: null,
  reasoning_effort: null,
  system_prompt: null,
};

/// Template levels for the effort select, keeping a stored value the current
/// template no longer lists so it stays visible until changed.
function levelsOf(opts: ReasoningOptions, stored: string | null): string[] {
  if (stored && !opts.levels.includes(stored)) return [...opts.levels, stored];
  return opts.levels;
}

export default function Roleplay({ go }: { go: (t: Tab) => void }) {
  const [appConfig, setAppConfig] = useAppConfig(true);
  const [cards, setCards] = useState<CardSummary[]>([]);
  const [activeCard, setActiveCard] = useState<CharacterCard | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reasoningOpts, setReasoningOpts] = useState<ReasoningOptions | null>(null);

  // Effort levels come from the loaded model's chat template; refresh when the
  // server (re)starts with a different model.
  useEffect(() => {
    const refresh = () =>
      call(commands.harnessReasoningOptions()).then(setReasoningOpts).catch(() => {});
    refresh();
    let wasRunning = getServerStatus().type === "running";
    return subscribeServerStatus((s) => {
      const running = s.type === "running";
      if (running && !wasRunning) refresh();
      wasRunning = running;
    });
  }, []);

  const rp: RP = { ...EMPTY, ...(appConfig?.roleplay ?? {}) };
  // Persona edits commit on blur so typing does not write the config per key.
  const [personaName, setPersonaName] = useState("");
  const [personaDesc, setPersonaDesc] = useState("");
  const [avatar, setAvatar] = useState<string | null>(null);
  useEffect(() => {
    setPersonaName(rp.user_name);
    setPersonaDesc(rp.user_description);
  }, [rp.user_name, rp.user_description]);

  const loadAvatar = () => {
    call(commands.roleplayUserAvatar())
      .then(setAvatar)
      .catch(() => setAvatar(null));
  };
  useEffect(loadAvatar, []);

  const importAvatar = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "Image", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }],
    });
    if (!picked || typeof picked !== "string") return;
    try {
      await call(commands.roleplaySetUserAvatar(picked));
      loadAvatar();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const clearAvatar = async () => {
    try {
      await call(commands.roleplaySetUserAvatar(null));
      setAvatar(null);
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const loadCards = () => {
    call(commands.roleplayListCards()).then(setCards).catch((e) => setError(String(e)));
  };
  useEffect(loadCards, []);

  useEffect(() => {
    if (!rp.card_id) {
      setActiveCard(null);
      return;
    }
    let alive = true;
    call(commands.roleplayGetCard(rp.card_id))
      .then((c) => alive && setActiveCard(c))
      .catch(() => alive && setActiveCard(null));
    return () => {
      alive = false;
    };
  }, [rp.card_id]);

  const save = (next: Partial<RP>) => {
    const merged = { ...rp, ...next };
    setAppConfig((c) => (c ? { ...c, roleplay: merged } : c));
    call(commands.setRoleplayConfig(merged))
      .then(notifyConfigChanged)
      .catch((e) => setError(String(e)));
  };

  const importCard = async () => {
    setError(null);
    const picked = await open({
      multiple: false,
      filters: [{ name: "Character card", extensions: ["png", "json"] }],
    });
    if (!picked || typeof picked !== "string") return;
    setBusy(true);
    try {
      const card = await call(commands.roleplayImportCard(picked));
      loadCards();
      save({ card_id: card.id, greeting: 0 });
    } catch (e) {
      setError(String(e));
    }
    setBusy(false);
  };

  const removeCard = async (card: CardSummary) => {
    if (!window.confirm(`Delete the card "${card.name}"?`)) return;
    setError(null);
    try {
      await call(commands.roleplayDeleteCard(card.id));
      loadCards();
      if (rp.card_id === card.id) save({ card_id: null, greeting: 0 });
    } catch (e) {
      setError(String(e));
    }
  };

  const startChat = async () => {
    setError(null);
    try {
      await call(commands.roleplayStartChat());
      go("talk");
    } catch (e) {
      setError(String(e));
    }
  };

  const greetings = activeCard ? [activeCard.first_mes, ...activeCard.alternate_greetings] : [];
  const greetingText = (() => {
    const raw = greetings[rp.greeting] ?? "";
    if (!activeCard) return "";
    const user = rp.user_name.trim() || "User";
    return raw.replace(/\{\{char\}\}/g, activeCard.name).replace(/\{\{user\}\}/g, user);
  })();

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4 max-w-6xl mx-auto">
        <div>
          <h1 className="section-title">Roleplay</h1>
          <p className="section-desc">
            Character cards and everything that shapes a roleplay chat.
          </p>
        </div>

        <div className="card flex items-center justify-between gap-3">
          <div className="min-w-0">
            <h2 className="section-title mb-1">Talk</h2>
            <p className="section-desc">
              One continuous discussion per character, with only the remember tool and no
              approvals.
            </p>
          </div>
          <button
            className="btn-primary text-xs shrink-0"
            disabled={!rp.card_id}
            onClick={startChat}
            title={
              rp.card_id ? "Start over with the selected greeting" : "Pick a character card first"
            }
          >
            <Sparkles size={12} /> Start chat
          </button>
        </div>

        <div className="grid grid-cols-2 gap-4 items-start">
          <div className="space-y-4">
            <div className="card">
              <div className="flex items-center justify-between gap-2 mb-1">
                <h2 className="section-title mb-0">Characters</h2>
                <button
                  className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
                  onClick={importCard}
                  disabled={busy}
                  title="Import a PNG character card or a JSON card"
                >
                  <Plus size={11} /> Import
                </button>
              </div>
              <p className="section-desc">
                PNG cards (SillyTavern V1-V3, AICC) or plain JSON. Stored in the app data folder;
                the image doubles as the avatar.
              </p>
              {cards.length === 0 && (
                <p className="text-[0.6875rem] text-dim mt-3">
                  No cards yet — import one to get started.
                </p>
              )}
              <div className="space-y-1.5 mt-3">
                {cards.map((card) => (
                  <div
                    key={card.id}
                    className={`flex items-center gap-2.5 border rounded px-2.5 py-2 cursor-pointer transition-colors ${
                      rp.card_id === card.id
                        ? "border-accent bg-accent/10"
                        : "border-border hover:bg-surface-2"
                    }`}
                    onClick={() => save({ card_id: card.id, greeting: 0 })}
                  >
                    <CardAvatar id={card.id} name={card.name} />
                    <div className="flex-1 min-w-0">
                      <p className="text-xs text-ink font-medium truncate">{card.name}</p>
                      <p className="text-[0.625rem] text-faint truncate">
                        {card.spec} · {card.greetings} greeting{card.greetings === 1 ? "" : "s"}
                        {card.tags.length > 0 ? ` · ${card.tags.slice(0, 3).join(", ")}` : ""}
                      </p>
                    </div>
                    {rp.card_id === card.id && (
                      <span className="badge-gray text-[0.5625rem] shrink-0">active</span>
                    )}
                    <button
                      className="btn-ghost p-1 shrink-0 text-faint hover:text-accent-red"
                      title="Delete this card"
                      onClick={(e) => {
                        e.stopPropagation();
                        void removeCard(card);
                      }}
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                ))}
              </div>
            </div>

            <div className="card">
              <h2 className="section-title mb-1">You</h2>
              <p className="section-desc">
                Your name, description, and avatar for the Talk bubbles; cards reference the name
                as {"{{user}}"}.
              </p>
              <div className="flex items-center gap-3 mt-3">
                {avatar ? (
                  <img
                    src={avatar}
                    alt="Your avatar"
                    className="w-11 h-11 rounded object-cover border border-border shrink-0"
                  />
                ) : (
                  <div className="w-11 h-11 rounded border border-border bg-surface-2 flex items-center justify-center text-faint shrink-0">
                    <UserRound size={13} />
                  </div>
                )}
                <div className="flex items-center gap-1.5">
                  <button
                    className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
                    onClick={importAvatar}
                  >
                    <ImagePlus size={11} /> {avatar ? "Replace avatar" : "Import avatar"}
                  </button>
                  {avatar && (
                    <button
                      className="btn-ghost text-[0.625rem] py-0.5 px-1.5 text-accent-red"
                      onClick={clearAvatar}
                    >
                      <X size={11} /> Clear
                    </button>
                  )}
                </div>
              </div>
              <label className="block mt-3">
                <span className="text-[0.6875rem] text-dim">Name</span>
                <input
                  className="input w-full mt-1"
                  placeholder="User"
                  value={personaName}
                  onChange={(e) => setPersonaName(e.target.value)}
                  onBlur={() => {
                    if (personaName !== rp.user_name) save({ user_name: personaName });
                  }}
                />
              </label>
              <label className="block mt-2">
                <span className="text-[0.6875rem] text-dim">Description</span>
                <textarea
                  className="input w-full mt-1 text-xs"
                  rows={3}
                  placeholder="Optional: who you are in the story."
                  value={personaDesc}
                  onChange={(e) => setPersonaDesc(e.target.value)}
                  onBlur={() => {
                    if (personaDesc !== rp.user_description) save({ user_description: personaDesc });
                  }}
                />
              </label>
            </div>
          </div>

          <div className="space-y-4">
            {activeCard && (
              <div className="card">
                <h2 className="section-title mb-1">Greeting</h2>
                <p className="section-desc">The opener shown when a roleplay chat starts.</p>
                <select
                  className="input w-full mt-3"
                  value={rp.greeting}
                  onChange={(e) => save({ greeting: Number(e.target.value) })}
                >
                  {greetings.map((_, i) => (
                    <option key={i} value={i}>
                      {i === 0 ? "First message" : `Alternate ${i}`}
                    </option>
                  ))}
                </select>
                {greetingText && (
                  <pre className="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap break-words text-[0.6875rem] leading-snug text-dim border border-border rounded p-2 bg-surface-2 select-text">
                    {greetingText}
                  </pre>
                )}
              </div>
            )}

            <div className="card">
              <h2 className="section-title mb-1">Generation settings</h2>
              <p className="section-desc">
                Per-request overrides for roleplay runs; empty means the server default.
              </p>
              <div className="grid grid-cols-3 gap-3 mt-3">
                <OptNumber
                  label="Temperature"
                  step={0.05}
                  value={rp.temperature}
                  onCommit={(v) => save({ temperature: v })}
                />
                <OptNumber
                  label="Top-P"
                  step={0.05}
                  value={rp.top_p}
                  onCommit={(v) => save({ top_p: v })}
                />
                <OptNumber
                  label="Repeat penalty"
                  step={0.05}
                  value={rp.repeat_penalty}
                  onCommit={(v) => save({ repeat_penalty: v })}
                />
              </div>
              {reasoningOpts?.supported &&
                (reasoningOpts.levels.length > 0 || rp.reasoning_effort) && (
                  <label className="block mt-3">
                    <span className="text-[0.6875rem] text-dim">Reasoning effort</span>
                    <select
                      className="input w-full mt-1"
                      value={rp.reasoning_effort ?? ""}
                      onChange={(e) => save({ reasoning_effort: e.target.value || null })}
                      title="Levels come from the loaded model's chat template"
                    >
                      <option value="">Default</option>
                      {levelsOf(reasoningOpts, rp.reasoning_effort).map((level) => (
                        <option key={level} value={level}>
                          {EFFORT_LABELS[level] ?? level}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
            </div>

            <SystemPromptCard
              card={activeCard}
              stored={rp.system_prompt ?? ""}
              onSave={(p) => save({ system_prompt: p })}
            />

            <RoleplayMemoryCard cardId={rp.card_id} />
          </div>
        </div>

        {error && <p className="text-xs text-accent-red">{error}</p>}
      </div>
    </div>
  );
}

function CardAvatar({ id, name }: { id: string; name: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    call(commands.roleplayCardAvatar(id))
      .then((u) => alive && setUrl(u))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [id]);
  return url ? (
    <img
      src={url}
      alt={name}
      className="w-11 h-11 rounded object-cover border border-border shrink-0"
    />
  ) : (
    <div className="w-11 h-11 rounded border border-border bg-surface-2 flex items-center justify-center text-faint shrink-0">
      <Sparkles size={13} />
    </div>
  );
}

function OptNumber({
  label,
  step,
  value,
  onCommit,
}: {
  label: string;
  step: number;
  value: number | null;
  onCommit: (v: number | null) => void;
}) {
  const [draft, setDraft] = useState(value == null ? "" : String(value));
  useEffect(() => {
    setDraft(value == null ? "" : String(value));
  }, [value]);
  return (
    <label className="block">
      <span className="text-[0.6875rem] text-dim">{label}</span>
      <input
        type="number"
        step={step}
        min={0}
        className="input w-full mt-1"
        placeholder="default"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          const text = draft.trim();
          if (text === "") {
            onCommit(null);
            return;
          }
          const n = Number(text);
          if (Number.isFinite(n)) onCommit(n);
          else setDraft(value == null ? "" : String(value));
        }}
      />
    </label>
  );
}

function SystemPromptCard({
  card,
  stored,
  onSave,
}: {
  card: CharacterCard | null;
  stored: string;
  onSave: (prompt: string | null) => void;
}) {
  const [builtIn, setBuiltIn] = useState("");
  const [draft, setDraft] = useState<string | null>(null);

  useEffect(() => {
    call(commands.roleplaySystemPromptDefault())
      .then(setBuiltIn)
      .catch(() => {});
  }, []);
  useEffect(() => {
    setDraft(null);
  }, [card?.id]);

  const cardPrompt = (card?.system_prompt ?? "").trim();
  const fallback = cardPrompt || builtIn;
  const shown = stored.trim() !== "" ? stored : fallback;
  const dirty = draft !== null && draft.trim() !== shown.trim();
  const source =
    stored.trim() !== ""
      ? "Custom prompt active"
      : cardPrompt
        ? "Using the card's prompt"
        : "Using the built-in default";

  return (
    <div className="card">
      <div className="flex items-baseline justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">System prompt</h2>
        {stored.trim() !== "" && (
          <button
            className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
            onClick={() => {
              setDraft(null);
              onSave(null);
            }}
            title="Clear the custom prompt and follow the card or built-in default"
          >
            Reset
          </button>
        )}
      </div>
      <p className="section-desc">
        The core roleplay instructions. The character's details, your persona, world info, and
        memory are appended automatically at run time.
      </p>
      <textarea
        className="input w-full mt-3 font-mono text-[0.6875rem] leading-snug"
        rows={8}
        value={draft ?? shown}
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="flex items-center gap-2 mt-2">
        <button
          className="btn-primary text-[0.625rem] py-0.5 px-2"
          disabled={!dirty}
          onClick={() => {
            const v = (draft ?? "").trim();
            setDraft(null);
            onSave(v === "" ? null : v);
          }}
        >
          Save
        </button>
        <span className="text-[0.6875rem] text-faint ml-auto">{source}</span>
      </div>
    </div>
  );
}

function RoleplayMemoryCard({ cardId }: { cardId: string | null }) {
  const [file, setFile] = useState<MemoryFileDto | null>(null);
  const [draft, setDraft] = useState("");
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    call(commands.harnessMemoryGet("roleplay"))
      .then((f) => {
        setFile(f);
        setDraft(f.text);
        setDirty(false);
      })
      .catch(() => {});
  };
  useEffect(load, [cardId]);

  const save = async () => {
    setError(null);
    try {
      await call(commands.harnessMemorySet("roleplay", draft));
      setDirty(false);
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="card">
      <div className="flex items-baseline justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Memory</h2>
        <div className="flex items-center gap-1 shrink-0">
          {file?.path && (
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
              title="Reveal the memory file"
              onClick={() => void revealItemInDir(file.path).catch((e) => setError(String(e)))}
            >
              <FolderOpen size={11} />
            </button>
          )}
          <button className="btn-primary text-[0.625rem] py-0.5 px-2" disabled={!dirty} onClick={save}>
            Save
          </button>
        </div>
      </div>
      <p className="section-desc">
        What the character remembers across sessions; the remember tool writes here.
      </p>
      <textarea
        className="input w-full mt-2 font-mono text-[0.6875rem] leading-snug"
        rows={5}
        placeholder="Empty — facts the character saves land here."
        value={draft}
        onChange={(e) => {
          setDraft(e.target.value);
          setDirty(true);
        }}
      />
      {error && <p className="text-xs text-accent-red mt-1">{error}</p>}
    </div>
  );
}

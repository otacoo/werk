import { useEffect, useState, type ChangeEvent } from "react";
import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  Download,
  FileDown,
  FolderOpen,
  ImagePlus,
  Pencil,
  Play,
  Plus,
  Sparkles,
  Trash2,
  Upload,
  UserRound,
  X,
} from "lucide-react";
import { commands } from "../bindings";
import type {
  CardSummary,
  CharacterCard,
  DiscussionSummary,
  MemoryFileDto,
  ReasoningOptions,
} from "../bindings";
import { call } from "../utils/ipc";
import { notifyConfigChanged, notifyTalkReloaded } from "../utils/appSettings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import {
  getStreamResponses,
  setStreamResponses,
  subscribeStreamResponses,
} from "../utils/appearance";
import { useAppConfig } from "../utils/useAppConfig";
import { EFFORT_LABELS } from "./chat/external-controls";
import Toggle from "../components/Toggle";
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

/// Standard llama.cpp effort levels, used when the template declares none.
const EFFORT_CHOICES = ["minimal", "low", "medium", "high", "xhigh", "max"];
/// OpenAI-style levels for provider models in External API mode.
const PROVIDER_EFFORT_CHOICES = ["minimal", "low", "medium", "high"];

/// Levels for the effort select: the template's own when it declares any,
/// otherwise the standard set, plus a stored value the current set no longer
/// lists so it stays visible until changed. Providers use their own set.
function levelsOf(
  opts: ReasoningOptions | null,
  stored: string | null,
  external: boolean,
): string[] {
  const template = external ? [] : (opts?.levels ?? []);
  const choices = external ? PROVIDER_EFFORT_CHOICES : EFFORT_CHOICES;
  const out =
    template.length === 0
      ? [...choices]
      : [
          ...choices.filter((l) => template.includes(l)),
          ...template.filter((l) => !choices.includes(l)),
        ];
  if (stored && !out.includes(stored)) out.push(stored);
  return out;
}

type RoleplayTab = "characters" | "behavior" | "discussions";

const TABS: { id: RoleplayTab; label: string; desc: string }[] = [
  {
    id: "characters",
    label: "Characters",
    desc: "Character cards, your persona, and the opening greeting.",
  },
  {
    id: "discussions",
    label: "Discussions",
    desc: "Saved discussions for the selected character.",
  },
  {
    id: "behavior",
    label: "Behavior",
    desc: "System prompt, character memory, and generation settings.",
  },
];

export default function Roleplay({ go }: { go: (t: Tab) => void }) {
  const [appConfig, setAppConfig, , refresh] = useAppConfig(true);
  const [cards, setCards] = useState<CardSummary[]>([]);
  const [activeCard, setActiveCard] = useState<CharacterCard | null>(null);
  // Bumped after a card-prompt save to reload the card.
  const [cardVersion, setCardVersion] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reasoningOpts, setReasoningOpts] = useState<ReasoningOptions | null>(null);
  const [tab, setTab] = useState<RoleplayTab>("characters");
  const [streamLive, setStreamLive] = useState(getStreamResponses());
  useEffect(() => subscribeStreamResponses(setStreamLive), []);
  const [editingCard, setEditingCard] = useState<CharacterCard | null>(null);
  const currentTab = TABS.find((t) => t.id === tab) ?? TABS[0];

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

  const saveCardPrompt = async (prompt: string) => {
    if (!activeCard) return;
    try {
      await call(commands.roleplaySetCardPrompt(activeCard.id, prompt));
      setCardVersion((v) => v + 1);
    } catch (e) {
      setError(String(e));
      throw e;
    }
  };

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
  }, [rp.card_id, cardVersion]);

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

  const exportCard = async (card: CardSummary) => {
    setError(null);
    const safe = card.name.replace(/[\\/:*?"<>|]/g, "_");
    const ext = card.has_avatar ? "png" : "json";
    try {
      const picked = await saveDialog({
        defaultPath: `${safe}.${ext}`,
        filters: [{ name: "Character card", extensions: ["png", "json"] }],
      });
      if (!picked) return;
      await call(commands.roleplayExportCard(card.id, picked));
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

  // After importing a bundle the backend selects the new card copy; pick it up.
  const onDiscussionsImported = async () => {
    await refresh();
    loadCards();
    notifyConfigChanged();
  };

  const openEditor = (id: string) => {
    setError(null);
    call(commands.roleplayGetCard(id))
      .then(setEditingCard)
      .catch((e) => setError(String(e)));
  };

  const onCardSaved = () => {
    setEditingCard(null);
    loadCards();
    setCardVersion((v) => v + 1);
    notifyConfigChanged();
    notifyTalkReloaded();
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
        <p className="section-desc">{currentTab.desc}</p>

        {tab === "discussions" && (
          <DiscussionsCard
            cardId={rp.card_id}
            cardName={cards.find((c) => c.id === rp.card_id)?.name ?? "character"}
            onError={setError}
            onStartChat={startChat}
            onImported={onDiscussionsImported}
          />
        )}

        {tab !== "discussions" && (
        <div className="grid grid-cols-2 gap-4 items-start">
          <div className="space-y-4">
            {tab === "characters" && (
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
                    onDoubleClick={() => openEditor(card.id)}
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
                      className="btn-ghost p-1 shrink-0 text-faint hover:text-ink"
                      title="Edit this card (or double-click the row)"
                      onClick={(e) => {
                        e.stopPropagation();
                        openEditor(card.id);
                      }}
                    >
                      <Pencil size={11} />
                    </button>
                    <button
                      className="btn-ghost p-1 shrink-0 text-faint hover:text-ink"
                      title="Export this card (PNG when it has an avatar, JSON otherwise)"
                      onClick={(e) => {
                        e.stopPropagation();
                        void exportCard(card);
                      }}
                    >
                      <FileDown size={11} />
                    </button>
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
            )}

            {tab === "behavior" && (
              <SystemPromptCard
                stored={rp.system_prompt ?? ""}
                onSave={(p) => save({ system_prompt: p })}
              />
            )}
            {tab === "behavior" && <RoleplayMemoryCard cardId={rp.card_id} />}
          </div>

          <div className="space-y-4">
            {tab === "characters" && (
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
            )}

            {tab === "characters" && activeCard && (
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
                  <textarea
                    className="input w-full mt-2 font-mono text-[0.6875rem] leading-snug resize-y"
                    rows={Math.min(24, Math.max(4, greetingText.split("\n").length + 1))}
                    value={greetingText}
                    readOnly
                  />
                )}
              </div>
            )}

            {tab === "characters" && (
              <CardPromptCard card={activeCard} onSaveCardPrompt={saveCardPrompt} />
            )}

            {tab === "behavior" && (
            <div className="card">
              <h2 className="section-title mb-1">Talk</h2>
              <p className="section-desc">How Talk renders replies.</p>
              <div className="mt-3">
                <Toggle
                  label="Stream responses"
                  hint="Show the reply as tokens generate; off waits for the full message."
                  checked={streamLive}
                  onChange={setStreamResponses}
                />
              </div>
            </div>
            )}

            {tab === "behavior" && (
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
              <label className="block mt-3">
                <span className="text-[0.6875rem] text-dim">Reasoning effort</span>
                <select
                  className="input w-full mt-1"
                  value={rp.reasoning_effort ?? ""}
                  onChange={(e) => save({ reasoning_effort: e.target.value || null })}
                  title="Levels the model's chat template accepts, plus the standard llama.cpp levels"
                >
                  <option value="">Default</option>
                  {levelsOf(
                    reasoningOpts,
                    rp.reasoning_effort,
                    appConfig?.server_mode === "external",
                  ).map((level) => (
                    <option key={level} value={level}>
                      {EFFORT_LABELS[level] ?? level}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            )}
          </div>
        </div>
        )}

        {error && <p className="text-xs text-accent-red">{error}</p>}
      </div>

      {editingCard && (
        <CardEditor
          card={editingCard}
          onClose={() => setEditingCard(null)}
          onSaved={onCardSaved}
        />
      )}
    </div>
  );
}

function DiscussionsCard({
  cardId,
  cardName,
  onError,
  onStartChat,
  onImported,
}: {
  cardId: string | null;
  cardName: string;
  onError: (e: string | null) => void;
  onStartChat: () => void;
  onImported: () => void;
}) {
  const [items, setItems] = useState<DiscussionSummary[] | null>(null);
  const [busy, setBusy] = useState(false);

  const load = () => {
    if (!cardId) {
      setItems([]);
      return;
    }
    call(commands.roleplayListDiscussions())
      .then(setItems)
      .catch(() => setItems([]));
  };
  useEffect(load, [cardId]);

  const loadDiscussion = async (id: string) => {
    onError(null);
    setBusy(true);
    try {
      await call(commands.roleplayLoadDiscussion(id));
      notifyTalkReloaded();
      load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (d: DiscussionSummary) => {
    if (!window.confirm(`Delete the discussion "${d.title}"?`)) return;
    onError(null);
    setBusy(true);
    try {
      await call(commands.roleplayDeleteDiscussion(d.id));
      notifyTalkReloaded();
      load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const exportDiscussion = async (d: DiscussionSummary) => {
    onError(null);
    try {
      const picked = await saveDialog({
        defaultPath: `${cardName} - ${d.title}.werkdiscussion.json`,
        filters: [{ name: "Werk discussion", extensions: ["json"] }],
      });
      if (!picked) return;
      await call(commands.roleplayExportDiscussion(d.id, picked));
    } catch (e) {
      onError(String(e));
    }
  };

  const importDiscussion = async () => {
    onError(null);
    let picked: string | string[] | null = null;
    try {
      picked = await open({
        multiple: false,
        filters: [{ name: "Werk discussion", extensions: ["json"] }],
      });
    } catch {
      return;
    }
    if (!picked || typeof picked !== "string") return;
    setBusy(true);
    try {
      await call(commands.roleplayImportDiscussion(picked));
      onImported();
      notifyTalkReloaded();
      load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between gap-2 mb-1">
        <h2 className="section-title mb-0">Discussions</h2>
        <div className="flex items-center gap-1 shrink-0">
          <button
            className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
            onClick={importDiscussion}
            disabled={busy}
            title="Import a discussion bundle (card, memory, and transcript)"
          >
            <Download size={11} /> Import
          </button>
          <button
            className="btn-primary text-[0.625rem] py-0.5 px-2"
            onClick={onStartChat}
            disabled={!cardId || busy}
            title={
              cardId ? "Start a new discussion with the selected greeting" : "Pick a character first"
            }
          >
            <Plus size={11} /> New
          </button>
        </div>
      </div>
      <p className="section-desc">
        One row per saved discussion; the active one is the thread Talk continues. Exports carry
        the card, its memory, and the transcript.
      </p>
      {!cardId ? (
        <p className="text-[0.6875rem] text-dim mt-3">
          Pick a character on the Characters tab to see its discussions.
        </p>
      ) : items === null ? (
        <p className="text-[0.6875rem] text-dim mt-3">Loading…</p>
      ) : items.length === 0 ? (
        <p className="text-[0.6875rem] text-dim mt-3">
          No discussions yet — start one with New, or import a bundle.
        </p>
      ) : (
        <div className="space-y-1 mt-3">
          {items.map((d) => (
            <div
              key={d.id}
              className="flex items-center gap-2 border border-border rounded px-2.5 py-1.5"
            >
              <div className="flex-1 min-w-0">
                <p className="text-xs text-ink truncate" title={d.title}>
                  {d.title}
                </p>
                <p className="text-[0.625rem] text-faint truncate">
                  {d.messages} message{d.messages === 1 ? "" : "s"} ·{" "}
                  {new Date(d.updated * 1000).toLocaleString()}
                </p>
              </div>
              {d.active && <span className="badge-green text-[0.5625rem] shrink-0">active</span>}
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
                onClick={() => loadDiscussion(d.id)}
                disabled={busy || d.active}
                title={d.active ? "Already active" : "Continue this discussion in Talk"}
              >
                <Play size={10} /> Load
              </button>
              <button
                className="btn-ghost text-[0.625rem] py-0.5 px-1.5 shrink-0"
                onClick={() => exportDiscussion(d)}
                disabled={busy}
                title="Export this discussion"
              >
                <Upload size={10} />
              </button>
              <button
                className="text-faint hover:text-accent-red shrink-0"
                onClick={() => remove(d)}
                disabled={busy}
                title="Delete this discussion"
              >
                <Trash2 size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
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
  stored,
  onSave,
}: {
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

  const systemShown = stored.trim() !== "" ? stored : builtIn;
  const systemDirty = draft !== null && draft.trim() !== systemShown.trim();
  const systemSource =
    stored.trim() !== "" ? "Custom system prompt active." : "Using the built-in default.";

  return (
    <div className="card">
      <h2 className="section-title mb-1">System prompt</h2>
      <p className="section-desc">
        Always sent first, before the card prompt, character details, your persona, world info,
        and memory. Empty uses the built-in default.
      </p>
      <textarea
        className="input w-full mt-3 font-mono text-[0.6875rem] leading-snug"
        rows={8}
        value={draft ?? systemShown}
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="flex items-center gap-2 mt-2">
        <button
          className="btn-primary text-[0.625rem] py-0.5 px-2"
          disabled={!systemDirty}
          onClick={() => {
            const v = (draft ?? "").trim();
            setDraft(null);
            onSave(v === "" || v === builtIn.trim() ? null : v);
          }}
        >
          Save
        </button>
        {stored.trim() !== "" && (
          <button
            className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
            onClick={() => {
              setDraft(null);
              onSave(null);
            }}
            title="Clear the custom prompt and follow the built-in default"
          >
            Reset
          </button>
        )}
        <span className="text-[0.6875rem] text-faint ml-auto text-right">{systemSource}</span>
      </div>
    </div>
  );
}

/// The character card's own prompt, edited from the Characters tab.
function CardPromptCard({
  card,
  onSaveCardPrompt,
}: {
  card: CharacterCard | null;
  onSaveCardPrompt: (prompt: string) => Promise<void>;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    setDraft(null);
  }, [card?.id]);

  if (!card) return null;
  const shown = card.system_prompt ?? "";
  const dirty = draft !== null && draft.trim() !== shown.trim();

  const save = async () => {
    const v = (draft ?? "").trim();
    setSaving(true);
    try {
      await onSaveCardPrompt(v);
      setDraft(null);
    } catch {
      // The page surfaces the error; keep the draft so the edit is not lost.
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Card prompt</h2>
      <p className="section-desc">
        The card's own prompt, appended after the system prompt and before the character
        details.
      </p>
      <textarea
        className="input w-full mt-3 font-mono text-[0.6875rem] leading-snug"
        rows={8}
        placeholder="Empty — nothing appended."
        value={draft ?? shown}
        onChange={(e) => setDraft(e.target.value)}
      />
      <div className="flex items-center gap-2 mt-2">
        <button
          className="btn-primary text-[0.625rem] py-0.5 px-2"
          disabled={!dirty || saving}
          onClick={save}
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}

/// Floating card editor: name, description, personality, scenario, messages,
/// greetings, examples, author's note, and tags.
function CardEditor({
  card,
  onClose,
  onSaved,
}: {
  card: CharacterCard;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState({
    name: card.name,
    description: card.description,
    personality: card.personality,
    scenario: card.scenario,
    first_mes: card.first_mes,
    mes_example: card.mes_example,
    post_history_instructions: card.post_history_instructions,
    tags: card.tags.join(", "),
  });
  const [greetings, setGreetings] = useState<string[]>(card.alternate_greetings);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const field = (key: keyof typeof draft) => ({
    value: draft[key],
    onChange: (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
      setDraft((d) => ({ ...d, [key]: e.target.value })),
  });

  const save = async () => {
    setError(null);
    setSaving(true);
    try {
      await call(
        commands.roleplayUpdateCard({
          ...card,
          name: draft.name.trim(),
          description: draft.description,
          personality: draft.personality,
          scenario: draft.scenario,
          first_mes: draft.first_mes,
          mes_example: draft.mes_example,
          post_history_instructions: draft.post_history_instructions,
          tags: draft.tags.split(",").map((t) => t.trim()).filter(Boolean),
          alternate_greetings: greetings.map((g) => g.trim()).filter(Boolean),
        }),
      );
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 bg-black/60 flex items-center justify-center"
      onClick={onClose}
    >
      <div
        className="card max-w-5xl w-full mx-4 max-h-[85vh] overflow-y-auto space-y-3"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between gap-2">
          <h3 className="section-title mb-0">Edit card</h3>
          <button className="text-faint hover:text-ink" onClick={onClose} title="Close">
            <X size={14} />
          </button>
        </div>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Name</span>
          <input className="input w-full mt-1" {...field("name")} />
        </label>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Description</span>
          <textarea className="input w-full mt-1 text-xs" rows={4} {...field("description")} />
        </label>
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="text-[0.6875rem] text-dim">Personality</span>
            <textarea className="input w-full mt-1 text-xs" rows={4} {...field("personality")} />
          </label>
          <label className="block">
            <span className="text-[0.6875rem] text-dim">Scenario</span>
            <textarea className="input w-full mt-1 text-xs" rows={4} {...field("scenario")} />
          </label>
        </div>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">First message</span>
          <textarea className="input w-full mt-1 text-xs" rows={4} {...field("first_mes")} />
        </label>
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-[0.6875rem] text-dim">Alternate greetings</span>
            <button
              className="btn-ghost text-[0.625rem] py-0.5 px-1.5"
              onClick={() => setGreetings((g) => [...g, ""])}
            >
              <Plus size={10} /> Add
            </button>
          </div>
          {greetings.map((g, i) => (
            <div key={i} className="flex items-start gap-1.5">
              <textarea
                className="input flex-1 text-xs"
                rows={2}
                value={g}
                onChange={(e) =>
                  setGreetings((prev) => prev.map((x, k) => (k === i ? e.target.value : x)))
                }
              />
              <button
                className="text-faint hover:text-accent-red shrink-0 mt-1"
                title="Remove greeting"
                onClick={() => setGreetings((prev) => prev.filter((_, k) => k !== i))}
              >
                <Trash2 size={11} />
              </button>
            </div>
          ))}
        </div>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Example dialogue</span>
          <textarea className="input w-full mt-1 text-xs" rows={3} {...field("mes_example")} />
        </label>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Author's note</span>
          <textarea
            className="input w-full mt-1 text-xs"
            rows={3}
            {...field("post_history_instructions")}
          />
        </label>
        <label className="block">
          <span className="text-[0.6875rem] text-dim">Tags (comma separated)</span>
          <input className="input w-full mt-1" {...field("tags")} />
        </label>
        {error && <p className="text-xs text-accent-red">{error}</p>}
        <div className="flex items-center justify-end gap-2">
          <button className="btn-ghost text-xs py-1 px-2" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn-primary text-xs py-1 px-2"
            disabled={!draft.name.trim() || saving}
            onClick={save}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
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
              onClick={() => {
                setError(null);
                call(commands.harnessMemoryEnsure("roleplay"))
                  .then((path) => revealItemInDir(path))
                  .catch((e) => setError(String(e)));
              }}
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

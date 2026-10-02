# Changelog

## [Unreleased]

### Changed

- The agent can no longer write its own tool or skill definitions: project
  `.werk/plugins` and `.werk/skills` are read-only to the file tools, so a
  model cannot add a tool — or a no-approval plugin — to its next run.
- `exec`'s free read-only path is stricter: shell metacharacters
  (redirection, pipes, chaining, substitution) and mutating git forms
  (`git remote add`, `git branch -D`, `--output`, `--ext-diff`) now require
  approval.
- "New plugin" scaffolds into the global plugins folder (the one the Tools
  page lists) instead of the active project's `.werk/plugins/`, so it works
  without a project open.

### Fixed

- The Chat context ring shows the effective context window instead of the
  model's training maximum: the orchestrator's role override wins, then the
  launch `--ctx-size` set on the Run page, and only then the GGUF length
  (a 92K setting now reads as 92K, not 262K). Router mode no longer reads the
  router's own `/slots`/`/props` — they describe the router, not the loaded
  child; single-model mode still prefers live slot values, which include
  `--fit` shrinking.
- The system prompt no longer names disabled tools: with `remember`,
  `ask_user`, `spawn_subagent`, the file tools, or `exec` switched off, their
  sentences are dropped (or the tool list is rewritten) so the model is never
  told about a tool it cannot call. The tool schemas were already filtered.

## [0.3.7] - 2026-10-01

### Added

- Settings → Memory shows "Restore backup" when `/distill` left a
  `MEMORY.md.bak`, so a bad coalesce is one click from recovery.
- Tool authoring: "New plugin" in Tools → Agent → Plugin tools scaffolds a
  commented plugin (manifest + script) in the active project's
  `.werk/plugins/` and opens it in your editor; a plugin can also be a single
  `.json` manifest dropped in a plugins root, and each row has a button to
  open its folder. The Chat composer's pencil button edits the current draft
  in the system editor and pulls it back when the window refocuses.
- The System prompt card shows a rough token estimate for the current prompt.
- Runs start with context: date/time (UTC), git branch + short HEAD + dirty
  count, and a capped top-level listing of the project, injected into the
  system prompt.

## [0.3.6] - 2026-10-01

### Added

- Run page launch options grew: prompt-cache controls (`--cache-reuse`,
  `--cache-ram`, idle-slot caching, `--slot-save-path`), `--fit-ctx`, output
  constraints (`--json-schema[-file]`, `--grammar[-file]`), mmproj GPU
  offload, ngram speculative decoding (map-k / map-k4v modes plus per-mode
  tuning fields), `--no-kv-offload`, `--swa-full`, `--sleep-idle-seconds`,
  `--logit-bias`, and template parsing toggles (chat parsing, prefill
  assistant, preserve reasoning). Every new field stays unset by default, so
  llama.cpp's own defaults apply; the toggles show the server's current
  behavior as their checked state and only emit the flag when it changes.
- Deleting a chat session shows an Undo button in the sidebar for ~12 seconds
  — the transcript is kept in memory for the window, so a misclick is one
  click away from being reverted.
- A subagent goal that names exactly one existing image auto-attaches it when
  the subagent's model has vision and no `image:` was passed — small
  orchestrators forget the argument, and this keeps the first spawn from
  being wasted on "the image is not text" errors.
- Subagent tool cards name the model that ran them — a badge with the model
  stem on every subagent call and spawn card, so orchestrator and worker
  traffic is distinguishable at a glance.
- `spawn_subagent` can attach one local image (`image: <path>`) when the
  subagent's model has vision (a worker/main GGUF with an mmproj; provider
  models are left to decide). The file must be inside the project or the read
  allowlist, capped at 8 MiB.
- Utility model (Settings → Agent): context compaction and `/distill` run on
  a cheap provider model or a local role instead of your main model.
- `spawn_subagent` accepts a `model` choice — a favorite from External API
  mode or the local orchestrator/worker role in router mode. The allowed
  choices are listed in the prompt and validated, so a typo errors instead of
  silently using the default.
- `/distill`: summarize the session into memory, coalesce all memories with a
  short model pass (the previous file is kept as `MEMORY.md.bak`), then start
  a fresh chat — clearing the context without losing the learnings. Memory
  entries are now structured and dated (`- [YYYY-MM-DD] topic: text`), and the
  `remember` tool takes a topic tag.
- System prompt presets in Settings → Agent: quick-swap buttons (Default,
  then up to five named presets, plus a `+` to add one from the current
  prompt). Clicking a preset activates it immediately and the status line
  names it; Save updates the active preset (or the custom override).
  Double-click a preset to rename it, shift-click to delete it.

### Changed

- Sampling lists Top-P before Top-K, verbosity 3 stays the default even when
  a preset omits it, and the WebUI MCP proxy option hides (and is cleared)
  when the Web UI is off.
- Deleting a chat session moves its file to the OS recycle bin (trash on
  Linux/macOS) instead of erasing it, so even after the undo window the
  transcript stays recoverable outside the app.
- Router launches plan residency from the role models' estimated footprints:
  both stay loaded when they fit VRAM/RAM (`--models-max 2`), otherwise the
  router is capped at one (`--models-max 1`) so it swaps orchestrator ↔
  worker on subagent turns and the orchestrator reloads afterwards at full
  speed instead of both models degrading each other. An explicit
  `--models-max` in Extra args always wins. The plan is reported on the
  Memory estimate card (and the server log), not the Launch command card.
- The built-in prompt only suggests delegating to a researcher subagent in
  router mode — single and external modes have no worker model, so they no
  longer mention subagents. The file-tool rule now leads the prompt ("never
  use shell commands to read, list, search, or edit files; `exec` is for
  programs") before the shell-syntax guidance, and router-mode prompts state
  the subagent model's vision capability and the `image` argument.

### Fixed

- The memory estimate honours the orchestrator's per-role ctx and GPU-layer
  overrides in router mode — editing them previously had no effect on the
  estimate (the router-wide resource plan already counted both roles).
- Number inputs behave the same in every theme: the up/down arrows appear on
  hover/focus only — the werk theme pinned them on with a sepia tint — and
  the glyph follows each theme's color scheme. The per-role Ctx/GPU fields
  also name their flags (`--ctx-size`, `--ngl`).
- Router mode shows the Fit toggle now, and turning it off passes `--fit off`
  to the router launch (fit is already the default for children) — previously
  the flag was single-mode only while the memory estimate still reported Fit
  behavior, with no way to change it. Per-role context and GPU-layer
  overrides still win.
- Image work no longer tempts the shell: the file-tool rule now covers images
  ("you cannot view image files through shell commands or scripts"), subagent
  prompts carry the same file-tool rule as the orchestrator, `exec`'s
  description warns against file work and images, and the router prompt states
  that a path alone doesn't let a subagent see an image — only `image:` does.
- Subagent model choices are advertised as model ids, not role names — the
  orchestrator kept conflating `model` ("orchestrator"/"worker") with
  `agent_type` ("coder"/"researcher") and spawning invalid calls. The
  attached image's path is now named in the subagent's task text, so the
  worker knows which file it is looking at.
- Subagents are told to describe only what is actually visible in an attached
  image, and to say so when none is attached, instead of confabulating visual
  detail from the filename.
- Chat says "Loading model…" while the server (or a router child) loads
  instead of "Thinking…" — the loading state is now driven by the server log.

## [0.3.5] - 2026-09-30

### Added

- About section: the third-party notices (KaTeX and the other bundled
  components, with license texts) are linked and shipped with the app.

### Changed

- The default preset is always available as "Default preset": it cannot be
  deleted, the reset button restores factory defaults, saving with an empty
  name updates it, and preset actions flash Saved/Reset/Deleted feedback.
- One shared server-status poller app-wide; hidden tabs pause their polling
  (chat stats, session list, live tools), and Settings changes refresh Run
  and Chat instead of them polling the config.

### Fixed

- Reply footer stats persist across session switches (one session save per
  run, after the footer metadata is written).
- Run page: the preset and server cards sit side by side again, and the
  server card keeps its title.

## [0.3.4] - 2026-09-30

### Added

- Copy button for the server logs (Run page).
- Changed-files summary after each run: the files the agent changed with +/−
  line counts and an A/M/D list (5 shown, the rest expand). Toggle it in
  Settings → General → Chat.

### Changed

- The context popover shows this run's generation and prompt rates instead of
  the server's lifetime averages, and the live rate spans the whole run.
- The time under a reply covers the whole run (prompt, every turn, tool
  calls) instead of the last generation only.
- The built-in default preset shows as "Default preset".
- A refresh button in the Projects header re-reads the project list.

### Fixed

- Router mode: chat requests use the router's model ids instead of GGUF paths
  (fixes "model '<path>' not found").
- The exec tool no longer hangs when a command leaves a child holding its
  output pipes; timed-out commands kill the whole process tree.
- Reply footer stats survive session switches — the session is saved after
  the footer metadata is written.
- Syntax highlighting colors are no longer stripped from builds.
- The changed-files summary sees committed work too (it diffs against the
  HEAD captured at run start).

## [0.3.3] - 2026-09-30

### Added

- Gemma theme: a light sky-blue palette with white surfaces and starlight
  gold highlights, picked with a star icon in Appearance.
- Migu theme: graphite and silver surfaces with teal and neon-pink
  highlights, picked with a microphone icon in Appearance.
- Chat Files panel: toggle a project tree beside the chat (Settings →
  General → Chat, or the Files button in the chat header); markers show git
  changes on disk and the agent's edits this session, the panel is resizable,
  and double-clicking a file opens it in the OS. Seeded with ddg-search as the
  default MCP server again (only when `mcp.json` is missing; existing files
  are left alone).
- Split models (multi-part GGUF): the download dialog now asks whether to
  fetch all parts of the clicked model, and sidecar mmproj/dspark choices are
  counted over the whole set. Sibling parts are matched within the same repo
  folder, mmproj files download next to the model, and discarding a paused
  split download removes every part.

### Changed

- Quick Bench is opt-in now (off by default in Settings → General).
- The last applied preset is remembered and restored at start (router mode
  included); single mode still restores each model's own preset.
- Turning off Show file tree removes the Files button from Chat.
- Dashboard, Mode and Tools stay mounted when switching tabs, so running
  downloads and page state keep their progress instead of resetting.
- New app icon: the wordmark now sits on a graphite tile (with a matching
  favicon) so it no longer disappears on light taskbars. Regenerate with
  `tools/make-app-icon.ps1` then `npx tauri icon src-tauri/app-icon.png`.
- Resuming a split download skips parts that are already complete, and the
  progress bar spans all parts instead of resetting per file.

### Fixed

- Downloads never restart a complete file when the server answers the resume
  offset with 416 (previously re-downloaded from scratch).
- The Mode tab's Web UI card no longer nests `<p>` inside `<p>` (React DOM
  nesting warning in the console).
- Changing the HuggingFace sort order re-runs the search with the newly
  picked order (it previously reused the old one).

## [0.3.2] - 2026-09-29

### Fixed

- Fixed a deadlock that starved all background requests a few seconds after
  launch when no model was selected — the Dashboard could hang on "Loading
  system info…" and the window felt frozen.
- The Dashboard reports which load step failed (with one automatic retry)
  instead of a generic timeout message.

## [0.3.1] - 2026-09-29

### Changed

- Release builds publish from the workspace target directory, and the release
  workflow runs on Node 24 (development now requires Node >= 24).

### Fixed

- Window controls reach the screen corner again — clicking the very top-right
  closes the app instead of hitting a dead shadow margin.
- Dashboard no longer gets stuck on "Loading system info…": loads time out
  with a clear error and a Retry button, and heavy probes no longer occupy the
  request pool.
- The flaky language-server diagnostics test that failed CI is fixed.

## [0.3.0] - 2026-09-29

### Added

- External API mode: run the agent against any OpenAI-compatible endpoint
  (OpenRouter, DeepSeek, another local server) with the full harness — system
  prompt, tools, subagents, sessions — no local model needed.
- MCP support for the agent in every mode, with the same approval prompts as
  the other tools. Servers start only when one of their tools is actually
  used.
- Tools is now organized into Live Tools, Agent, Server, and MCP tabs; the
  Live tab shows what the running server and the agent currently offer.
- API tab with connection details, API key usage, and a Web UI opener;
  chat code blocks are now syntax highlighted.
- Chat model and reasoning-effort pickers in External API mode, with
  one-click switching between favorite models.

### Changed

- The modes are cleanly separated: Router loads local GGUFs only, while
  external endpoints live in their own External API mode. Server tools and
  MCP now also work in router mode.
- Run page: the launch command and server logs are separate cards (with a
  copy button), and hardware options are grouped more sensibly.
- No MCP servers ship by default — add your own under Tools → MCP.

### Fixed

- Stop reliably cancels provider generation and the next message is
  unaffected.
- Provider errors now show the real upstream reason (e.g. rate limits)
  instead of a generic message.
- The code-block copy button no longer flickers while streaming.
- Tokens/sec is hidden for external providers, where the number didn't mean
  anything.

## [0.2.0] - 2026-09-28

### Added

- Language server integration: configurable stdio servers, compile-error
  diagnostics appended to write/edit results, and a read-only `lsp` tool
  (definition, references, hover, document/workspace symbols, implementation).
- Mode tab: persisted single-model / router mode; the choice gates the server
  launch, the agent's role usage, and what Run shows.
- Agent-side settings: language servers list with add/edit/restore, LSP toggle,
  and a live "serve the loaded model" label under chat responses.
- Presence penalty launch option; KV cache K/V fields moved to the Context tab.
- Chat bubble alignment (left/right) and dimmed theme-aware placeholders.

### Changed

- Fit on now lets llama.cpp size GPU layers itself; the launch preview explains
  when layers are fit-managed.
- Memory estimates include companion files (mmproj, draft), warn on tight VRAM
  fits, and account for tool schemas in compaction budgets.
- Quick Bench lives on the Bench page; presets are remembered per model;
  subagents always available (worker model applies in router mode only).

### Fixed

- Server, llama-bench, and language-server children are killed on app exit.
- Absolute paths inside the project are accepted; paths outside stay rejected.
- Streaming aborts after a stalled server and honors Stop within seconds.
- Approval deadlock on persisted grants; bench output capture; tool cards now
  merge mid-run instead of waiting for a repaint.
- Pre-tool reasoning stays above its tool calls; footer shows the model that
  actually served the request.

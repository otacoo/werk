# Changelog

## [Unreleased]

### Added

- The assistant gets a user-picked home folder for persistent files plus an always-granted temp workspace for scratch scripts, downloads, and screenshots.
- Tool results carry images when the local model has vision, so the assistant can see screenshots and image files it reads.
- Assistant system control is opt-in from the Access tab: file tools over chosen folders plus clipboard, window, and screen-capture tools, each approval-gated.
- The assistant overlay anchors to the monitor work area and re-asserts topmost, so the taskbar no longer covers it.
- Assistant reminders: a persisted list with a reminder tool, a 30 s scheduler, cross-platform attention alerts, and catch-up for missed times.
- The Assistant gets an always-on-top pulsing overlay with a floating input, a global hotkey, tray pause/assistant entries, and an autostart toggle.
- The Assistant profile is live: a Talk-style tab with a persona, its own persistent memory, skills, and the remember/ask_user/get_time tools, running on the same server as the other profiles.
- Router roles can override the draft model, spec type (including EAGLE3 and DFlash), and draft token limits.
- Sibling draft files are detected from GGUF metadata (EAGLE3 target layers, DFlash arch, MTP nextn tensors), so drafts without telling filenames auto-attach.

## [0.6.0] - 2026-10-03

### Added

- Talk has a Reasoning button that shows or hides every reasoning trace, live and restored.
- Talk's rewind button (and /rewind) drops the last exchange, mirroring Chat.
- Talk shows a pulsing brain with "Thinking…" in the header and transcript while the model is working, and keeps the spinner for server start/load.
- Roleplay's prompt card gets System prompt / Card prompt tabs: the system prompt is always sent first, with the card prompt appended after it.
- Settings → Memory lists entries (date, topic, text) with per-entry edit and delete, plus a raw-editor toggle.
- The context ring's hover card shows session token usage (prompt in / generated out) and, when the active provider declares prices, an estimated cost.
- Providers can declare input/output prices (USD per 1M tokens) in the Mode editor.
- Roleplay is split into Characters, Discussions, and Behavior sub-tabs.
- Roleplay's Card prompt card moved to the Characters tab (below Greeting), separate from the System prompt card; the greeting preview expands vertically.
- Character cards get a floating editor (pencil button or double-click) for name, description, personality, scenario, messages, greetings, examples, author's note, and tags; the card id stays stable so discussions and memory survive edits.
- Cards export from their row as a PNG with embedded V2 data when they have an avatar, or plain JSON otherwise.
- A "Stream responses" toggle in Roleplay's Behavior tab controls whether Talk paints tokens live (on by default).
- The Discussions tab lists every saved discussion for the active character with Load, Export, and Delete; Import restores a bundle with the card (as a copy when it exists), avatar, memory, and transcript.
- Starting a new chat archives the previous discussion instead of overwriting it.
- Model downloads land under `<file>.part` and are renamed into place only when complete, so paused or aborted downloads never show up as installed models.
- Finished downloads are verified against the repo's LFS sha256 when it reports one; a mismatch removes the file and surfaces an error.
- Downloads refuse to start when the remaining bytes (plus a 256 MB reserve) do not fit the free space on the target disk.
- Memory files are structured entries now: free-form lines migrate on save, and overflow folds the oldest entries into an `earlier` entry instead of refusing new notes.
- The memory and skills folder buttons create the file/folder first, so revealing them no longer fails with "path doesn't exist".
- The built-in roleplay prompt is rewritten around explicit content rules, independent characters, and tighter style limits.
- Roleplay uses its own system prompt only — the agent's system prompt and presets never apply there.

### Fixed

- Auto-attached drafts reserve room in the fit margin (`--fit-target` = 1 GiB + draft size + 768 MiB), so MTP/EAGLE3/DFlash drafts no longer OOM against a model that already fills VRAM.
- Permission grants are deduplicated and refreshed instead of stacking duplicates, a live global grant subsumes matching project grants, and old `grants.json` files compact on startup.
- Sensitive shielding now reads nested `.gitignore` files, scoped to their directory and cached, instead of only the project root's.
- Free read-only `exec` commands no longer expand shell variables — `cat $SECRET_PATH`, `$env:` and `$VAR` reads now need approval.
- Sending with the local server stopped now locks the composer and shows "Starting the server…/Loading the model…" in Chat and Talk instead of silently accepting more messages.

## [0.5.1] - 2026-10-03

### Added

- Chat profiles: the Mode page flows from the server mode to a profile row (general/code, WebUI, roleplay), where roleplay swaps Chat for a Talk tab with character avatars, one continuous discussion per character, and distill into character memory.
- Roleplay: character cards (SillyTavern V1-V3, AICC) with per-character memory, greeting picker, sampling overrides, an editable system prompt, and a reasoning-effort picker in Generation settings.
- The WebUI profile embeds llama-server's own UI in a mounted WebUI tab.
- Talk's context ring drops down from the header and tracks the talk transcript.
- ask_user is available in roleplay, with its question box in Talk, and answer options can carry an optional description shown under the title.
- Settings gains a Debug section with a button to run the setup wizard again.

### Changed

- The Dashboard is rebuilt around a server overview (model, runtime, backend, quick actions) and a machine card (CPU, memory, GPU, backend), with Runtime/Models/Browse as plain tab cards.
- Sending starts the local server on demand; the Local server card keeps only the idle unload (the auto-start toggle is gone).
- The setup wizard is restyled: a sticky footer carries each step's action (including model downloads), the machine summary is a single spec card, and its theme cards match Settings.
- The HuggingFace browser drops its owner filter and the curated quantizer list.
- The WebUI profile keeps the server's own UI enabled automatically, hides the agent tools and LSP from the Tools page, and its card greys out in External API mode.
- The app icon is a bold lowercase w. with a blue dot, generated for every platform from assets/logo.png.
- Chat transcript bubbles and cards are a little narrower on wide windows.

### Fixed

- Talk's run lock is its own, so a second message no longer reports an agent run in progress.
- The Tools page follows mode/profile changes, and profile switches move between Chat and Talk immediately.
- Chat shortcuts on the Dashboard and Run open Talk in the roleplay profile instead of leaving the Chat page visible.
- Talk reloads the user avatar after an import or clear, without needing a restart.
- Round corners now actually apply — the theme's `--radius` token was overriding the setting.
- Cancelling a recommended-model download discards the partial file instead of leaving it to be counted as installed.
- The recommended Gemma model now points at unsloth/gemma-4-E4B-it-GGUF as "Gemma E4B IT".

## [0.5.0] - 2026-10-02

### Added

- NERV theme: black surfaces, orange ink, red alerts, and a green gradient
  sweeping along card borders — old computer graphics.
- Embedded project terminal in the Chat (icon button at the top of the
  sidebar, next to the file-tree one): commands run in the project folder
  with streamed output, a tracked `cd`, Up/Down history, a stop button, and a
  resizable panel. It is your own shell — the agent's sandbox and approvals
  do not apply.
- Local server lifecycle (Settings → Agent → "Local server"): start the
  server automatically when you send (off by default — uses the saved preset
  and selected model, and waits until it is ready), and unload it after 5
  idle minutes by default (0 disables; skipped while the Web UI is enabled).
- Mixed mode in Router mode: either role can be an external favorite (pick
  it in the role dropdown on the Run page). The provider runs that half, the
  local server loads the other, and the launch options and estimate target
  the local role. Router mode now only requires one local role; External API
  mode stays provider-only.
- Sensitive shielding: credential paths are blocked from every agent tool —
  built-in patterns (`.env*`, `*.pem`, `*.key`, `id_rsa*`, `.ssh/`, `.aws/`,
  `*.sqlite`, …), everything matched by the project's `.gitignore`, and a user
  pattern list in Settings → Agent. An allow list exempts exceptions (e.g.
  test fixtures), and built-ins always win over repo negations.
- `exec` output is scrubbed for key-shaped strings (OpenAI/GitHub/AWS/Slack
  keys, JWTs, `api_key: …` values) before it reaches the model or the session.
- The agent keeps a task list with the `todo` tool (add / complete / drop,
  max 10). It is re-injected into every request, shown as checkpoints above
  the composer (`2/5 · current item`, expandable, clearable), saved with the
  session, and never visible to subagents.
- Files panel: right-click a file or folder to open, rename, or delete it —
  delete moves it to the OS trash.

### Changed

- Themes are one CSS file each under `src/themes/` (tokens, shadows and
  syntax colors together), so a new theme is a single file plus its entry in
  the theme list.
- The Files toggle moved from the chat header to the top of the sidebar
  (icon-only, with tooltips, right-aligned and larger), and the "Show file
  tree" Settings option is gone — the panel is always available and remembers
  whether it was open.
- Removed the Chat composer's "edit the draft in your system editor" button —
  the scratch-file round-trip earned less than the space it took.
- Agent settings moved out of the Settings menu into their own header tab,
  split into Behavior / Prompt / Access / Memory sub-tabs (verification and
  turns, utility model and server lifecycle, system prompt, agent files and
  sensitive shielding, skills and memory). Settings keeps General,
  Appearance, and About.
- `exec`'s free read-only path also rejects environment reads (`$env:KEY`,
  `echo $HOME`); they need approval now.
- The built-in system prompt is ~35% shorter (1.3K chars) with the same
  rules — tighter wording, bare tool names, condensed subagent notes. A test
  caps its size so it can't creep back up.

### Fixed

- External roles no longer report local companion files (mmproj, template,
  draft model) in the launch preview — they have none.
- Compaction and `/distill` fold long sessions in bounded chunks (~20K chars
  per call, carrying a running summary) and keep the newest content when a
  transcript is capped — they now work with small utility-model windows
  instead of one large call that kept the oldest text and could overflow.
- The subagent prompt no longer reads as a contradiction: model choices and
  the default are stated in one paragraph ("the default is …"), and it notes
  that subagents never see the conversation.
- The LSP tab describes itself instead of showing the Server tab's text.

## [0.4.0] - 2026-10-02

### Added

- LSP settings got their own tab in the Tools page, after MCP.
- Settings → Agent → "Agent files": hides `AGENTS.md`/`.agent*` files from
  every agent tool (reads, writes, edits, globs, content search). Visible by
  default.
- `get_time`: current UTC date/time for date-sensitive work; the prompt
  suggests it before search-style questions.
- Settings → Memory: "Restore backup" for the pre-`/distill` `MEMORY.md.bak`.

### Changed

- The agent can no longer write its own tool or skill definitions:
  `.werk/plugins` and `.werk/skills` are read-only to the file tools.
- `exec`'s free read-only path rejects shell metacharacters (redirection,
  pipes, chains, substitution) and mutating git forms (`git remote add`,
  `git branch -D`, `--output`, `--ext-diff`).
- "New plugin" scaffolds into the global plugins folder, so it works with no
  project open.

### Fixed

- The Chat context ring shows the effective context window — role override,
  then the Run page's `--ctx-size`, then the GGUF length — instead of the
  model's training maximum; router mode no longer reads the router's own
  `/slots`/`/props`.
- The system prompt no longer names disabled tools; their sentences and the
  shell snippet drop out with the tool.

## [0.3.7] - 2026-10-01

### Added

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

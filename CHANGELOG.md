# Changelog

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

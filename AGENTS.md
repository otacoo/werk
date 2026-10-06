# Agent instructions

- Keep comments short, avoid unnecessary comments
- CHANGELOG style: one short, plain-language bullet per user-visible change; no multi-sentence explanations and no internal details. Release headings are `## YYYY-MM-DD – vX.Y.Z`; bigger releases group bullets under `### New options & features` and `### Bug fixes & others`. Bold lead-ins like `**New option:**` or `**Overlay:**` are fine; sub-bullets for a feature's details.
- After completing a set of working changes, commit them with a short, imperative message (match the existing terse style, e.g. "QuickBench", "Fix llama-bench orphans and bench state across tab switches"). Group related changes into separate commits when they are logically distinct.
- Commit but do not push.
- Run `npx tsc --noEmit` for frontend changes and `cargo test --manifest-path src-tauri/Cargo.toml` for Rust changes before committing to check for errors or warnings.

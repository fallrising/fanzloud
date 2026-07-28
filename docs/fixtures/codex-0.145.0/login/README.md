# Codex CLI 0.145.0 login fixtures

These redacted fixtures resolve `SPEC-T002`'s device-login output and exit-semantics gap for the
private P0.

Sources:

- Official npm package `@openai/codex@0.145.0`, with `codex-cli 0.145.0` verified locally.
- Official tag
  [`rust-v0.145.0` CLI login source](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/cli/src/login.rs).
- Official tag
  [`rust-v0.145.0` device-code source](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/login/src/device_code_auth.rs).
- A live device-instruction capture on 2026-07-27 using a new empty mode-`0700`
  `CODEX_HOME`. The short-lived code was not authorized, was not persisted in this repository, and
  is represented by the shape-preserving value `A1B2-3456C`.

The live capture confirmed that the prompt is written to stdout while the process remains active,
the verification URL is `https://auth.openai.com/codex/device`, and the code shape is
`[A-Z0-9]{4}-[A-Z0-9]{5}`. ANSI SGR color bytes are intentionally normalized out of the committed
fixture. The pinned source establishes success/failure and status exit mappings.

These are exact-version compatibility fixtures, not a claim of a stable upstream machine protocol.
No token, account identifier, credential cache, or usable device code appears here.

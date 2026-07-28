# Codex Cloud `0.145.0` source-derived fixtures

These synthetic fixtures encode the exact shapes emitted by the official
`rust-v0.145.0` `cloud-tasks` source. They are not live task captures and contain no credential,
account identifier, private repository, or real user prompt.

- `exec.stdout`: default browser task URL.
- `status.*.stdout`: the four status templates; exit semantics are recorded in
  `exit-semantics.json`.
- `list.stdout.json`: exact `cloud list --json` schema.
- `diff.stdout`: bounded synthetic unified diff display payload.

Changing the pinned CLI version requires source review, fixture regeneration, and a specification
change.

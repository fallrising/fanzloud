import test from "node:test";

const contractSkeletons = [
  "p0_web_bootstrap_token_is_ephemeral_and_never_persisted",
  "p0_web_login_status_and_device_actions_use_exact_api_contract",
  "p0_web_prompt_submission_requires_one_explicit_operator_action",
  "p0_web_stream_replays_and_reconnects_from_validated_cursor",
  "p0_web_cancel_is_explicit_and_disconnect_never_cancels",
  "p0_web_diff_is_bounded_text_and_never_html",
  "p0_web_refresh_rehydrates_identity_without_replaying_mutations",
  "p0_web_errors_and_diagnostics_exclude_sensitive_canaries",
  "p0_web_exposes_no_execution_provider_or_arbitrary_route_authority",
  "p0_web_controller_model_preserves_generation_sequence_and_e0_boundaries",
];

for (const name of contractSkeletons) {
  test(name, { skip: "T006 contract skeleton" }, () => {});
}

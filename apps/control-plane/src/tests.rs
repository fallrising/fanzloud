macro_rules! contract_skeleton {
    ($name:ident) => {
        #[test]
        #[ignore = "T005B contract skeleton"]
        fn $name() {}
    };
}

contract_skeleton!(p0_http_bootstrap_sets_secure_host_cookie_and_redacts_secret);
contract_skeleton!(p0_http_rejects_missing_cookie_and_wrong_origin_before_handler);
contract_skeleton!(p0_http_login_status_and_device_code_are_exact_and_bounded);
contract_skeleton!(p0_http_device_code_never_enters_events_errors_or_logs);
contract_skeleton!(p0_http_start_turn_validates_prompt_and_returns_accepted_receipt);
contract_skeleton!(p0_http_mutations_require_current_instance_and_idempotency_key);
contract_skeleton!(p0_http_same_key_replays_once_and_different_request_conflicts);
contract_skeleton!(p0_http_concurrent_same_key_joins_in_flight_response);
contract_skeleton!(p0_http_cancel_is_explicit_and_disconnect_is_not_cancel);
contract_skeleton!(p0_http_reconcile_does_not_resolve_or_retry);
contract_skeleton!(p0_http_abandon_requires_exact_true_ack_and_current_operation);
contract_skeleton!(p0_http_adopt_rejects_unlisted_or_stale_task);
contract_skeleton!(p0_http_diff_is_plain_bounded_untrusted_and_not_cached);
contract_skeleton!(p0_http_bounds_content_type_and_error_schema_fail_closed);
contract_skeleton!(p0_http_logout_invalidates_only_current_app_session);
contract_skeleton!(p0_http_forbids_browser_provider_and_host_configuration);
contract_skeleton!(p0_http_canaries_are_absent_from_debug_and_nonsecret_responses);
contract_skeleton!(p0_http_session_expiry_capacity_and_cookie_comparison_are_bounded);
contract_skeleton!(p0_http_shutdown_drains_handlers_and_cleans_lower_runtime);

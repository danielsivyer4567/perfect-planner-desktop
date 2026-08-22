@{
    SchemaVersion = 1
    Stages = @(
        @{
            Name = 'Registry'
            Step = 'Machine-wide registry admission and persistence'
            Hazards = @('incomplete coverage', 'malformed state', 'alias swap', 'lost concurrent update', 'post-scan plan race')
            QuickTests = @(
                'collision_assessor::registry::tests::missing_malformed_partial_unsupported_duplicate_and_stale_are_unknown'
                'collision_assessor::registry::tests::incomplete_root_census_never_becomes_complete'
                'collision_assessor::registry::tests::physical_alias_swaps_change_or_invalidate_the_attestation'
            )
            FullTests = @(
                'collision_assessor::registry::tests::conditional_census_write_rejects_mutation_and_duplicate_races'
                'collision_assessor::registry::tests::capability_expiry_while_waiting_for_registry_lock_prevents_acceptance'
                'collision_assessor::registry::tests::extra_plan_created_after_child_scan_is_rejected_before_persistence'
                'collision_assessor::registry::tests::extra_plan_created_after_persistence_makes_restart_inspection_unknown'
                'collision_assessor::registry::tests::startup_migrates_stale_v1_once_and_never_rewrites_malformed_legacy'
            )
        }
        @{
            Name = 'Discovery'
            Step = 'Single-use read-only discovery capability'
            Hazards = @('token replay', 'nonce rebinding', 'truncated frame', 'oversized response', 'unregistered plan')
            QuickTests = @(
                'collision_assessor::discovery::tests::replayed_or_rebound_response_nonce_is_rejected'
                'collision_assessor::discovery::tests::missing_malformed_duplicate_node_and_duplicate_manifest_are_unknown'
                'collision_assessor::discovery::tests::unregistered_extra_plan_in_fixed_directory_is_unknown'
            )
            FullTests = @(
                'collision_assessor::discovery::tests::frames_reject_trailing_truncated_unknown_and_oversized_input'
                'collision_assessor::discovery::tests::strict_request_rejects_unknown_fields_and_nonce_rebinding'
                'collision_assessor::discovery::tests::nested_or_sibling_configured_root_is_unknown'
            )
        }
        @{
            Name = 'NativeCensus'
            Step = 'Native collector boundary and one-time persistence'
            Hazards = @('concurrent replay', 'expiry during collection', 'explicit revocation', 'identity drift', 'detail leakage')
            QuickTests = @(
                'collision_assessor::api::tests::concurrent_replay_runs_exactly_one_native_collection'
                'collision_assessor::api::tests::expiry_during_collection_is_rechecked_and_never_persisted'
                'collision_assessor::api::tests::invalid_or_unknown_token_causes_zero_registry_or_collector_reads'
            )
            FullTests = @(
                'collision_assessor::api::tests::explicit_revoke_during_collection_cancels_native_boundary_and_discards_result'
                'collision_assessor::api::tests::conditional_persistence_failure_revokes_without_leaking_detail'
                'collision_assessor::api::tests::same_generation_native_identity_drift_revokes_before_persistence'
                'collision_assessor::api::tests::failure_codes_do_not_serialize_native_details'
            )
        }
        @{
            Name = 'CanonicalClaims'
            Step = 'Canonical claim and signed disposition construction'
            Hazards = @('unsupported glob', 'unsigned mutation', 'subset laundering', 'issuer rotation', 'dependency tombstone loss')
            QuickTests = @(
                'collision_assessor::registry::tests::unsupported_glob_remains_registered_but_snapshot_is_unknown'
                'collision_assessor::registry::tests::signed_publisher_rejects_every_unapproved_contract_mutation'
                'collision_assessor::registry::tests::owner_pin_rejects_same_generation_subset_and_no_receipt_tamper_laundering'
            )
            FullTests = @(
                'collision_assessor::registry::tests::atomic_authority_set_is_exact_owner_pinned_and_transition_safe'
                'collision_assessor::registry::tests::issuer_rotation_rejects_old_receipts_and_is_atomic_on_native_failure'
                'collision_assessor::registry::tests::terminal_dependency_tombstone_releases_claims_without_erasing_dependency_identity'
            )
        }
        @{
            Name = 'CollisionGraph'
            Step = 'Pure deterministic collision analysis'
            Hazards = @('input permutation', 'false path prefix', 'duplicate participant', 'policy contradiction', 'work-budget overflow')
            QuickTests = @(
                'collision_assessor::analyzer::tests::every_three_planner_and_root_permutation_has_identical_ids_graph_and_verdict'
                'collision_assessor::analyzer::tests::complete_current_coverage_is_required_before_clear'
                'collision_assessor::analyzer::tests::duplicate_participant_claim_and_contract_inputs_are_unknown'
            )
            FullTests = @(
                'collision_assessor::analyzer::tests::one_hundred_mixed_participants_use_indexes_and_false_prefixes_do_not_collide'
                'collision_assessor::analyzer::tests::hot_key_fanout_fails_unknown_instead_of_quadratic_or_truncated_output'
                'collision_assessor::analyzer::tests::contradictory_and_mixed_policy_never_hide_the_known_graph'
                'collision_assessor::analyzer::tests::conflict_ids_are_domain_separated_basis_bound_and_order_independent'
            )
        }
        @{
            Name = 'Snapshot'
            Step = 'Immutable assessment and conflict commitment'
            Hazards = @('hidden conflict', 'field splice', 'proof truncation', 'participant reorder', 'different bytes at same key')
            QuickTests = @(
                'collision_assessor::snapshot::tests::clear_snapshot_cannot_hide_a_conflict'
                'collision_assessor::snapshot::tests::tamper_expiry_and_participant_order_invalidate_the_hash_or_shape'
                'collision_assessor::snapshot::tests::conflict_commitment_proves_every_exact_field_and_rejects_splices'
            )
            FullTests = @(
                'collision_assessor::snapshot::tests::non_power_of_two_commitments_reject_truncated_extra_and_flipped_proofs'
                'collision_assessor::snapshot::tests::self_rehashed_participant_binding_and_dependency_cycle_are_rejected'
                'collision_assessor::snapshot::tests::immutable_store_round_trips_and_rejects_different_existing_bytes'
            )
        }
        @{
            Name = 'Clearance'
            Step = 'Single-use clearance issue, consume and revoke'
            Hazards = @('parallel replay', 'exact-boundary expiry', 'binding drift', 'invalid MAC', 'issuer restart')
            QuickTests = @(
                'collision_assessor::clearance::tests::one_hundred_simultaneous_replays_have_exactly_one_winner'
                'collision_assessor::clearance::tests::exact_expiry_is_denied_and_durably_revoked'
                'collision_assessor::clearance::tests::every_clearance_binding_field_rejects_drift'
            )
            FullTests = @(
                'collision_assessor::clearance::tests::invalid_mac_neither_consumes_nor_appends'
                'collision_assessor::clearance::tests::snapshot_loss_before_consumption_revokes_instead_of_admitting'
                'collision_assessor::clearance::tests::clearance_from_prior_issuer_epoch_is_denied_before_state_change'
                'collision_assessor::clearance::tests::consuming_clearance_can_be_revoked_and_never_replayed'
            )
        }
        @{
            Name = 'Tickets'
            Step = 'Owner-scoped conflict-ticket messaging'
            Hazards = @('production enablement bypass', 'cross-owner access', 'route expiry', 'duplicate signal', 'unauthorized acknowledgement')
            QuickTests = @(
                'collision_assessor::tickets::tests::broker_is_production_disabled_and_has_no_generic_control_plane_dependency'
                'collision_assessor::tickets::tests::capability_is_owner_scoped_epoch_bound_and_not_a_ticket_lookup_api'
                'collision_assessor::tickets::tests::signal_and_ack_are_exactly_idempotent_and_survive_restart'
            )
            FullTests = @(
                'collision_assessor::tickets::tests::minted_mailbox_cannot_outlive_or_mutate_its_route_authority'
                'collision_assessor::tickets::tests::non_neighbor_cannot_acknowledge_a_signal_it_could_not_receive'
                'collision_assessor::tickets::tests::one_manifest_transition_atomically_reaches_every_conflicting_peer'
                'collision_assessor::tickets::tests::one_hundred_parallel_signal_retries_append_exactly_once'
                'collision_assessor::tickets::tests::ticket_index_cache_is_bounded_and_eviction_does_not_mutate_issued_capability'
            )
            StressTests = @(
                'collision_assessor::tickets::tests::one_hundred_parallel_signal_retries_append_exactly_once'
            )
        }
        @{
            Name = 'Journal'
            Step = 'Append-only audit durability and restart recovery'
            Hazards = @('concurrent writer loss', 'torn tail', 'middle corruption', 'rollback', 'unanchored restart')
            QuickTests = @(
                'collision_assessor::journal::tests::concurrent_appenders_produce_one_complete_hash_chain'
                'collision_assessor::journal::tests::malformed_torn_tail_is_truncated_but_middle_corruption_blocks_append'
                'collision_assessor::journal::tests::live_high_water_rejects_full_file_rollback'
            )
            FullTests = @(
                'collision_assessor::journal::tests::damage_to_a_committed_final_newline_is_not_silently_repaired'
                'collision_assessor::journal::tests::missing_or_zeroed_history_after_restart_never_becomes_authoritative'
                'collision_assessor::journal::tests::restarted_history_is_unanchored_and_cannot_authorize_clearance'
                'collision_assessor::journal::tests::snapshot_revocation_between_consuming_and_consumed_blocks_commit'
                'collision_assessor::journal::tests::assessment_and_clearance_timestamps_stay_inside_snapshot_window'
            )
            StressTests = @(
                'collision_assessor::journal::tests::concurrent_appenders_produce_one_complete_hash_chain'
            )
        }
        @{
            Name = 'Scale'
            Step = 'Bounded high-contention behavior'
            Hazards = @('quadratic fanout', 'duplicate delivery', 'unbounded ticket materialization', 'maximum assessment overflow')
            QuickTests = @(
                'collision_assessor::tickets::tests::one_hundred_distinct_participants_publish_and_ack_owned_edges_in_parallel'
                'collision_assessor::journal::tests::maximum_participant_and_unique_edge_assessment_fits_and_restarts'
            )
            FullTests = @(
                'collision_assessor::tickets::tests::one_hundred_agent_complete_conflict_graph_stays_bounded_and_fans_out_once'
                'collision_assessor::tickets::tests::maximum_ticket_set_materializes_lazily_and_journals_only_active_delivery'
                'collision_assessor::analyzer::tests::sealed_analyzer_entry_handles_a_ten_thousand_participant_chain'
            )
            StressTests = @(
                'collision_assessor::tickets::tests::one_hundred_distinct_participants_publish_and_ack_owned_edges_in_parallel'
                'collision_assessor::journal::tests::maximum_participant_and_unique_edge_assessment_fits_and_restarts'
                'collision_assessor::tickets::tests::one_hundred_agent_complete_conflict_graph_stays_bounded_and_fans_out_once'
            )
        }
    )
}

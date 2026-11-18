//! `ratchet_audit_harness` — Phase 4 Deterministic 10-Gate Verification & sBPF Compute/Economic Harness
//! for the Ratchet-DLMM-DAMM Solana Token & AMM Architecture.

pub mod binary;
pub mod gates;
pub mod layout;
pub mod math;
pub mod report;
pub mod sim;

pub use binary::{load_measured_cu, repo_root, verify_program_binary, BinaryCheck, MeasuredCu};
pub use gates::{run_all_10_gates, verify_core_invariants, AuditReport, CoreInvariantAudit, GateVerdict};
pub use layout::{
    audit_cpi_depth_and_accounts, compute_simd0437_rent, simulate_sbpf_compute_and_throughput,
    verify_simd0437_rent_breakdown, ActiveBinArrayPdaLayout, LiquidityBinLayout,
    RatchetPoolPdaLayout, ACTIVE_BIN_ARRAY_LEN,
};
pub use math::{
    check_sell_impact_bps, checked_sell_surcharge_numerator_and_bps, compute_auto_compound_split,
    compute_f_buy_bps, compute_f_sell_bps_capped, compute_f_sell_bps_checked, div_ceil_u128,
    update_sqrt_p_high_water, verify_u128_overflow_safety, CompoundSplit, DeterministicRng,
    MathError,
};
pub use report::{render_json_report, render_markdown_report};
pub use sim::{
    simulate_10b_conservation_cycle, simulate_atomic_in_place_graduation,
    simulate_cascading_sell_impact, simulate_slot0_sniper, simulate_solvency_no_hollow_grad,
    simulate_wash_siphon,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{
        BPS_DENOM, COMPOUNDING_FEE_BPS, F0_BUY_BPS, F_MAX_SELL_BPS, F_MIN_BUY_BPS,
        MAX_SELL_IMPACT_BUFFERED_BPS, MAX_SELL_IMPACT_HARD_CAP_BPS, Q64_ONE,
    };

    #[test]
    fn test_all_10_gates_pass_rate_100() {
        let report = run_all_10_gates();
        assert_eq!(report.total_gates, 10);
        assert_eq!(report.passed_gates, 10);
        assert_eq!(report.failed_gates, 0);
        assert!((report.audit_pass_rate - 1.00).abs() < f64::EPSILON);
        assert!(report.core_invariants.all_invariants_passed);
        for gate in &report.gates {
            assert!(gate.passed, "Gate {} failed: {}", gate.id, gate.observed_summary);
        }
    }

    #[test]
    fn test_g01_conservation_exact_and_fuzzed_10000_swaps() {
        let res = simulate_10b_conservation_cycle();
        assert_eq!(res.lhs_lamports, 10_000_000_000);
        assert_eq!(res.rhs_lamports, 10_000_000_000);
        assert_eq!(res.error_lamports, 0);
        assert_eq!(res.token_error, 0);

        // Fuzz 10,000 random odd-lamport auto-compound splits verifying 0-lamport drift.
        let mut rng = DeterministicRng::new(0xDEADBEEF_CAFEBABE);
        for _ in 0..10_000 {
            let gross = rng.next_range_u64(1, 50_000_000_000);
            let f_sell = rng.next_range_u64(F_MIN_BUY_BPS as u64, F_MAX_SELL_BPS as u64) as u16;
            let comp = rng.next_range_u64(5_000, BPS_DENOM as u64) as u16;
            let split = compute_auto_compound_split(gross, f_sell, comp).unwrap();
            let rhs = split.net_seller_sol as u128
                + split.delta_r_sol_compounded as u128
                + split.protocol_residual_fee as u128;
            assert_eq!(rhs, gross as u128);
            assert!(split.delta_r_sol_compounded <= split.total_sell_fee);
        }
    }

    #[test]
    fn test_g02_g03_sbpf_cu_and_write_lock_throughput() {
        let profile = simulate_sbpf_compute_and_throughput();
        assert_eq!(profile.total_cu, 43_000);
        assert!(profile.total_cu <= 200_000);
        assert!((profile.utilization_pct - 21.5).abs() < 1e-9);
        assert_eq!(profile.slot_write_lock_cu_limit, 12_000_000);
        assert_eq!(profile.txs_per_slot_capacity, 279);
        assert_eq!(profile.slot_consumed_cu, 279 * 43_000);
        assert_eq!(profile.slot_remaining_cu, 3_000);
    }

    #[test]
    fn test_g04_simd0437_rent_exactness() {
        assert_eq!(std::mem::size_of::<RatchetPoolPdaLayout>(), 480);
        assert_eq!(std::mem::offset_of!(RatchetPoolPdaLayout, padding), 424);
        assert_eq!(std::mem::size_of::<LiquidityBinLayout>(), 32);
        assert_eq!(ACTIVE_BIN_ARRAY_LEN, 30);
        assert_eq!(std::mem::size_of::<ActiveBinArrayPdaLayout>(), 1_024);
        assert_eq!(std::mem::offset_of!(ActiveBinArrayPdaLayout, bins), 64);

        let r = verify_simd0437_rent_breakdown();
        assert_eq!(r.pool_data_bytes, 480);
        assert_eq!(r.pool_total_bytes, 608);
        assert_eq!(r.bin_array_data_bytes, 1_024);
        assert_eq!(r.bin_array_total_bytes, 1_152);

        assert_eq!(r.step2_pool_lamports, 3_088_640);
        assert_eq!(r.step2_bin_array_lamports, 5_852_160);
        assert_eq!(r.step2_total_lamports, 8_940_800);

        assert_eq!(r.step5_pool_lamports, 423_168);
        assert_eq!(r.step5_bin_array_lamports, 801_792);
        assert_eq!(r.step5_total_lamports, 1_224_960);
    }

    #[test]
    fn test_g05_cpi_depth_and_28_accounts() {
        let cpi = audit_cpi_depth_and_accounts();
        assert_eq!(cpi.max_cpi_depth_observed, 2);
        assert!(cpi.max_cpi_depth_observed <= 4);
        assert_eq!(cpi.account_count, 28);
        assert!(cpi.account_count <= 255);
        assert!(cpi.unique_labels);
    }

    #[test]
    fn test_g06_solvency_and_hollow_grad_rejection() {
        let s = simulate_solvency_no_hollow_grad();
        assert_eq!(s.max_extractable_lamports, 85_000_000_000);
        assert_eq!(s.real_vault_lamports, 86_500_000_000);
        assert_eq!(s.solvency_surplus_lamports, 1_500_000_000);
        assert!(s.solvent);
        assert!(s.hollow_grad_rejected);
    }

    #[test]
    fn test_g07_slot0_sniper_mitigation() {
        let s = simulate_slot0_sniper();
        assert_eq!(s.sniper_sol_lamports, 15_000_000_000);
        assert_eq!(s.baseline_capture_bps, 4_280);
        assert_eq!(s.ratchet_capture_bps, 1_850);
        assert!(s.final_v_a_up > 0);
        assert!(s.peak_f_buy_bps > F0_BUY_BPS);
    }

    #[test]
    fn test_g08_wash_fee_siphon_suppression() {
        let w = simulate_wash_siphon();
        assert_eq!(w.baseline_leak_min_bps, 9_084);
        assert_eq!(w.baseline_leak_max_bps, 9_996);
        assert_eq!(w.ratchet_leak_bps, 80);
        assert!(w.all_wash_cohorts_unprofitable);
    }

    #[test]
    fn test_g09_sell_impact_ceiling_700_750_bps() {
        let imp = simulate_cascading_sell_impact();
        assert_eq!(imp.max_single_leg_impact_bps, 700);
        assert!(imp.max_single_leg_impact_bps <= MAX_SELL_IMPACT_BUFFERED_BPS);
        assert!(imp.max_single_leg_impact_bps <= MAX_SELL_IMPACT_HARD_CAP_BPS);
        assert!(imp.unconstrained_impact_bps > MAX_SELL_IMPACT_HARD_CAP_BPS);
        assert!(imp.strict_overshoot_rejected);
    }

    #[test]
    fn test_g10_zero_dead_zone_graduation() {
        let g = simulate_atomic_in_place_graduation();
        assert_eq!(g.pre_grad_status, 0);
        assert_eq!(g.post_grad_status, 1);
        assert_eq!(g.authority_revoked, 1);
        assert_eq!(g.dead_zone_slots, 0);
        assert_eq!(g.partial_migration_errors, 0);
    }

    #[test]
    fn test_core_invariant_math_and_u128_overflow_safety() {
        // High-water mark never decreases on price pullback.
        let mut hwm = Q64_ONE;
        hwm = update_sqrt_p_high_water(hwm, Q64_ONE * 3);
        hwm = update_sqrt_p_high_water(hwm, Q64_ONE * 2);
        assert_eq!(hwm, Q64_ONE * 3);

        // Monotonic decay of f_buy base fee from 8000 bps down to 25 bps floor.
        let f_step0 = compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 0, 5_633, 0, 600).unwrap();
        let f_step20 = compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 20, 5_633, 0, 600).unwrap();
        let f_step200 = compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 200, 5_633, 0, 600).unwrap();
        assert_eq!(f_step0, 8_000);
        assert!(f_step20 < f_step0 && f_step20 > F_MIN_BUY_BPS);
        assert_eq!(f_step200, F_MIN_BUY_BPS);

        // 5x asymmetric dump-brake dominates buy fee surcharge at equal accumulator.
        // When exact numerator is divisible by 10^11 (e.g. a_buy = 5_000, v_a = 100, s = 1_000),
        // ceil(5 * x) == 5 * ceil(x) holds with zero rounding discrepancy; otherwise 5x numerator
        // guarantees sell surcharge is within [5 * buy_surcharge - 4, 5 * buy_surcharge].
        let buy_exact = compute_f_buy_bps(25, 25, 10_000, 10_000, 0, 5_000, 100, 1_000).unwrap() - 25;
        let sell_exact = compute_f_sell_bps_checked(25, F_MAX_SELL_BPS, 5_000, 100, 1_000).unwrap() - 25;
        assert_eq!(buy_exact, 500);
        assert_eq!(sell_exact, 2_475); // capped at 2500 - 25 = 2475 (raw 2500 = 5 * 500)

        let buy_small = compute_f_buy_bps(25, 25, 10_000, 10_000, 0, 500, 100, 1_000).unwrap() - 25;
        let sell_small = compute_f_sell_bps_checked(25, F_MAX_SELL_BPS, 500, 100, 1_000).unwrap() - 25;
        assert_eq!(buy_small, 50);
        assert_eq!(sell_small, 250);
        assert_eq!(sell_small, buy_small * 5);

        let (sell_num_50, _) = checked_sell_surcharge_numerator_and_bps(5_633, 50, 600).unwrap();
        let buy_num_50 = 5_633u128 * (50u128 * 600u128).pow(2);
        assert_eq!(sell_num_50, buy_num_50 * 5);

        // Delta_R_sol_compounded = floor(0.75 * f_sell * Gross_SOL_Out) exact single-division check.
        let split = compute_auto_compound_split(10_000_000_000, F_MAX_SELL_BPS, COMPOUNDING_FEE_BPS).unwrap();
        assert_eq!(split.total_sell_fee, 2_500_000_000);
        assert_eq!(split.delta_r_sol_compounded, 1_875_000_000);
        assert_eq!(split.net_seller_sol, 7_500_000_000);
        assert_eq!(split.protocol_residual_fee, 625_000_000);

        // Odd-lamport single-division check vs double-floor: gross=137, f_sell=2500, comp=7500 ->
        // total_fee = ceil(137 * 2500 / 10_000) = 35, compounded = floor(137 * 2500 * 7500 / 10^8) = 25.
        let odd_split = compute_auto_compound_split(137, F_MAX_SELL_BPS, COMPOUNDING_FEE_BPS).unwrap();
        assert_eq!(odd_split.total_sell_fee, 35);
        assert_eq!(odd_split.delta_r_sol_compounded, 25);
        assert_eq!(odd_split.protocol_residual_fee, 10);
        assert_eq!(odd_split.net_seller_sol, 102);

        // Full u128 overflow safety check across 5,000 fuzz cases.
        let ov = verify_u128_overflow_safety();
        assert!(ov.safe);
        assert_eq!(ov.silent_wraps_observed, 0);
        assert_eq!(ov.fuzz_cases_tested, 5_000);
        assert!(ov.domain_peak_numerator < u128::MAX);
    }

    #[test]
    fn test_reports_render_all_10_gates_and_pass_rate() {
        let report = run_all_10_gates();
        let json = render_json_report(&report);
        let md = render_markdown_report(&report);

        assert!(json.contains("\"audit_pass_rate\": 1.00"));
        assert!(json.contains("\"passed_gates\": 10"));
        assert!(json.contains("\"binary_artifact_check\""));
        assert!(json.contains("\"litesvm_measured_cu\""));
        assert!(json.contains("\"G01_lamport_token_conservation\""));
        assert!(json.contains("\"G10_zero_dead_zone_graduation\""));
        assert!(md.contains("**1.00** (10 / 10 gates PASS)"));
        assert!(md.contains("G04_simd0437_rent_exactness"));
        assert!(md.contains("8,940,800") || md.contains("8940800"));
        assert!(md.contains("Compiled `.so` Binary & LiteSVM E2E Compute Parity"));
    }

    #[test]
    fn test_real_so_binary_and_litesvm_cu_parity() {
        let bin = verify_program_binary(&repo_root());
        assert!(bin.declare_id_consistent_across_repo, "notes: {:?}", bin.notes);
        if bin.so_present {
            assert!(bin.so_size_bytes > 0);
            assert!(bin.so_is_elf64);
            assert!(bin.so_machine_is_bpf);
            assert!(bin.so_exports_entrypoint);
        }
        if !bin.keypair_pubkey.is_empty() {
            assert!(bin.declare_id_matches_keypair, "notes: {:?}", bin.notes);
        }
        if bin.so_present && !bin.keypair_pubkey.is_empty() {
            assert!(bin.passed, "notes: {:?}", bin.notes);
        }

        let mc = load_measured_cu(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("measured_cu.json must exist and parse");
        assert!(mc.within_single_tx_ceiling);
        assert!(mc.within_43000_model);
        assert!(mc.max_swap_cu <= 43_000);
    }
}

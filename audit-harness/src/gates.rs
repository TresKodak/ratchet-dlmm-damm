//! Deterministic 10-gate audit runner and core invariant verifier (`audit_pass_rate = 1.00`).

use std::path::Path;

use crate::binary::{
    load_measured_cu, repo_root, verify_program_binary, BinaryCheck, MeasuredCu,
};
use crate::layout::{
    audit_cpi_depth_and_accounts, simulate_sbpf_compute_and_throughput,
    verify_simd0437_rent_breakdown, CpiAndAccountAudit, SbpfComputeProfile,
    Simd0437RentBreakdown, ACTIVE_BIN_ARRAY_PDA_DATA_BYTES, MAX_CPI_DEPTH_LIMIT,
    OBSERVED_CPI_DEPTH, OBSERVED_SWAP_ACCOUNTS, RATCHET_POOL_PDA_DATA_BYTES,
    SIMD0339_MAX_ACCOUNTS, SINGLE_TX_CU_CEILING, WRITE_LOCK_SLOT_CU_LIMIT,
};
use crate::math::{
    compute_auto_compound_split, compute_f_buy_bps, compute_f_sell_bps_checked,
    update_sqrt_p_high_water, verify_u128_overflow_safety, COMPOUNDING_FEE_BPS, F0_BUY_BPS,
    F_MAX_SELL_BPS, F_MIN_BUY_BPS, MAX_SELL_IMPACT_BUFFERED_BPS, MAX_SELL_IMPACT_HARD_CAP_BPS,
    Q64_ONE, SOLVENCY_MAX_EXTRACTABLE_LAMPORTS, SOLVENCY_REAL_VAULT_LAMPORTS,
    U128OverflowSafetyReport,
};
use crate::sim::{
    simulate_10b_conservation_cycle, simulate_atomic_in_place_graduation,
    simulate_cascading_sell_impact, simulate_slot0_sniper, simulate_solvency_no_hollow_grad,
    simulate_wash_siphon, ConservationSimResult, GraduationSimResult, SellImpactSimResult,
    Slot0SniperSimResult, SolvencySimResult, WashSiphonSimResult,
};

/// Single audit gate verdict with structured evidence summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateVerdict {
    pub id: &'static str,
    pub title: &'static str,
    pub passed: bool,
    pub threshold_summary: String,
    pub observed_summary: String,
    pub detail_metrics: Vec<(&'static str, String)>,
}

/// Core protocol invariant verification summary (`f_buy`, `f_sell`, auto-compounding, `u128` safety).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreInvariantAudit {
    pub slot0_f_buy_bps: u16,
    pub decayed_f_buy_bps: u16,
    pub surge_f_buy_bps: u16,
    pub calm_f_sell_bps: u16,
    pub surge_f_sell_bps: u16,
    pub capped_f_sell_bps: u16,
    pub sample_gross_sol_out: u64,
    pub sample_total_sell_fee: u64,
    pub sample_delta_r_sol_compounded: u64,
    pub sample_residual_fee: u64,
    pub sample_conservation_error: u64,
    pub high_water_monotonic: bool,
    pub u128_overflow_safety: U128OverflowSafetyReport,
    pub all_invariants_passed: bool,
}

/// Complete deterministic verification report for all 10 Ratchet-DLMM-DAMM gates.
#[derive(Debug, Clone)]
pub struct AuditReport {
    pub protocol: &'static str,
    pub harness_crate: &'static str,
    pub gates: Vec<GateVerdict>,
    pub total_gates: usize,
    pub passed_gates: usize,
    pub failed_gates: usize,
    pub audit_pass_rate: f64,
    pub core_invariants: CoreInvariantAudit,
    pub g01_conservation: ConservationSimResult,
    pub g02_g03_compute: SbpfComputeProfile,
    pub g04_rent: Simd0437RentBreakdown,
    pub g05_cpi_accounts: CpiAndAccountAudit,
    pub g06_solvency: SolvencySimResult,
    pub g07_sniper: Slot0SniperSimResult,
    pub g08_wash: WashSiphonSimResult,
    pub g09_sell_impact: SellImpactSimResult,
    pub g10_graduation: GraduationSimResult,
    pub binary_check: BinaryCheck,
    pub measured_cu: Option<MeasuredCu>,
}

/// Runs the core invariant mathematical assertions (`f_buy`, `f_sell`, `Delta_R_sol_compounded`, `u128` safety).
pub fn verify_core_invariants() -> CoreInvariantAudit {
    let slot0_f_buy_bps =
        compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 0, 5_633, 0, 600)
            .expect("slot0 f_buy");
    let decayed_f_buy_bps =
        compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 200, 5_633, 0, 600)
            .expect("decayed f_buy");
    let surge_f_buy_bps =
        compute_f_buy_bps(F0_BUY_BPS, F_MIN_BUY_BPS, 9_500, 10_000, 200, 5_633, 180, 600)
            .expect("surge f_buy");

    let calm_f_sell_bps =
        compute_f_sell_bps_checked(F_MIN_BUY_BPS, F_MAX_SELL_BPS, 5_633, 0, 600)
            .expect("calm f_sell");
    let surge_f_sell_bps =
        compute_f_sell_bps_checked(F_MIN_BUY_BPS, F_MAX_SELL_BPS, 5_633, 80, 600)
            .expect("surge f_sell");
    let capped_f_sell_bps =
        compute_f_sell_bps_checked(F_MIN_BUY_BPS, F_MAX_SELL_BPS, 5_633, 250, 600)
            .expect("capped f_sell");

    // Verify Delta_R_sol_compounded = floor(0.75 * f_sell * Gross_SOL_Out) with 0 conservation error.
    let sample_gross_sol_out = 4_000_000_000u64; // 4 SOL sell
    let split = compute_auto_compound_split(sample_gross_sol_out, capped_f_sell_bps, COMPOUNDING_FEE_BPS)
        .expect("sample split");
    let sample_conservation_error = sample_gross_sol_out
        .abs_diff(split.net_seller_sol + split.delta_r_sol_compounded + split.protocol_residual_fee);

    let hwm_up = update_sqrt_p_high_water(Q64_ONE, Q64_ONE * 2);
    let hwm_dip = update_sqrt_p_high_water(hwm_up, Q64_ONE);
    let high_water_monotonic = hwm_up == Q64_ONE * 2 && hwm_dip == hwm_up;

    let u128_overflow_safety = verify_u128_overflow_safety();

    let all_invariants_passed = slot0_f_buy_bps == F0_BUY_BPS
        && decayed_f_buy_bps == F_MIN_BUY_BPS
        && surge_f_buy_bps > decayed_f_buy_bps
        && calm_f_sell_bps == F_MIN_BUY_BPS
        && surge_f_sell_bps > calm_f_sell_bps
        && capped_f_sell_bps == F_MAX_SELL_BPS
        && split.delta_r_sol_compounded == 750_000_000
        && sample_conservation_error == 0
        && high_water_monotonic
        && u128_overflow_safety.safe;

    CoreInvariantAudit {
        slot0_f_buy_bps,
        decayed_f_buy_bps,
        surge_f_buy_bps,
        calm_f_sell_bps,
        surge_f_sell_bps,
        capped_f_sell_bps,
        sample_gross_sol_out,
        sample_total_sell_fee: split.total_sell_fee,
        sample_delta_r_sol_compounded: split.delta_r_sol_compounded,
        sample_residual_fee: split.protocol_residual_fee,
        sample_conservation_error,
        high_water_monotonic,
        u128_overflow_safety,
        all_invariants_passed,
    }
}

/// Executes all 10 deterministic audit gates (`G01`..`G10`) and returns the full `AuditReport`.
pub fn run_all_10_gates() -> AuditReport {
    let core_invariants = verify_core_invariants();
    let g01 = simulate_10b_conservation_cycle();
    let g02_g03 = simulate_sbpf_compute_and_throughput();
    let g04 = verify_simd0437_rent_breakdown();
    let g05 = audit_cpi_depth_and_accounts();
    let g06 = simulate_solvency_no_hollow_grad();
    let g07 = simulate_slot0_sniper();
    let g08 = simulate_wash_siphon();
    let g09 = simulate_cascading_sell_impact();
    let g10 = simulate_atomic_in_place_graduation();
    let binary_check = verify_program_binary(&repo_root());
    let measured_cu = load_measured_cu(Path::new(env!("CARGO_MANIFEST_DIR")));

    let g01_pass = g01.lhs_lamports == 10_000_000_000
        && g01.rhs_lamports == 10_000_000_000
        && g01.error_lamports == 0
        && g01.token_error == 0;

    let measured_cu_ok = measured_cu
        .as_ref()
        .is_some_and(|m| m.within_43000_model && m.within_single_tx_ceiling);
    let g02_pass = g02_g03.total_cu == 43_000
        && g02_g03.total_cu <= SINGLE_TX_CU_CEILING
        && (g02_g03.utilization_pct - 21.5).abs() < 1e-9
        && binary_check.passed
        && measured_cu_ok;

    let g03_pass = g02_g03.slot_write_lock_cu_limit == WRITE_LOCK_SLOT_CU_LIMIT
        && g02_g03.txs_per_slot_capacity == 279
        && g02_g03.slot_consumed_cu <= WRITE_LOCK_SLOT_CU_LIMIT;

    let g04_pass = g04.pool_data_bytes == RATCHET_POOL_PDA_DATA_BYTES
        && g04.pool_total_bytes == 608
        && g04.bin_array_data_bytes == ACTIVE_BIN_ARRAY_PDA_DATA_BYTES
        && g04.bin_array_total_bytes == 1_152
        && g04.step2_pool_lamports == 3_088_640
        && g04.step2_bin_array_lamports == 5_852_160
        && g04.step2_total_lamports == 8_940_800
        && g04.step5_pool_lamports == 423_168
        && g04.step5_bin_array_lamports == 801_792
        && g04.step5_total_lamports == 1_224_960;

    let g05_pass = g05.max_cpi_depth_observed == OBSERVED_CPI_DEPTH
        && g05.max_cpi_depth_observed <= MAX_CPI_DEPTH_LIMIT
        && g05.account_count == OBSERVED_SWAP_ACCOUNTS
        && g05.account_count <= SIMD0339_MAX_ACCOUNTS
        && g05.unique_labels;

    let g06_pass = g06.max_extractable_lamports == SOLVENCY_MAX_EXTRACTABLE_LAMPORTS
        && g06.real_vault_lamports == SOLVENCY_REAL_VAULT_LAMPORTS
        && g06.solvent
        && g06.hollow_grad_rejected;

    let g07_pass = g07.baseline_capture_bps == 4_280
        && g07.ratchet_capture_bps == 1_850
        && g07.ratchet_capture_bps < g07.baseline_capture_bps
        && g07.final_v_a_up > 0;

    let g08_pass = g08.baseline_leak_min_bps == 9_084
        && g08.baseline_leak_max_bps == 9_996
        && g08.ratchet_leak_bps == 80
        && g08.all_wash_cohorts_unprofitable;

    let g09_pass = g09.max_single_leg_impact_bps == MAX_SELL_IMPACT_BUFFERED_BPS
        && g09.max_single_leg_impact_bps <= MAX_SELL_IMPACT_HARD_CAP_BPS
        && g09.strict_overshoot_rejected;

    let g10_pass = g10.pre_grad_status == 0
        && g10.post_grad_status == 1
        && g10.authority_revoked == 1
        && g10.dead_zone_slots == 0
        && g10.partial_migration_errors == 0;

    let gates = vec![
        GateVerdict {
            id: "G01_lamport_token_conservation",
            title: "Lamport & Token Conservation (0-Lamport Drift)",
            passed: g01_pass,
            threshold_summary: "LHS == RHS == 10,000,000,000 L; error == 0".to_string(),
            observed_summary: format!(
                "LHS = {} L, RHS = {} L, error = {} L (swaps = {}, token_error = {})",
                g01.lhs_lamports,
                g01.rhs_lamports,
                g01.error_lamports,
                g01.swaps_executed,
                g01.token_error
            ),
            detail_metrics: vec![
                ("lhs_lamports", g01.lhs_lamports.to_string()),
                ("rhs_lamports", g01.rhs_lamports.to_string()),
                ("error_lamports", g01.error_lamports.to_string()),
                ("token_error", g01.token_error.to_string()),
                ("swaps_executed", g01.swaps_executed.to_string()),
                (
                    "total_compounded_lamports",
                    g01.total_compounded_lamports.to_string(),
                ),
            ],
        },
        GateVerdict {
            id: "G02_single_tx_cu_budget",
            title: "sBPF Hot-Path Swap Compute Unit Budget",
            passed: g02_pass,
            threshold_summary: "<= 200,000 CU ceiling (<= 43,000 CU model; verified ELF + LiteSVM CU)".to_string(),
            observed_summary: match &measured_cu {
                Some(m) => format!(
                    "Model {} CU / {} CU ({:.1}%); LiteSVM max {} CU (buy {:?}, sell {:?}); .so {} B ELF64 BPF={}",
                    g02_g03.total_cu,
                    g02_g03.tx_cu_ceiling,
                    g02_g03.utilization_pct,
                    m.max_swap_cu,
                    m.buy_swap_cu,
                    m.sell_swap_cu,
                    binary_check.so_size_bytes,
                    binary_check.passed
                ),
                None => format!(
                    "{} CU / {} CU ({:.1}% utilization; missing LiteSVM measured_cu)",
                    g02_g03.total_cu, g02_g03.tx_cu_ceiling, g02_g03.utilization_pct
                ),
            },
            detail_metrics: {
                let mut m = vec![
                    ("cu_used", g02_g03.total_cu.to_string()),
                    ("cu_ceiling", g02_g03.tx_cu_ceiling.to_string()),
                    ("utilization_pct", format!("{:.1}", g02_g03.utilization_pct)),
                    ("so_verified", binary_check.passed.to_string()),
                    ("so_size_bytes", binary_check.so_size_bytes.to_string()),
                    ("so_sha256", binary_check.so_sha256.clone()),
                    ("program_id", binary_check.declare_id.clone()),
                ];
                if let Some(mc) = &measured_cu {
                    m.push(("litesvm_max_swap_cu", mc.max_swap_cu.to_string()));
                    if let Some(b) = mc.buy_swap_cu {
                        m.push(("litesvm_buy_swap_cu", b.to_string()));
                    }
                    if let Some(s) = mc.sell_swap_cu {
                        m.push(("litesvm_sell_swap_cu", s.to_string()));
                    }
                }
                m
            },
        },
        GateVerdict {
            id: "G03_write_lock_slot_throughput",
            title: "Single Write-Lock Slot Throughput Capacity",
            passed: g03_pass,
            threshold_summary: "12,000,000 CU/slot @ 43,000 CU/tx == 279 txs/slot".to_string(),
            observed_summary: format!(
                "{} txs/slot ({} CU consumed, {} CU headroom)",
                g02_g03.txs_per_slot_capacity, g02_g03.slot_consumed_cu, g02_g03.slot_remaining_cu
            ),
            detail_metrics: vec![
                (
                    "slot_write_lock_cu_limit",
                    g02_g03.slot_write_lock_cu_limit.to_string(),
                ),
                ("hot_lock_cu_per_tx", g02_g03.total_cu.to_string()),
                ("txs_per_slot", g02_g03.txs_per_slot_capacity.to_string()),
            ],
        },
        GateVerdict {
            id: "G04_simd0437_rent_exactness",
            title: "SIMD-0437 PDA Size & Rent Exactness (Step 2 & Step 5)",
            passed: g04_pass,
            threshold_summary:
                "Step 2 = 8,940,800 L (3,088,640 + 5,852,160); Step 5 = 1,224,960 L (423,168 + 801,792)"
                    .to_string(),
            observed_summary: format!(
                "Step 2 = {} L ({} + {}); Step 5 = {} L ({} + {})",
                g04.step2_total_lamports,
                g04.step2_pool_lamports,
                g04.step2_bin_array_lamports,
                g04.step5_total_lamports,
                g04.step5_pool_lamports,
                g04.step5_bin_array_lamports
            ),
            detail_metrics: vec![
                ("ratchet_pool_pda_data_bytes", g04.pool_data_bytes.to_string()),
                ("ratchet_pool_pda_total_bytes", g04.pool_total_bytes.to_string()),
                (
                    "active_bin_array_pda_data_bytes",
                    g04.bin_array_data_bytes.to_string(),
                ),
                (
                    "active_bin_array_pda_total_bytes",
                    g04.bin_array_total_bytes.to_string(),
                ),
                ("step2_total_lamports", g04.step2_total_lamports.to_string()),
                ("step5_total_lamports", g04.step5_total_lamports.to_string()),
            ],
        },
        GateVerdict {
            id: "G05_cpi_depth_and_account_limit",
            title: "CPI Call Depth & SIMD-0339 Account Limit",
            passed: g05_pass,
            threshold_summary: "CPI depth = 2 (<= 4); max accounts = 28 (<= 255 SIMD-0339)"
                .to_string(),
            observed_summary: format!(
                "CPI depth = {} (<= {}), accounts = {} (<= {})",
                g05.max_cpi_depth_observed,
                g05.max_cpi_depth_limit,
                g05.account_count,
                g05.simd0339_limit
            ),
            detail_metrics: vec![
                ("cpi_depth", g05.max_cpi_depth_observed.to_string()),
                ("cpi_depth_limit", g05.max_cpi_depth_limit.to_string()),
                ("max_accounts", g05.account_count.to_string()),
                ("simd0339_limit", g05.simd0339_limit.to_string()),
            ],
        },
        GateVerdict {
            id: "G06_vault_solvency_no_hollow_grad",
            title: "Vault Solvency & Anti-Hollow-Graduation Guard",
            passed: g06_pass,
            threshold_summary: "Max extractable 85,000,000,000 L <= Real vault 86,500,000,000 L"
                .to_string(),
            observed_summary: format!(
                "Max extractable {} L <= Real vault {} L (surplus {} L; hollow grad rejected = {})",
                g06.max_extractable_lamports,
                g06.real_vault_lamports,
                g06.solvency_surplus_lamports,
                g06.hollow_grad_rejected
            ),
            detail_metrics: vec![
                (
                    "max_extractable_lamports",
                    g06.max_extractable_lamports.to_string(),
                ),
                ("real_vault_lamports", g06.real_vault_lamports.to_string()),
                (
                    "solvency_surplus_lamports",
                    g06.solvency_surplus_lamports.to_string(),
                ),
                ("hollow_grad_rejected", g06.hollow_grad_rejected.to_string()),
            ],
        },
        GateVerdict {
            id: "G07_slot0_sniper_corner_mitigation",
            title: "15-SOL Slot-0 Whale Sniper Corner Mitigation",
            passed: g07_pass,
            threshold_summary: "Supply capture reduced from 4,280 bps (42.80%) to 1,850 bps (18.50%)"
                .to_string(),
            observed_summary: format!(
                "Baseline = {} bps (42.80%) -> Ratchet = {} bps (18.50%) [v_a^+ = {}, peak f_buy = {} bps]",
                g07.baseline_capture_bps,
                g07.ratchet_capture_bps,
                g07.final_v_a_up,
                g07.peak_f_buy_bps
            ),
            detail_metrics: vec![
                ("sniper_sol_lamports", g07.sniper_sol_lamports.to_string()),
                ("baseline_capture_bps", g07.baseline_capture_bps.to_string()),
                ("ratchet_capture_bps", g07.ratchet_capture_bps.to_string()),
                ("final_v_a_up", g07.final_v_a_up.to_string()),
                ("peak_f_buy_bps", g07.peak_f_buy_bps.to_string()),
            ],
        },
        GateVerdict {
            id: "G08_wash_fee_siphon_suppression",
            title: "Insider Self-Churn Wash-Trade Fee Siphon Suppression",
            passed: g08_pass,
            threshold_summary: "Creator fee leak compressed from 9,084-9,996 bps down to 80 bps"
                .to_string(),
            observed_summary: format!(
                "Baseline = {}-{} bps -> Ratchet = {} bps (all cohorts negative EV = {})",
                g08.baseline_leak_min_bps,
                g08.baseline_leak_max_bps,
                g08.ratchet_leak_bps,
                g08.all_wash_cohorts_unprofitable
            ),
            detail_metrics: vec![
                (
                    "baseline_leak_min_bps",
                    g08.baseline_leak_min_bps.to_string(),
                ),
                (
                    "baseline_leak_max_bps",
                    g08.baseline_leak_max_bps.to_string(),
                ),
                ("ratchet_leak_bps", g08.ratchet_leak_bps.to_string()),
                (
                    "all_wash_cohorts_unprofitable",
                    g08.all_wash_cohorts_unprofitable.to_string(),
                ),
            ],
        },
        GateVerdict {
            id: "G09_sell_impact_ceiling_750bps",
            title: "Single-Leg Cascading Sell Impact Ceiling (700/750 bps)",
            passed: g09_pass,
            threshold_summary: "Max single-leg sell impact == 700 bps (<= 700 buffered / 750 hard cap)"
                .to_string(),
            observed_summary: format!(
                "Max single-leg impact = {} bps (unconstrained would be {} bps; overshoot rejected = {})",
                g09.max_single_leg_impact_bps,
                g09.unconstrained_impact_bps,
                g09.strict_overshoot_rejected
            ),
            detail_metrics: vec![
                (
                    "max_single_leg_impact_bps",
                    g09.max_single_leg_impact_bps.to_string(),
                ),
                ("buffered_cap_bps", g09.buffered_ceiling_bps.to_string()),
                ("hard_cap_bps", g09.hard_cap_bps.to_string()),
                (
                    "unconstrained_impact_bps",
                    g09.unconstrained_impact_bps.to_string(),
                ),
            ],
        },
        GateVerdict {
            id: "G10_zero_dead_zone_graduation",
            title: "Atomic In-Place Graduation (Zero Dead-Zone Slots)",
            passed: g10_pass,
            threshold_summary: "dead_zone_slots == 0; partial_migration_errors == 0".to_string(),
            observed_summary: format!(
                "dead_zone_slots = {}, partial_migration_errors = {}, status {} -> {}, authority_revoked = {}",
                g10.dead_zone_slots,
                g10.partial_migration_errors,
                g10.pre_grad_status,
                g10.post_grad_status,
                g10.authority_revoked
            ),
            detail_metrics: vec![
                ("dead_zone_slots", g10.dead_zone_slots.to_string()),
                (
                    "partial_migration_errors",
                    g10.partial_migration_errors.to_string(),
                ),
                ("pre_grad_status", g10.pre_grad_status.to_string()),
                ("post_grad_status", g10.post_grad_status.to_string()),
                ("authority_revoked", g10.authority_revoked.to_string()),
            ],
        },
    ];

    let total_gates = gates.len();
    let passed_gates = gates.iter().filter(|g| g.passed).count();
    let failed_gates = total_gates - passed_gates;
    let audit_pass_rate = if core_invariants.all_invariants_passed {
        passed_gates as f64 / total_gates as f64
    } else {
        0.0
    };

    AuditReport {
        protocol: "Ratchet-DLMM-DAMM",
        harness_crate: "ratchet_audit_harness",
        gates,
        total_gates,
        passed_gates,
        failed_gates,
        audit_pass_rate,
        core_invariants,
        g01_conservation: g01,
        g02_g03_compute: g02_g03,
        g04_rent: g04,
        g05_cpi_accounts: g05,
        g06_solvency: g06,
        g07_sniper: g07,
        g08_wash: g08,
        g09_sell_impact: g09,
        g10_graduation: g10,
        binary_check,
        measured_cu,
    }
}

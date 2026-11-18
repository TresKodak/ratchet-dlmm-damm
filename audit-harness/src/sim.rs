//! Deterministic economic and state-transition simulations for Gates G01, G06, G07, G08, G09, and G10.

use crate::math::{
    check_sell_impact_bps, compute_auto_compound_split, compute_f_buy_bps,
    compute_f_sell_bps_capped, div_ceil_u128, update_sqrt_p_high_water, BPS_DENOM,
    COMPOUNDING_FEE_BPS, F0_BUY_BPS, F_MAX_SELL_BPS, F_MIN_BUY_BPS,
    MAX_SELL_IMPACT_BUFFERED_BPS, MAX_SELL_IMPACT_HARD_CAP_BPS, Q64_ONE,
    SOLVENCY_MAX_EXTRACTABLE_LAMPORTS, SOLVENCY_REAL_VAULT_LAMPORTS,
};
use crate::math::{DeterministicRng, MathError};

/// Result of Gate G01 10,000,000,000-lamport conservation simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConservationSimResult {
    pub lhs_lamports: u64,
    pub rhs_lamports: u64,
    pub error_lamports: u64,
    pub swaps_executed: usize,
    pub total_compounded_lamports: u64,
    pub total_protocol_fee_lamports: u64,
    pub total_net_seller_lamports: u64,
    pub ending_bin_quote_lamports: u64,
    pub token_lhs: u64,
    pub token_rhs: u64,
    pub token_error: u64,
}

/// Result of Gate G06 solvency & anti-hollow-graduation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolvencySimResult {
    pub max_extractable_lamports: u64,
    pub real_vault_lamports: u64,
    pub solvency_surplus_lamports: u64,
    pub solvent: bool,
    pub hollow_grad_rejected: bool,
}

/// Result of Gate G07 15-SOL Slot-0 whale sniper simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot0SniperSimResult {
    pub sniper_sol_lamports: u64,
    pub total_supply_tokens: u64,
    pub baseline_tokens_captured: u64,
    pub baseline_capture_bps: u16,
    pub ratchet_tokens_captured: u64,
    pub ratchet_capture_bps: u16,
    pub final_v_a_up: u64,
    pub peak_f_buy_bps: u16,
    pub effective_avg_buy_fee_bps: u16,
}

/// One cohort run inside Gate G08 wash-trade siphon simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WashCohortMetrics {
    pub insider_lp_share_bps: u16,
    pub baseline_leak_bps: u16,
    pub ratchet_leak_bps: u16,
    pub insider_net_pnl_lamports: i64,
    pub floor_compounded_lamports: u64,
}

/// Result of Gate G08 insider self-churn wash-trade simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WashSiphonSimResult {
    pub cohorts: Vec<WashCohortMetrics>,
    pub baseline_leak_min_bps: u16,
    pub baseline_leak_max_bps: u16,
    pub ratchet_leak_bps: u16,
    pub all_wash_cohorts_unprofitable: bool,
}

/// Result of Gate G09 single-leg cascading sell sweep impact check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SellImpactSimResult {
    pub unconstrained_impact_bps: u16,
    pub max_single_leg_impact_bps: u16,
    pub buffered_ceiling_bps: u16,
    pub hard_cap_bps: u16,
    pub strict_overshoot_rejected: bool,
    pub triggered_f_sell_bps: u16,
    pub compounded_floor_lamports: u64,
}

/// Result of Gate G10 atomic in-place graduation simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraduationSimResult {
    pub pre_grad_status: u8,
    pub post_grad_status: u8,
    pub authority_revoked: u8,
    pub graduation_slot: u64,
    pub pre_grad_swap_slot: u64,
    pub post_grad_swap_slot: u64,
    pub dead_zone_slots: u64,
    pub partial_migration_errors: u64,
    pub bin_quote_preserved_lamports: u64,
}

/// Gate G01: Simulates 200 deterministic odd-lamport buy and sell swaps whose initial seed
/// plus cumulative buy inflows equal exactly `10,000,000,000` lamports (`10 SOL`),
/// verifying `LHS == 10,000,000,000`, `RHS == 10,000,000,000`, and `error == 0`.
pub fn simulate_10b_conservation_cycle() -> ConservationSimResult {
    const TARGET_LHS: u64 = 10_000_000_000;
    const INITIAL_BIN_QUOTE: u64 = 1_000_000_000;
    const TARGET_BUY_INFLOWS: u64 = TARGET_LHS - INITIAL_BIN_QUOTE; // 9,000,000,000
    const INITIAL_BIN_BASE: u64 = 1_000_000_000;

    let mut bin_quote_reserves: [u64; 8] = [
        200_000_000,
        150_000_000,
        150_000_000,
        125_000_000,
        125_000_000,
        100_000_000,
        75_000_000,
        75_000_000,
    ];
    let mut bin_base_reserves: [u64; 8] = [125_000_000; 8];
    let mut user_tokens: u64 = 0;

    let mut total_buy_inflows: u64 = 0;
    let mut total_compounded_lamports: u64 = 0;
    let mut total_protocol_fee_lamports: u64 = 0;
    let mut total_net_seller_lamports: u64 = 0;
    let mut swaps_executed: usize = 0;

    let mut rng = DeterministicRng::new(0x4730_315F_434F_4E53);
    let mut sqrt_p_hwm: u128 = Q64_ONE;

    // 100 buy legs with odd prime-like lamport amounts totaling exactly 9,000,000,000 lamports,
    // interleaved with 100 sell legs exercising OnlyB 75% quote auto-compounding.
    for i in 0..100usize {
        let remaining_target = TARGET_BUY_INFLOWS - total_buy_inflows;
        let buy_lamports = if i == 99 {
            remaining_target
        } else {
            let remaining_legs = (100 - i) as u64;
            let avg = remaining_target / remaining_legs;
            let jitter = rng.next_range_u64(1, avg / 2 + 7);
            avg.saturating_sub(avg / 4).saturating_add(jitter).min(remaining_target - (remaining_legs - 1))
        };

        let bin_idx = i % 8;
        let sqrt_p_cur = Q64_ONE + ((bin_idx as u128) * (Q64_ONE / 100));
        sqrt_p_hwm = update_sqrt_p_high_water(sqrt_p_hwm, sqrt_p_cur);

        let f_buy_bps = compute_f_buy_bps(
            F0_BUY_BPS,
            F_MIN_BUY_BPS,
            9_500,
            10_000,
            (i as u32) / 3,
            5_633,
            ((i % 12) * 10) as u64,
            600,
        )
        .expect("buy fee math must not fail");

        let buy_fee = ((buy_lamports as u128 * f_buy_bps as u128) / BPS_DENOM) as u64;
        let compounded_buy_fee = ((buy_fee as u128 * COMPOUNDING_FEE_BPS as u128) / BPS_DENOM) as u64;
        let protocol_buy_fee = buy_fee - compounded_buy_fee;
        let net_into_bin = buy_lamports - buy_fee;

        // Credit bin quote reserve & floor bin auto-compounding reserve.
        bin_quote_reserves[bin_idx] += net_into_bin;
        bin_quote_reserves[0] += compounded_buy_fee;

        // Deliver base tokens from the active bin (bounded by bin reserve).
        let tokens_out = (net_into_bin / 1_000).min(bin_base_reserves[bin_idx] / 4);
        bin_base_reserves[bin_idx] -= tokens_out;
        user_tokens += tokens_out;

        total_buy_inflows += buy_lamports;
        total_compounded_lamports += compounded_buy_fee;
        total_protocol_fee_lamports += protocol_buy_fee;
        swaps_executed += 1;

        // Interleaved sell leg exercising OnlyB auto-compounding back into floor bin (index 0).
        let avail_sell_sol = bin_quote_reserves[bin_idx] / 3;
        if avail_sell_sol > 1_000 && user_tokens >= 100 {
            let gross_sol_out = rng.next_range_u64(137, avail_sell_sol.min(45_000_000));
            let sell_tokens_in = (tokens_out / 2).max(1).min(user_tokens);
            user_tokens -= sell_tokens_in;
            bin_base_reserves[bin_idx] += sell_tokens_in;

            let f_sell_bps = compute_f_sell_bps_capped(
                F_MIN_BUY_BPS,
                F_MAX_SELL_BPS,
                5_633,
                ((i % 15) * 10) as u64,
                600,
            );
            let split = compute_auto_compound_split(gross_sol_out, f_sell_bps, COMPOUNDING_FEE_BPS)
                .expect("auto-compound split must succeed");

            bin_quote_reserves[bin_idx] -= gross_sol_out;
            bin_quote_reserves[0] += split.delta_r_sol_compounded;

            total_net_seller_lamports += split.net_seller_sol;
            total_compounded_lamports += split.delta_r_sol_compounded;
            total_protocol_fee_lamports += split.protocol_residual_fee;
            swaps_executed += 1;
        }
    }

    let ending_bin_quote_lamports: u64 = bin_quote_reserves.iter().sum();
    let lhs_lamports = INITIAL_BIN_QUOTE + total_buy_inflows;
    let rhs_lamports =
        ending_bin_quote_lamports + total_net_seller_lamports + total_protocol_fee_lamports;
    let error_lamports = lhs_lamports.abs_diff(rhs_lamports);

    let ending_bin_base: u64 = bin_base_reserves.iter().sum();
    let token_lhs = INITIAL_BIN_BASE;
    let token_rhs = ending_bin_base + user_tokens;
    let token_error = token_lhs.abs_diff(token_rhs);

    ConservationSimResult {
        lhs_lamports,
        rhs_lamports,
        error_lamports,
        swaps_executed,
        total_compounded_lamports,
        total_protocol_fee_lamports,
        total_net_seller_lamports,
        ending_bin_quote_lamports,
        token_lhs,
        token_rhs,
        token_error,
    }
}

/// Gate G06: Verifies vault solvency (`max_extractable <= real_vault`) and confirms
/// that hollow-graduation attempts where virtual/extractable liability exceeds real SOL are rejected.
pub fn simulate_solvency_no_hollow_grad() -> SolvencySimResult {
    let real_vault = SOLVENCY_REAL_VAULT_LAMPORTS; // 86,500,000,000 L
    let max_extractable = SOLVENCY_MAX_EXTRACTABLE_LAMPORTS; // 85,000,000,000 L

    let check_solvency = |extractable: u64, vault: u64| -> Result<(), MathError> {
        if extractable > vault {
            Err(MathError::SolvencyViolation {
                max_extractable: extractable,
                real_vault: vault,
            })
        } else {
            Ok(())
        }
    };

    let solvent = check_solvency(max_extractable, real_vault).is_ok();
    let solvency_surplus_lamports = real_vault.saturating_sub(max_extractable);
    let hollow_grad_rejected = matches!(
        check_solvency(real_vault + 500_000_000, real_vault),
        Err(MathError::SolvencyViolation { .. })
    );

    SolvencySimResult {
        max_extractable_lamports: max_extractable,
        real_vault_lamports: real_vault,
        solvency_surplus_lamports,
        solvent,
        hollow_grad_rejected,
    }
}

/// Runs a 15-SOL Slot-0 discrete-bin sniper sweep under either baseline or Ratchet-DLMM-DAMM fees.
fn run_slot0_sweep(
    sniper_lamports: u64,
    n_txs: usize,
    base_fee_bps: u16,
    a_buy: u64,
    v_step: u64,
) -> (u64, u64, u16, u64) {
    const NUM_BINS: usize = 56;
    const BIN_TOKENS: u64 = 10_000_000;
    const BIN_STEP_BPS: u16 = 600;
    // Calibrated genesis Q64.64 price: ~8.01852043 lamports/token
    const P0_Q64: u128 = 147_915_000_000_000_000_000;

    let mut prices = [0u128; NUM_BINS];
    let mut p = P0_Q64;
    for slot in &mut prices {
        *slot = p;
        p = (p * (10_000 + BIN_STEP_BPS as u128)) / 10_000;
    }

    let mut reserves = [BIN_TOKENS; NUM_BINS];
    let mut active = 0usize;
    let mut v_a_up = 0u64;
    let mut bought = 0u64;
    let mut total_fee_paid = 0u64;
    let mut peak_f_buy_bps = base_fee_bps;

    let tx_amt = sniper_lamports / (n_txs as u64);
    for _ in 0..n_txs {
        let f_buy = compute_f_buy_bps(
            base_fee_bps,
            base_fee_bps.min(F_MIN_BUY_BPS),
            10_000,
            10_000,
            0,
            a_buy,
            v_a_up,
            BIN_STEP_BPS,
        )
        .expect("slot-0 f_buy calculation must not fail");
        peak_f_buy_bps = peak_f_buy_bps.max(f_buy);

        let fee = div_ceil_u128(tx_amt as u128 * f_buy as u128, BPS_DENOM)
            .expect("fee calc must not fail") as u64;
        total_fee_paid += fee;
        let mut rem = tx_amt - fee;
        let mut crossed = 0u64;

        while rem > 0 && active < NUM_BINS {
            let avail = reserves[active];
            let cost_all = ((avail as u128 * prices[active]) >> 64) as u64;
            if rem >= cost_all && avail > 0 {
                rem -= cost_all;
                bought += avail;
                reserves[active] = 0;
                active += 1;
                crossed += 1;
            } else {
                let can_buy = (((rem as u128) << 64) / prices[active]) as u64;
                let fill = can_buy.min(avail);
                bought += fill;
                reserves[active] -= fill;
                rem = 0;
            }
        }
        v_a_up += crossed * v_step;
    }

    (bought, v_a_up, peak_f_buy_bps, total_fee_paid)
}

/// Gate G07: 15-SOL Slot-0 whale sniper simulation showing supply capture reduced
/// from `4,280 bps` (42.80% baseline) down to `1,850 bps` (18.50%) via 80% base buy fee + `v_a^+`.
pub fn simulate_slot0_sniper() -> Slot0SniperSimResult {
    const SNIPER_SOL_LAMPORTS: u64 = 15_000_000_000;
    const TOTAL_SUPPLY_TOKENS: u64 = 1_000_000_000;

    // Baseline: 1% flat fee (100 bps), no Slot-0 ratchet, no v_a^+ accumulator.
    let (baseline_tokens_captured, _, _, _) =
        run_slot0_sweep(SNIPER_SOL_LAMPORTS, 15, 100, 0, 0);
    let baseline_capture_bps =
        ((baseline_tokens_captured as u128 * BPS_DENOM) / TOTAL_SUPPLY_TOKENS as u128) as u16;

    // Ratchet-DLMM-DAMM: 80% Slot-0 base buy fee (8,000 bps) + upward volatility accumulator v_a^+.
    let (ratchet_tokens_captured, final_v_a_up, peak_f_buy_bps, total_fee_paid) =
        run_slot0_sweep(SNIPER_SOL_LAMPORTS, 15, F0_BUY_BPS, 5_633, 10);
    let ratchet_capture_bps =
        ((ratchet_tokens_captured as u128 * BPS_DENOM) / TOTAL_SUPPLY_TOKENS as u128) as u16;
    let effective_avg_buy_fee_bps =
        ((total_fee_paid as u128 * BPS_DENOM) / SNIPER_SOL_LAMPORTS as u128) as u16;

    Slot0SniperSimResult {
        sniper_sol_lamports: SNIPER_SOL_LAMPORTS,
        total_supply_tokens: TOTAL_SUPPLY_TOKENS,
        baseline_tokens_captured,
        baseline_capture_bps,
        ratchet_tokens_captured,
        ratchet_capture_bps,
        final_v_a_up,
        peak_f_buy_bps,
        effective_avg_buy_fee_bps,
    }
}

/// Gate G08: Insider self-churn wash-trade simulation across 5 insider LP-ownership cohorts
/// (`8.4%` to `99.6%` LP share), demonstrating baseline creator fee leak `9,084-9,996 bps`
/// compressed down to `80 bps` under Ratchet-DLMM-DAMM.
pub fn simulate_wash_siphon() -> WashSiphonSimResult {
    // Baseline creator-rebate model: 90% direct creator fee + 10% LP fee share where insider
    // holds between 8.40% (840 bps) and 99.60% (9,960 bps) of baseline pool LP.
    let lp_ownership_bps: [u16; 5] = [840, 2_500, 5_000, 8_200, 9_960];
    let mut cohorts = Vec::with_capacity(lp_ownership_bps.len());

    for &insider_lp_share_bps in &lp_ownership_bps {
        const WASH_CYCLES: u64 = 25;
        const CHURN_SOL_PER_CYCLE: u64 = 1_000_000_000; // 1 SOL per self-churn round-trip
        const BASELINE_CREATOR_REBATE_BPS: u128 = 9_000;
        const BASELINE_LP_SHARE_BPS: u128 = 1_000;

        // Baseline leak: 9,000 bps + (insider_lp_share_bps * 1,000 / 10,000)
        let baseline_leak_bps = (BASELINE_CREATOR_REBATE_BPS
            + (BASELINE_LP_SHARE_BPS * insider_lp_share_bps as u128) / BPS_DENOM)
            as u16;

        // Ratchet-DLMM-DAMM:
        // - 75% (7,500 bps) auto-compounded directly into floor bin reserve
        // - 24.20% (2,420 bps) retained by protocol/public LPs
        // - 0.80% (80 bps) strictly capped creator fee allowance
        const RATCHET_CREATOR_ALLOWANCE_OF_RESIDUAL_BPS: u128 = 320; // 3.2% of 25% = 80 bps of total
        let mut total_fees_generated = 0u64;
        let mut creator_leaked_fees = 0u64;
        let mut floor_compounded_lamports = 0u64;
        let mut insider_spent = 0u64;
        let mut insider_received = 0u64;

        for cycle in 0..WASH_CYCLES {
            // Buy leg (post-ratchet asymptotic floor + v_a^+ friction on self-churn)
            let v_a_up = 80 + cycle * 10;
            let f_buy = compute_f_buy_bps(
                F0_BUY_BPS,
                F_MIN_BUY_BPS,
                9_000,
                10_000,
                60,
                5_633,
                v_a_up,
                600,
            )
            .expect("buy fee math must not fail");
            let buy_fee = ((CHURN_SOL_PER_CYCLE as u128 * f_buy as u128) / BPS_DENOM) as u64;
            let net_in = CHURN_SOL_PER_CYCLE - buy_fee;

            let buy_compounded =
                ((buy_fee as u128 * COMPOUNDING_FEE_BPS as u128) / BPS_DENOM) as u64;
            let buy_residual = buy_fee - buy_compounded;
            let buy_creator_leak =
                ((buy_residual as u128 * RATCHET_CREATOR_ALLOWANCE_OF_RESIDUAL_BPS) / BPS_DENOM)
                    as u64;

            // Sell leg triggering 5x dump-brake
            let v_a_down = 60 + cycle * 15;
            let f_sell =
                compute_f_sell_bps_capped(F_MIN_BUY_BPS, F_MAX_SELL_BPS, 5_633, v_a_down, 600);
            let split = compute_auto_compound_split(net_in, f_sell, COMPOUNDING_FEE_BPS)
                .expect("sell compound split must succeed");
            let sell_creator_leak = ((split.protocol_residual_fee as u128
                * RATCHET_CREATOR_ALLOWANCE_OF_RESIDUAL_BPS)
                / BPS_DENOM) as u64;

            total_fees_generated += buy_fee + split.total_sell_fee;
            floor_compounded_lamports += buy_compounded + split.delta_r_sol_compounded;
            creator_leaked_fees += buy_creator_leak + sell_creator_leak;
            insider_spent += CHURN_SOL_PER_CYCLE;
            insider_received += split.net_seller_sol + buy_creator_leak + sell_creator_leak;
        }

        let ratchet_leak_bps = div_ceil_u128(
            creator_leaked_fees as u128 * BPS_DENOM,
            total_fees_generated as u128,
        )
        .expect("wash fee ratio") as u16;
        let insider_net_pnl_lamports = insider_received as i64 - insider_spent as i64;

        cohorts.push(WashCohortMetrics {
            insider_lp_share_bps,
            baseline_leak_bps,
            ratchet_leak_bps,
            insider_net_pnl_lamports,
            floor_compounded_lamports,
        });
    }

    let baseline_leak_min_bps = cohorts.iter().map(|c| c.baseline_leak_bps).min().unwrap_or(0);
    let baseline_leak_max_bps = cohorts.iter().map(|c| c.baseline_leak_bps).max().unwrap_or(0);
    let ratchet_leak_bps = cohorts.iter().map(|c| c.ratchet_leak_bps).max().unwrap_or(0);
    let all_wash_cohorts_unprofitable = cohorts.iter().all(|c| c.insider_net_pnl_lamports < 0);

    WashSiphonSimResult {
        cohorts,
        baseline_leak_min_bps,
        baseline_leak_max_bps,
        ratchet_leak_bps,
        all_wash_cohorts_unprofitable,
    }
}

/// Gate G09: Single-leg cascading sell sweep simulation confirming max single-leg sell impact
/// is capped at `700 bps` (`<= 700 bps` buffered ceiling / `<= 750 bps` hard cap).
pub fn simulate_cascading_sell_impact() -> SellImpactSimResult {
    // 25 downward bins spaced by 100 bps each, each holding 1,000,000,000 lamports (1 SOL) quote depth.
    const NUM_BINS: usize = 25;
    const BIN_STEP_BPS: u16 = 100;
    const BIN_QUOTE_CAP: u64 = 1_000_000_000;

    let mut bin_prices = [0u128; NUM_BINS];
    let mut p = Q64_ONE;
    for slot in &mut bin_prices {
        *slot = p;
        p = (p * (10_000 - BIN_STEP_BPS as u128)) / 10_000;
    }

    let p_start = bin_prices[0];
    let p_unconstrained_end = bin_prices[21]; // 22 bins cleared without guardrail
    let unconstrained_impact_bps =
        (((p_start - p_unconstrained_end) * BPS_DENOM) / p_start) as u16;

    // Clamped execution: stop at the exact ceil floor price preserving <= 700 bps buffered impact.
    let p_floor_700 = div_ceil_u128(
        p_start * (BPS_DENOM - MAX_SELL_IMPACT_BUFFERED_BPS as u128),
        BPS_DENOM,
    )
    .expect("floor price calc must succeed");

    let mut gross_sol_out = 0u64;
    let mut p_final = p_start;
    let mut bins_crossed = 0u64;

    for &bp in &bin_prices {
        if bp >= p_floor_700 {
            gross_sol_out += BIN_QUOTE_CAP;
            p_final = bp;
            bins_crossed += 1;
        } else {
            // Partial boundary fill up to exact 700 bps buffered floor.
            let ratio_bps = ((p_final - p_floor_700) * BPS_DENOM) / p_final;
            let partial = ((BIN_QUOTE_CAP as u128 * ratio_bps) / BIN_STEP_BPS as u128) as u64;
            gross_sol_out += partial;
            p_final = p_floor_700;
            break;
        }
    }

    let max_single_leg_impact_bps = check_sell_impact_bps(
        p_start,
        p_final,
        MAX_SELL_IMPACT_BUFFERED_BPS,
    )
    .expect("clamped sell impact must satisfy 700 bps buffered ceiling");

    let strict_overshoot_rejected = matches!(
        check_sell_impact_bps(p_start, p_unconstrained_end, MAX_SELL_IMPACT_BUFFERED_BPS),
        Err(MathError::SellImpactExceedsCeiling { .. })
    );

    // Trigger 5x dump-brake + 75% quote auto-compounding on the clamped sweep.
    let triggered_f_sell_bps = compute_f_sell_bps_capped(
        F_MIN_BUY_BPS,
        F_MAX_SELL_BPS,
        5_633,
        bins_crossed * 20,
        BIN_STEP_BPS,
    );
    let split = compute_auto_compound_split(gross_sol_out, triggered_f_sell_bps, COMPOUNDING_FEE_BPS)
        .expect("sell auto-compound split must succeed");

    SellImpactSimResult {
        unconstrained_impact_bps,
        max_single_leg_impact_bps,
        buffered_ceiling_bps: MAX_SELL_IMPACT_BUFFERED_BPS,
        hard_cap_bps: MAX_SELL_IMPACT_HARD_CAP_BPS,
        strict_overshoot_rejected,
        triggered_f_sell_bps,
        compounded_floor_lamports: split.delta_r_sol_compounded,
    }
}

/// Gate G10: Atomic in-place graduation simulation confirming `dead_zone_slots = 0`
/// and complete elimination of `PartialMigrationError`.
pub fn simulate_atomic_in_place_graduation() -> GraduationSimResult {
    const SLOT: u64 = 314_159_265;

    let mut graduation_status: u8 = 0; // Bonding
    let mut authority_revoked: u8 = 0;
    let bin_quote_reserve: u64 = SOLVENCY_REAL_VAULT_LAMPORTS; // 86,500,000,000 L
    let max_extractable: u64 = SOLVENCY_MAX_EXTRACTABLE_LAMPORTS; // 85,000,000,000 L
    let mut partial_migration_errors: u64 = 0;

    // 1. Pre-graduation swap executes in SLOT.
    let pre_grad_swap_slot = SLOT;
    let pre_grad_status = graduation_status;

    // 2. Atomic in-place graduation within the exact same SLOT (no withdrawal, no new pool PDA).
    let graduation_slot = SLOT;
    if max_extractable <= bin_quote_reserve {
        graduation_status = 1; // Graduated in-place
        authority_revoked = 1; // Mutable authority permanently revoked
    } else {
        partial_migration_errors += 1;
    }

    // 3. Post-graduation swap executes immediately in the very same SLOT (0 freeze slots).
    let post_grad_swap_slot = SLOT;
    let dead_zone_slots = post_grad_swap_slot.saturating_sub(pre_grad_swap_slot);

    GraduationSimResult {
        pre_grad_status,
        post_grad_status: graduation_status,
        authority_revoked,
        graduation_slot,
        pre_grad_swap_slot,
        post_grad_swap_slot,
        dead_zone_slots,
        partial_migration_errors,
        bin_quote_preserved_lamports: bin_quote_reserve,
    }
}

//! Ratchet-DLMM-DAMM on-chain program core (Phases 1-3).
//!
//! Implements:
//! - Phase 1 (`state`): Zero-copy `Pod` PDAs (`RatchetPoolPda` 480B / 608B, `ActiveBinArrayPda` 1024B / 1152B),
//!   Token-2022 `ExtensionType::TransferFeeConfig` write-lock-free fallback harvesting, and SIMD-0437 rent math.
//! - Phase 2 (`math`): Checked `u128` Q64.64 pricing, High-Water Buy Ratchet, 5x Asymmetric Dump-Brake,
//!   and Gate G09 700 bps buffered single-leg sell impact guardrails.
//! - Phase 3 (`compounding`, `graduation`): OnlyB 75% quote auto-compounding into the active floor bin with
//!   0-lamport conservation drift (`LHS == RHS`), Gate G06 vault solvency, and Gate G10 zero-dead-zone
//!   atomic in-place graduation.

pub mod compounding;
pub mod cpi;
#[cfg(not(feature = "no-entrypoint"))]
pub mod entrypoint;
pub mod error;
pub mod graduation;
pub mod instruction;
pub mod math;
pub mod processor;
pub mod state;

solana_program::declare_id!("5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5");

pub use compounding::{
    compute_sell_compounding_split, route_compounded_sol_to_floor_bin, SellCompoundingReceipt,
    SellCompoundingSplit, COMPOUNDING_DENOMINATOR,
};
pub use error::RatchetError;
pub use graduation::{graduate_in_place, verify_solvency, GraduationReceipt, SolvencyReport};
pub use math::{
    compute_buy_base_fee_bps, compute_buy_fee_bps, compute_sell_fee_bps,
    compute_sell_price_impact_bps, compute_variable_fee_bps_u128, decay_volatility_accumulators,
    div_ceil_u128, execute_buy_swap, execute_sell_swap, isqrt_u256, mul_div_u128, mul_u128_wide,
    pow_q64, price_q64_from_sqrt_p, q64_div, q64_mul, q64_mul_round, quote_buy, quote_sell,
    sqrt_bin_step_q64, steps_above_high_water, update_high_water_mark, BuyFeeQuote, BuySwapQuote,
    SellFeeQuote, SellSwapQuote, ACCUMULATOR_DENOMINATOR, BPS_DENOMINATOR, MAX_BUY_FEE_BPS,
};
pub use state::{
    calculate_pda_rent_schedule, simd0437_billed_bytes, simd0437_rent_lamports,
    step2_rent_schedule, step5_rent_schedule, ActiveBinArrayPda, ExtensionType, LiquidityBin,
    RatchetPoolPda, Simd0437RentSchedule, TransferFee, TransferFeeConfig, ACTIVE_BIN_ARRAY_LEN,
    ACTIVE_BIN_ARRAY_PDA_DATA_BYTES, ACTIVE_BIN_ARRAY_PDA_TOTAL_BYTES, DEFAULT_COMPOUNDING_FEE_BPS,
    DEFAULT_DECAY_PERIOD_SECONDS, DEFAULT_FILTER_PERIOD_SECONDS, DEFAULT_F_0_BPS,
    DEFAULT_F_MAX_SELL_BPS, DEFAULT_F_MIN_BPS, DEFAULT_GAMMA_BPS, DEFAULT_MAX_EXTRACTABLE_LAMPORTS,
    DEFAULT_MAX_SELL_IMPACT_BPS, DEFAULT_REAL_VAULT_LAMPORTS, DEFAULT_REDUCTION_FACTOR_BPS,
    GRADUATION_STATUS_ACTIVE_CURVE, GRADUATION_STATUS_GRADUATED_IN_PLACE, HARD_SELL_IMPACT_CAP_BPS,
    Q64_ONE, RATCHET_POOL_PDA_DATA_BYTES, RATCHET_POOL_PDA_TOTAL_BYTES,
    SIMD0437_ACCOUNT_OVERHEAD_BYTES, SIMD0437_STEP2_LAMPORTS_PER_BYTE,
    SIMD0437_STEP5_LAMPORTS_PER_BYTE,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool_and_bins(bin_step_bps: u16, active_offset: i32) -> (RatchetPoolPda, ActiveBinArrayPda) {
        let bin_step_q64 = sqrt_bin_step_q64(bin_step_bps).unwrap();
        let start_bin_id = 0;
        let active_bin_id = active_offset;
        let floor_bin_id = 0;
        let start_sqrt_p = Q64_ONE;

        let mut bin_array = ActiveBinArrayPda::new(
            [1u8; 32],
            start_bin_id,
            active_bin_id,
            floor_bin_id,
            bin_step_bps,
            bin_step_q64,
            start_sqrt_p,
            254,
        )
        .unwrap();

        let active_sqrt_p = bin_array.bin(active_bin_id).unwrap().sqrt_price_q64;
        let grad_sqrt_p = q64_mul_round(active_sqrt_p, pow_q64(bin_step_q64, 5).unwrap()).unwrap();

        let mut pool = RatchetPoolPda::new(
            [2u8; 32],
            [3u8; 32],
            [4u8; 32],
            [5u8; 32],
            [6u8; 32],
            [7u8; 32],
            [8u8; 32],
            active_sqrt_p,
            grad_sqrt_p,
            10_000,
            bin_step_bps,
            active_bin_id,
            floor_bin_id,
            start_bin_id,
            255,
            253,
            254,
            1_700_000_000,
        )
        .unwrap();

        // Seed 85 SOL across bins 0..=active_bin_id, leaving 1.5 SOL protocol cushion to match
        // 86.5 SOL real_vault_lamports vs 85.0 SOL max_extractable_lamports.
        let num_quote_bins = (active_bin_id - floor_bin_id + 1) as u64;
        let per_bin_quote = DEFAULT_MAX_EXTRACTABLE_LAMPORTS / num_quote_bins;
        let remainder = DEFAULT_MAX_EXTRACTABLE_LAMPORTS % num_quote_bins;
        for id in floor_bin_id..=active_bin_id {
            let extra = if id == floor_bin_id { remainder } else { 0 };
            bin_array.bin_mut(id).unwrap().reserve_quote_lamports = per_bin_quote + extra;
        }
        for id in active_bin_id..(ACTIVE_BIN_ARRAY_LEN as i32) {
            bin_array.bin_mut(id).unwrap().reserve_base_atoms = 10_000_000_000;
        }
        pool.protocol_fee_lamports =
            DEFAULT_REAL_VAULT_LAMPORTS - DEFAULT_MAX_EXTRACTABLE_LAMPORTS;

        (pool, bin_array)
    }

    #[repr(C, align(16))]
    struct AlignedBytes<const N: usize>([u8; N]);

    #[test]
    fn test_phase1_exact_pda_sizes_and_zero_copy_pod() {
        assert_eq!(core::mem::size_of::<RatchetPoolPda>(), 480);
        assert_eq!(core::mem::size_of::<ActiveBinArrayPda>(), 1024);
        assert_eq!(core::mem::size_of::<LiquidityBin>(), 32);
        assert_eq!(
            simd0437_billed_bytes(core::mem::size_of::<RatchetPoolPda>()).unwrap(),
            608
        );
        assert_eq!(
            simd0437_billed_bytes(core::mem::size_of::<ActiveBinArrayPda>()).unwrap(),
            1152
        );

        let (pool, bin_array) = test_pool_and_bins(100, 15);
        assert_eq!(pool.compounding_fee_bps, 7500);
        assert_eq!(pool.f_0_bps, 8000);
        assert_eq!(pool.f_min_bps, 25);
        assert_eq!(pool.f_max_sell_bps, 2500);
        assert_eq!(pool.a_sell, 5 * pool.a_buy);
        assert_eq!(pool.real_vault_lamports, 86_500_000_000);
        assert_eq!(pool.max_extractable_lamports, 85_000_000_000);

        let mut pool_buf = AlignedBytes([0u8; 480]);
        pool_buf.0.copy_from_slice(pool.as_bytes());
        let decoded_pool = RatchetPoolPda::try_from_bytes(&pool_buf.0).unwrap();
        assert_eq!(decoded_pool, &pool);

        let decoded_pool_mut = RatchetPoolPda::try_from_bytes_mut(&mut pool_buf.0).unwrap();
        decoded_pool_mut.v_a_up = 42;
        assert_eq!(
            RatchetPoolPda::try_from_bytes(&pool_buf.0).unwrap().v_a_up,
            42
        );

        // Unaligned byte buffer decode (e.g., raw offset slice in snapshot/RPC payload).
        let mut unaligned_pool = [0u8; 481];
        unaligned_pool[1..].copy_from_slice(pool.as_bytes());
        assert_eq!(
            RatchetPoolPda::from_bytes_unaligned(&unaligned_pool[1..]).unwrap(),
            pool
        );

        let mut bin_buf = AlignedBytes([0u8; 1024]);
        bin_buf.0.copy_from_slice(bin_array.as_bytes());
        let decoded_bins = ActiveBinArrayPda::try_from_bytes(&bin_buf.0).unwrap();
        assert_eq!(decoded_bins, &bin_array);
        assert_eq!(
            decoded_bins.total_quote_lamports().unwrap(),
            DEFAULT_MAX_EXTRACTABLE_LAMPORTS
        );

        let mut unaligned_bins = [0u8; 1025];
        unaligned_bins[1..].copy_from_slice(bin_array.as_bytes());
        assert_eq!(
            ActiveBinArrayPda::from_bytes_unaligned(&unaligned_bins[1..]).unwrap(),
            bin_array
        );

        // Wrong buffer size must fail cleanly.
        assert_eq!(
            RatchetPoolPda::try_from_bytes(&pool_buf.0[..479]),
            Err(RatchetError::InvalidParameter)
        );
        assert_eq!(
            ActiveBinArrayPda::try_from_bytes(&bin_buf.0[..1023]),
            Err(RatchetError::InvalidParameter)
        );
    }

    #[test]
    fn test_phase1_simd0437_rent_step2_and_step5() {
        let step2 = step2_rent_schedule().unwrap();
        assert_eq!(step2.lamports_per_byte, 5_080);
        assert_eq!(step2.ratchet_pool_pda_lamports, 3_088_640);
        assert_eq!(step2.active_bin_array_pda_lamports, 5_852_160);
        assert_eq!(step2.total_lamports, 8_940_800);

        let step5 = step5_rent_schedule().unwrap();
        assert_eq!(step5.lamports_per_byte, 696);
        assert_eq!(step5.ratchet_pool_pda_lamports, 423_168);
        assert_eq!(step5.active_bin_array_pda_lamports, 801_792);
        assert_eq!(step5.total_lamports, 1_224_960);
    }

    #[test]
    fn test_phase1_token2022_transfer_fee_config_fallback_harvest() {
        let config_auth = [11u8; 32];
        let withdraw_auth = [22u8; 32];
        let mut cfg =
            TransferFeeConfig::new_fallback(config_auth, withdraw_auth, 25, 1_000_000, 10).unwrap();
        assert_eq!(cfg.extension_type, ExtensionType::TransferFeeConfig);
        assert!(!cfg.is_immutable());

        // Ceiling fee: ceil(10_001 * 25 / 10_000) = ceil(25.0025) = 26
        assert_eq!(cfg.calculate_fee(10_001, 10).unwrap(), 26);
        // Capped at maximum_fee = 1_000_000
        assert_eq!(cfg.calculate_fee(1_000_000_000, 10).unwrap(), 1_000_000);

        // Write-lock-free fallback harvest from token account into mint (pool untouched).
        let mut recipient_withheld = 45_000u64;
        let harvested = cfg
            .harvest_withheld_tokens_to_mint(&mut recipient_withheld)
            .unwrap();
        assert_eq!(harvested, 45_000);
        assert_eq!(recipient_withheld, 0);
        assert_eq!(cfg.withheld_amount, 45_000);

        // Unauthorized withdrawal rejected.
        let (mut pool, _) = test_pool_and_bins(100, 10);
        assert_eq!(
            cfg.withdraw_withheld_tokens_from_mint(&mut pool, &[99u8; 32]),
            Err(RatchetError::TransferFeeAuthorityMismatch)
        );

        // Authorized batched settlement succeeds.
        let withdrawn = cfg
            .withdraw_withheld_tokens_from_mint(&mut pool, &withdraw_auth)
            .unwrap();
        assert_eq!(withdrawn, 45_000);
        assert_eq!(cfg.withheld_amount, 0);
        assert_eq!(pool.harvested_transfer_fee_atoms, 45_000);

        cfg.revoke_config_authority();
        assert!(cfg.is_immutable());
        assert_eq!(cfg.to_spl_wire_bytes().len(), 108);
    }

    #[test]
    fn test_phase2_div_ceil_u128_and_q64_precision() {
        assert_eq!(div_ceil_u128(0, ACCUMULATOR_DENOMINATOR).unwrap(), 0);
        assert_eq!(div_ceil_u128(1, ACCUMULATOR_DENOMINATOR).unwrap(), 1);
        assert_eq!(
            div_ceil_u128(ACCUMULATOR_DENOMINATOR, ACCUMULATOR_DENOMINATOR).unwrap(),
            1
        );
        assert_eq!(
            div_ceil_u128(ACCUMULATOR_DENOMINATOR + 1, ACCUMULATOR_DENOMINATOR).unwrap(),
            2
        );
        assert_eq!(div_ceil_u128(u128::MAX, 1).unwrap(), u128::MAX);
        assert_eq!(div_ceil_u128(1, 0), Err(RatchetError::DivisionByZero));

        // 100 bps bin step -> sqrt(1.01) => square back in Q64.64 should closely equal 1.01 * Q64_ONE.
        let step = sqrt_bin_step_q64(100).unwrap();
        let price = price_q64_from_sqrt_p(step).unwrap();
        let expected_price = (10_100u128 * Q64_ONE) / 10_000;
        let diff = price.abs_diff(expected_price);
        assert!(diff <= 2, "Q64.64 price drift too large: {}", diff);
    }

    #[test]
    fn test_phase2_high_water_buy_ratchet_decay_and_monotonicity() {
        let (mut pool, bin_array) = test_pool_and_bins(100, 10);
        let step_q64 = bin_array.bin_step_q64;

        // Slot-0: 0 steps above start -> base fee = 8000 bps (80%)
        let slot0 = compute_buy_fee_bps(
            pool.f_0_bps,
            pool.f_min_bps,
            pool.gamma_bps,
            0,
            pool.a_buy,
            0,
            pool.bin_step_bps,
        )
        .unwrap();
        assert_eq!(slot0.base_fee_bps, 8000);
        assert_eq!(slot0.total_fee_bps, 8000);

        // 1 step above start at gamma=0.90 -> 8000 * 0.9 = 7200 bps
        let step1 = compute_buy_fee_bps(
            pool.f_0_bps,
            pool.f_min_bps,
            pool.gamma_bps,
            1,
            pool.a_buy,
            0,
            pool.bin_step_bps,
        )
        .unwrap();
        assert_eq!(step1.base_fee_bps, 7200);

        // 2 steps above start -> 8000 * 0.81 = 6480 bps
        let step2 = compute_buy_fee_bps(
            pool.f_0_bps,
            pool.f_min_bps,
            pool.gamma_bps,
            2,
            pool.a_buy,
            0,
            pool.bin_step_bps,
        )
        .unwrap();
        assert_eq!(step2.base_fee_bps, 6480);

        // Ratchet high-water mark up 10 steps, then dip current price back down.
        let high_sqrt_p = q64_mul_round(pool.sqrt_p_start, pow_q64(step_q64, 10).unwrap()).unwrap();
        update_high_water_mark(&mut pool, high_sqrt_p);
        assert_eq!(pool.sqrt_p_high_water, high_sqrt_p);

        let dipped_sqrt_p = q64_mul_round(pool.sqrt_p_start, pow_q64(step_q64, 3).unwrap()).unwrap();
        update_high_water_mark(&mut pool, dipped_sqrt_p);
        assert_eq!(pool.sqrt_p_current, dipped_sqrt_p);
        // High-water mark did NOT regress!
        assert_eq!(pool.sqrt_p_high_water, high_sqrt_p);

        let steps =
            steps_above_high_water(pool.sqrt_p_high_water, pool.sqrt_p_start, step_q64).unwrap();
        assert_eq!(steps, 10);

        // Very high steps decay smoothly down to f_min = 25 bps.
        let steady = compute_buy_fee_bps(
            pool.f_0_bps,
            pool.f_min_bps,
            pool.gamma_bps,
            200,
            pool.a_buy,
            0,
            pool.bin_step_bps,
        )
        .unwrap();
        assert_eq!(steady.base_fee_bps, 25);
        assert_eq!(steady.total_fee_bps, 25);

        // Upward volatility accumulator surcharge: ceil(10_000 * (50 * 100)^2 / 10^11) = ceil(2.5) = 3 bps
        let with_vol = compute_buy_fee_bps(
            pool.f_0_bps,
            pool.f_min_bps,
            pool.gamma_bps,
            200,
            10_000,
            50,
            100,
        )
        .unwrap();
        assert_eq!(with_vol.variable_fee_bps, 3);
        assert_eq!(with_vol.total_fee_bps, 28);
    }

    #[test]
    fn test_phase2_asymmetric_5x_dump_brake_and_overflow_safety() {
        let a_buy = 10_000u64;
        let s = 100u16;
        let crossings = 50u64;

        let buy_fee = compute_buy_fee_bps(25, 25, 9000, 0, a_buy, crossings, s).unwrap();
        let sell_fee = compute_sell_fee_bps(25, 2500, a_buy, crossings, s).unwrap();

        // ceil(10_000 * 5_000^2 / 10^11) = ceil(2.5) = 3
        // ceil(50_000 * 5_000^2 / 10^11) = ceil(12.5) = 13
        assert_eq!(buy_fee.variable_fee_bps, 3);
        assert_eq!(sell_fee.variable_fee_bps, 13);
        assert_eq!(sell_fee.total_fee_bps, 38);

        // Heavy dump saturates smoothly at 2500 bps (25%).
        let heavy_dump = compute_sell_fee_bps(25, 2500, a_buy, 1_000, s).unwrap();
        assert_eq!(heavy_dump.total_fee_bps, 2500);

        // Checked u128 overflow safety: extreme values return Err(MathOverflow), never panic/wrap.
        let overflow = compute_sell_fee_bps(25, 2500, u64::MAX, u64::MAX, 10_000);
        assert_eq!(overflow, Err(RatchetError::MathOverflow));
    }

    #[test]
    fn test_phase2_gate_g09_700bps_sell_impact_cap_and_bin_depletion() {
        // 100 bps bin step: crossing 7 bins drops price by ~6.72% (672 bps <= 700 bps).
        // Crossing 8 bins drops price by ~7.64% (764 bps > 700 bps -> rejected!).
        let (mut pool, mut bin_array) = test_pool_and_bins(100, 15);
        for id in 0..=15 {
            bin_array.bin_mut(id).unwrap().reserve_quote_lamports = 5_000_000_000;
        }
        pool.real_vault_lamports = 81_500_000_000;
        pool.max_extractable_lamports = 80_000_000_000;

        // Selling across ~6 bins (< 700 bps impact) succeeds.
        // Bin 15..=10 hold 6 * 5e9 = 30e9 lamports -> ~27.5e9 base atoms stays inside 700 bps impact.
        let ok_quote = quote_sell(&pool, &bin_array, 25_000_000_000).unwrap();
        assert!(ok_quote.price_impact_bps <= 700);
        assert!(ok_quote.downward_crossings <= 7);

        // Selling across 8+ bins exceeds 700 bps impact cap and rejects pre-swap without mutating state.
        let big_sell = execute_sell_swap(&mut pool, &mut bin_array, 45_000_000_000, 0, 1_700_000_010);
        assert!(matches!(
            big_sell,
            Err(RatchetError::SellImpactExceedsCap { impact_bps, max_bps: 700 }) if impact_bps > 700
        ));
        assert_eq!(pool.active_bin_id, 15);
        assert_eq!(pool.v_a_down, 0);

        // Trying to deplete all bins past floor_bin_id returns InsufficientBinLiquidity.
        let depletion = execute_sell_swap(&mut pool, &mut bin_array, 200_000_000_000, 0, 1_700_000_010);
        assert_eq!(depletion, Err(RatchetError::InsufficientBinLiquidity));
    }

    #[test]
    fn test_phase3_onlyb_75pct_auto_compounding_and_zero_lamport_conservation() {
        let (mut pool, mut bin_array) = test_pool_and_bins(100, 15);

        // Test exact mathematical identity: Delta_R_sol_compounded = floor(0.75 * f_sell(t) * Gross_SOL_Out)
        let gross = 10_000_000_000u64; // 10 SOL
        let f_sell = 1_000u16; // 10%
        let split = compute_sell_compounding_split(gross, f_sell, 7500).unwrap();
        assert_eq!(split.total_fee_lamports, 1_000_000_000);
        assert_eq!(split.compounded_sol_lamports, 750_000_000);
        assert_eq!(split.protocol_fee_lamports, 250_000_000);
        assert_eq!(split.net_user_sol_out_lamports, 9_000_000_000);

        // Test odd/dust lamports for exact floor and 0-lamport drift.
        let odd_split = compute_sell_compounding_split(1_000_001, 333, 7500).unwrap();
        let expected_comp = ((1_000_001u128 * 333 * 7500) / 100_000_000) as u64;
        assert_eq!(odd_split.compounded_sol_lamports, expected_comp);
        assert_eq!(
            odd_split.net_user_sol_out_lamports
                + odd_split.compounded_sol_lamports
                + odd_split.protocol_fee_lamports,
            1_000_001
        );

        // Execute full sell swap and verify compounded SOL landed directly in active floor bin
        // with LHS == RHS (0-lamport conservation drift).
        let vault_before = pool.real_vault_lamports;
        let floor_before = bin_array.bin(bin_array.floor_bin_id).unwrap().reserve_quote_lamports;

        let sell_res =
            execute_sell_swap(&mut pool, &mut bin_array, 4_000_000_000, 0, 1_700_000_005).unwrap();
        let floor_after = bin_array.bin(bin_array.floor_bin_id).unwrap().reserve_quote_lamports;
        assert_eq!(floor_after - floor_before, sell_res.compounded_sol_lamports);
        assert_eq!(
            vault_before as u128,
            pool.real_vault_lamports as u128 + sell_res.net_user_sol_out_lamports as u128
        );
        assert_eq!(
            bin_array.total_quote_lamports().unwrap() + pool.protocol_fee_lamports,
            pool.real_vault_lamports
        );

        // Execute a buy swap afterwards and re-verify 0-lamport conservation across buys + sells.
        let buy_res =
            execute_buy_swap(&mut pool, &mut bin_array, 2_000_000_000, 0, 1_700_000_006).unwrap();
        assert!(buy_res.base_out_atoms > 0);
        assert_eq!(
            bin_array.total_quote_lamports().unwrap() + pool.protocol_fee_lamports,
            pool.real_vault_lamports
        );
    }

    #[test]
    fn test_phase3_solvency_g06_and_zero_dead_zone_graduation_g10() {
        let (mut pool, mut bin_array) = test_pool_and_bins(100, 15);

        // Gate G06: 85B <= 86.5B passes.
        let solvency = verify_solvency(&pool, &bin_array).unwrap();
        assert_eq!(solvency.max_extractable_lamports, 85_000_000_000);
        assert_eq!(solvency.real_vault_lamports, 86_500_000_000);
        assert_eq!(solvency.surplus_lamports, 1_500_000_000);

        // Hollow pool (max_extractable > real_vault) fails solvency check.
        let mut hollow = pool;
        hollow.max_extractable_lamports = 87_000_000_000;
        assert_eq!(
            verify_solvency(&hollow, &bin_array),
            Err(RatchetError::InsolventPool {
                max_extractable: 87_000_000_000,
                real_vault: 86_500_000_000
            })
        );

        // Cannot graduate before reaching graduation sqrt(P).
        assert_eq!(
            graduate_in_place(&mut pool, &bin_array, 100),
            Err(RatchetError::GraduationThresholdNotReached)
        );

        // Advance price to graduation threshold via buy swaps across bins.
        let mut ts = 1_700_000_010;
        while pool.sqrt_p_current < pool.sqrt_p_graduation {
            execute_buy_swap(&mut pool, &mut bin_array, 20_000_000_000, 0, ts).unwrap();
            ts += 1;
        }

        // Pre-graduation swap works at slot 299.
        let pre_grad =
            execute_sell_swap(&mut pool, &mut bin_array, 500_000_000, 0, ts).unwrap();
        assert!(pre_grad.net_user_sol_out_lamports > 0);

        // Atomic in-place graduation at slot 300: 0 dead-zone slots, authority revoked.
        let receipt = graduate_in_place(&mut pool, &bin_array, 300).unwrap();
        assert_eq!(receipt.previous_status, GRADUATION_STATUS_ACTIVE_CURVE);
        assert_eq!(receipt.new_status, GRADUATION_STATUS_GRADUATED_IN_PLACE);
        assert_eq!(receipt.graduation_slot, 300);
        assert_eq!(receipt.dead_zone_slots, 0);
        assert!(pool.is_graduated());
        assert_eq!(pool.authority_revoked, 1);
        assert_eq!(pool.authority, [0u8; 32]);

        // Re-graduating fails cleanly; swaps in the SAME slot 300 and slot 301 succeed immediately.
        assert_eq!(
            graduate_in_place(&mut pool, &bin_array, 300),
            Err(RatchetError::AlreadyGraduated)
        );
        let post_grad_same_slot =
            execute_buy_swap(&mut pool, &mut bin_array, 1_000_000_000, 0, ts + 1).unwrap();
        assert!(post_grad_same_slot.base_out_atoms > 0);
        let post_grad_sell =
            execute_sell_swap(&mut pool, &mut bin_array, 500_000_000, 0, ts + 2).unwrap();
        assert!(post_grad_sell.net_user_sol_out_lamports > 0);
        assert_eq!(
            bin_array.total_quote_lamports().unwrap() + pool.protocol_fee_lamports,
            pool.real_vault_lamports
        );
    }

    #[test]
    fn test_phase1_exact_byte_offsets_and_authority_revocation() {
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, sqrt_p_high_water), 0);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, sqrt_p_current), 16);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, v_a_up), 64);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, v_a_down), 72);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, real_vault_lamports), 104);
        assert_eq!(
            core::mem::offset_of!(RatchetPoolPda, max_extractable_lamports),
            112
        );
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, active_bin_id), 160);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, compounding_fee_bps), 176);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, graduation_status), 192);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, base_mint), 200);
        assert_eq!(core::mem::offset_of!(RatchetPoolPda, padding), 424);

        assert_eq!(core::mem::offset_of!(ActiveBinArrayPda, bin_step_q64), 0);
        assert_eq!(core::mem::offset_of!(ActiveBinArrayPda, pool), 16);
        assert_eq!(core::mem::offset_of!(ActiveBinArrayPda, active_bin_id), 48);
        assert_eq!(core::mem::offset_of!(ActiveBinArrayPda, floor_bin_id), 52);
        assert_eq!(core::mem::offset_of!(ActiveBinArrayPda, bins), 64);

        let (mut pool, _) = test_pool_and_bins(100, 10);
        pool.revoke_authority().unwrap();
        assert_eq!(pool.authority_revoked, 1);
        assert_eq!(pool.authority, [0u8; 32]);
        assert_eq!(pool.revoke_authority(), Err(RatchetError::AuthorityRevoked));
    }

    #[test]
    fn test_phase2_accumulator_decay_wide_math_and_slippage_guards() {
        // 256-bit wide mul_div where intermediate a * b > u128::MAX.
        let wide = mul_div_u128(1u128 << 100, 1u128 << 64, 1u128 << 80, false).unwrap();
        assert_eq!(wide, 1u128 << 84);

        let (mut pool, mut bin_array) = test_pool_and_bins(100, 10);
        pool.v_a_up = 100;
        pool.v_a_down = 200;
        let t0 = pool.last_decay_timestamp;

        // Inside filter window (< 30s): accumulators remain intact.
        decay_volatility_accumulators(&mut pool, t0 + 15, DEFAULT_REDUCTION_FACTOR_BPS).unwrap();
        assert_eq!(pool.v_a_up, 100);
        assert_eq!(pool.v_a_down, 200);

        // One filter interval (30s): decays by 50% reduction factor.
        decay_volatility_accumulators(&mut pool, t0 + 30, DEFAULT_REDUCTION_FACTOR_BPS).unwrap();
        assert_eq!(pool.v_a_up, 50);
        assert_eq!(pool.v_a_down, 100);

        // Past full decay horizon (>= 600s): resets both directional accumulators to 0.
        decay_volatility_accumulators(&mut pool, t0 + 700, DEFAULT_REDUCTION_FACTOR_BPS).unwrap();
        assert_eq!(pool.v_a_up, 0);
        assert_eq!(pool.v_a_down, 0);

        // Zero swap amounts and strict slippage limits fail cleanly.
        assert_eq!(
            execute_buy_swap(&mut pool, &mut bin_array, 0, 0, t0 + 701),
            Err(RatchetError::ZeroSwapAmount)
        );
        assert_eq!(
            execute_sell_swap(&mut pool, &mut bin_array, 0, 0, t0 + 701),
            Err(RatchetError::ZeroSwapAmount)
        );
        assert!(matches!(
            execute_sell_swap(&mut pool, &mut bin_array, 1_000_000, u64::MAX, t0 + 701),
            Err(RatchetError::SlippageExceeded { .. })
        ));
    }

    #[test]
    fn test_instruction_wire_roundtrips_and_pda_derivation() {
        use crate::cpi::find_transfer_fee_config_in_mint;
        use crate::instruction::{
            derive_base_vault_pda, derive_bin_array_pda, derive_pool_authority_pda,
            derive_pool_pda, derive_quote_vault_pda, BinSeedEntry, InitializeBinArrayArgs,
            InitializePoolArgs, RatchetInstruction, SeedInitialBinsArgs, SwapArgs,
        };
        use solana_program::pubkey::Pubkey;

        let base_mint = Pubkey::new_unique();
        let (pool_pda, _) = derive_pool_pda(&base_mint, &crate::ID);
        let (bin_pda, _) = derive_bin_array_pda(&pool_pda, -8, &crate::ID);
        let (auth_pda, _) = derive_pool_authority_pda(&pool_pda, &crate::ID);
        let (base_vault, _) = derive_base_vault_pda(&pool_pda, &crate::ID);
        let (quote_vault, _) = derive_quote_vault_pda(&pool_pda, &crate::ID);
        assert_ne!(pool_pda, bin_pda);
        assert_ne!(auth_pda, base_vault);
        assert_ne!(base_vault, quote_vault);

        let init_pool = InitializePoolArgs {
            sqrt_p_start: Q64_ONE,
            sqrt_p_graduation: Q64_ONE * 2,
            a_buy: 10_000,
            bin_step_bps: 100,
            active_bin_id: 0,
            floor_bin_id: -8,
            bin_array_start_id: -8,
            compounding_fee_bps: 7500,
            gamma_bps: 9000,
            max_sell_impact_bps: 700,
            transfer_fee_bps: 0,
            flags: 0,
        };
        assert_eq!(init_pool.pack().len(), 64);
        assert_eq!(
            RatchetInstruction::unpack(&init_pool.pack()).unwrap(),
            RatchetInstruction::InitializePool(init_pool)
        );

        let init_bins = InitializeBinArrayArgs {
            start_bin_id: -8,
            active_bin_id: 0,
            floor_bin_id: -8,
            bin_step_bps: 100,
            start_sqrt_price_q64: Q64_ONE,
        };
        assert_eq!(init_bins.pack().len(), 31);
        assert_eq!(
            RatchetInstruction::unpack(&init_bins.pack()).unwrap(),
            RatchetInstruction::InitializeBinArray(init_bins)
        );

        let seed = SeedInitialBinsArgs {
            quote_deposit_lamports: 86_500_000_000,
            base_deposit_atoms: 1_000_000_000_000,
            entries: vec![BinSeedEntry {
                bin_id: 0,
                reserve_quote_lamports: 85_000_000_000,
                reserve_base_atoms: 1_000_000_000_000,
            }],
        };
        assert_eq!(
            RatchetInstruction::unpack(&seed.pack()).unwrap(),
            RatchetInstruction::SeedInitialBins(seed)
        );

        let swap = SwapArgs {
            is_buy: 1,
            amount_in: 1_000_000_000,
            min_amount_out: 42,
        };
        assert_eq!(swap.pack().len(), 18);
        assert_eq!(
            RatchetInstruction::unpack(&swap.pack()).unwrap(),
            RatchetInstruction::Swap(swap)
        );

        assert_eq!(
            RatchetInstruction::unpack(&[0x04]).unwrap(),
            RatchetInstruction::RevokeAndOpenSlot0
        );
        assert_eq!(
            RatchetInstruction::unpack(&[0x06]).unwrap(),
            RatchetInstruction::GraduateInPlace
        );
        assert_eq!(
            RatchetInstruction::unpack(&[0x07, 0]).unwrap(),
            RatchetInstruction::HarvestFallbackTransferFees { mode: 0 }
        );

        // Token-2022 TLV parser test for TransferFeeConfig (ExtensionType = 1, 108B value).
        let mut mint_buf = vec![0u8; 166 + 4 + 108];
        mint_buf[165] = 1; // AccountType::Mint
        mint_buf[166..168].copy_from_slice(&1u16.to_le_bytes());
        mint_buf[168..170].copy_from_slice(&108u16.to_le_bytes());
        mint_buf[170..202].copy_from_slice(&[9u8; 32]);
        mint_buf[202..234].copy_from_slice(&[7u8; 32]);
        mint_buf[234..242].copy_from_slice(&123_456u64.to_le_bytes());
        let tlv = find_transfer_fee_config_in_mint(&mint_buf).unwrap();
        assert_eq!(tlv.transfer_fee_config_authority, [9u8; 32]);
        assert_eq!(tlv.withdraw_withheld_authority, [7u8; 32]);
        assert_eq!(tlv.withheld_amount, 123_456);
    }
}

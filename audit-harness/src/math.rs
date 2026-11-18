//! Core invariant mathematics, Q64.64 helpers, checked `u128` overflow safety,
//! and deterministic PRNG utilities for the Ratchet-DLMM-DAMM audit harness.

/// Basis-point denominator (10,000 bps = 100%).
pub const BPS_DENOM: u128 = 10_000;

/// Volatility accumulator divisor (`10^11`) from the DLMM fee equation.
pub const VOL_DIVISOR: u128 = 100_000_000_000;

/// Combined basis-point denominator (`10_000 * 10_000 = 10^8`) for `compounding_fee_bps * f_sell_bps`.
pub const COMPOUNDING_DENOMINATOR: u128 = 100_000_000;

/// Q64.64 unit constant (`1.0` = `2^64`).
pub const Q64_ONE: u128 = 1u128 << 64;

/// Fixed-point decimal scale (`10^12`) for overflow-free exponential decay `gamma^steps`.
const DECAY_SCALE: u128 = 1_000_000_000_000;

/// Slot-0 base buy fee in basis points (`80.00%` = `8,000 bps`).
pub const F0_BUY_BPS: u16 = 8_000;

/// Minimum asymptotic buy fee in basis points (`0.25%` = `25 bps`).
pub const F_MIN_BUY_BPS: u16 = 25;

/// Hard immutable cap on buy fee in basis points (`99.00%` = `9,900 bps`).
pub const F_MAX_BUY_BPS: u16 = 9_900;

/// Hard immutable cap on sell fee in basis points (`25.00%` = `2,500 bps`).
pub const F_MAX_SELL_BPS: u16 = 2_500;

/// OnlyB quote auto-compounding share of sell fees (`75.00%` = `7,500 bps`).
pub const COMPOUNDING_FEE_BPS: u16 = 7_500;

/// Buffered single-leg sell impact ceiling (`700 bps` = `7.00%`).
pub const MAX_SELL_IMPACT_BUFFERED_BPS: u16 = 700;

/// Hard single-leg sell impact cap (`750 bps` = `7.50%`).
pub const MAX_SELL_IMPACT_HARD_CAP_BPS: u16 = 750;

/// Asymmetric sell multiplier (`A_sell = 5 * A_buy`).
pub const A_SELL_MULTIPLIER: u64 = 5;

/// Gate G06 solvency benchmark balances (in lamports).
pub const SOLVENCY_REAL_VAULT_LAMPORTS: u64 = 86_500_000_000;
pub const SOLVENCY_MAX_EXTRACTABLE_LAMPORTS: u64 = 85_000_000_000;

/// Errors returned by checked protocol arithmetic and guardrails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathError {
    Overflow,
    DivisionByZero,
    InvalidParameter,
    SellImpactExceedsCeiling { impact_bps: u16, ceiling_bps: u16 },
    SolvencyViolation { max_extractable: u64, real_vault: u64 },
    InsufficientBinLiquidity,
}

/// Exact lamport breakdown of an OnlyB auto-compounded sell swap.
///
/// Invariant:
/// `net_seller_sol + delta_r_sol_compounded + protocol_residual_fee == gross_sol_out`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompoundSplit {
    pub gross_sol_out: u64,
    pub total_sell_fee: u64,
    pub net_seller_sol: u64,
    pub delta_r_sol_compounded: u64,
    pub protocol_residual_fee: u64,
}

/// Result of `u128` overflow safety verification across domain and stress inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct U128OverflowSafetyReport {
    pub domain_max_v_a_down: u64,
    pub domain_max_bin_step_bps: u16,
    pub domain_max_a_buy: u64,
    pub domain_peak_numerator: u128,
    pub u128_max: u128,
    pub headroom_bits: u32,
    pub fuzz_cases_tested: usize,
    pub silent_wraps_observed: usize,
    pub checked_rejections_on_extreme_u64: usize,
    pub safe: bool,
}

/// Checked ceiling integer division `ceil(num / den)` in `u128` without intermediate overflow.
#[inline]
pub fn div_ceil_u128(num: u128, den: u128) -> Result<u128, MathError> {
    if den == 0 {
        return Err(MathError::DivisionByZero);
    }
    if num == 0 {
        return Ok(0);
    }
    let q = num / den;
    let r = num % den;
    if r == 0 {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(MathError::Overflow)
    }
}

/// Monotonic high-water mark update: `sqrt_P_high_water = max(sqrt_P_high_water, sqrt_P_current)`.
#[inline]
pub fn update_sqrt_p_high_water(current_hwm: u128, sqrt_p_current: u128) -> u128 {
    current_hwm.max(sqrt_p_current)
}

/// Computes `ceil((base_scaled * gamma^steps) / DECAY_SCALE)` where `gamma = gamma_num / gamma_den <= 1`.
fn decayed_base_bps(
    f_0_bps: u16,
    gamma_num: u64,
    gamma_den: u64,
    steps: u32,
) -> Result<u16, MathError> {
    if gamma_den == 0 || gamma_num > gamma_den {
        return Err(MathError::InvalidParameter);
    }
    if steps == 0 || gamma_num == gamma_den {
        return Ok(f_0_bps);
    }
    let mut base = (gamma_num as u128)
        .checked_mul(DECAY_SCALE)
        .ok_or(MathError::Overflow)?
        / (gamma_den as u128);
    let mut acc = DECAY_SCALE;
    let mut exp = steps;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc
                .checked_mul(base)
                .ok_or(MathError::Overflow)?
                / DECAY_SCALE;
        }
        exp >>= 1;
        if exp > 0 {
            base = base
                .checked_mul(base)
                .ok_or(MathError::Overflow)?
                / DECAY_SCALE;
        }
    }
    let decayed = (f_0_bps as u128)
        .checked_mul(acc)
        .ok_or(MathError::Overflow)?
        / DECAY_SCALE;
    Ok(decayed.min(u16::MAX as u128) as u16)
}

/// Computes `f_buy(t)` in basis points:
/// `f_buy(t) = max(f_min, f_0 * gamma^(steps_above(sqrt_P_max))) + ceil(A_buy * (v_a^+ * s)^2 / 10^11)`.
///
/// The result is clamped at `F_MAX_BUY_BPS` (`9,900 bps`) to guarantee swap progress.
#[allow(clippy::too_many_arguments)]
pub fn compute_f_buy_bps(
    f_0_bps: u16,
    f_min_bps: u16,
    gamma_num: u64,
    gamma_den: u64,
    steps_above_hwm: u32,
    a_buy: u64,
    v_a_up: u64,
    bin_step_bps: u16,
) -> Result<u16, MathError> {
    let decayed = decayed_base_bps(f_0_bps, gamma_num, gamma_den, steps_above_hwm)?;
    let base = decayed.max(f_min_bps);

    let prod = (v_a_up as u128)
        .checked_mul(bin_step_bps as u128)
        .ok_or(MathError::Overflow)?;
    let prod_sq = prod.checked_mul(prod).ok_or(MathError::Overflow)?;
    let num = (a_buy as u128)
        .checked_mul(prod_sq)
        .ok_or(MathError::Overflow)?;
    let surcharge = div_ceil_u128(num, VOL_DIVISOR)?;

    let total = (base as u128)
        .checked_add(surcharge)
        .ok_or(MathError::Overflow)?;
    Ok(total.min(F_MAX_BUY_BPS as u128) as u16)
}

/// Computes the raw checked numerator `(v_a^- * s)^2 * 5 * A_buy` and `ceil(num / 10^11)` surcharge.
///
/// Exposed directly to audit `u128` overflow safety across domain and adversarial boundary inputs.
pub fn checked_sell_surcharge_numerator_and_bps(
    a_buy: u64,
    v_a_down: u64,
    bin_step_bps: u16,
) -> Result<(u128, u128), MathError> {
    let vs = (v_a_down as u128)
        .checked_mul(bin_step_bps as u128)
        .ok_or(MathError::Overflow)?;
    let vs_sq = vs.checked_mul(vs).ok_or(MathError::Overflow)?;
    let a_sell = (A_SELL_MULTIPLIER as u128)
        .checked_mul(a_buy as u128)
        .ok_or(MathError::Overflow)?;
    let num = vs_sq.checked_mul(a_sell).ok_or(MathError::Overflow)?;
    let surcharge = div_ceil_u128(num, VOL_DIVISOR)?;
    Ok((num, surcharge))
}

/// Computes `f_sell(t)` in basis points using strictly checked `u128` arithmetic:
/// `f_sell(t) = min(f_max_sell_bps, f_base_bps + ceil(5 * A_buy * (v_a^- * s)^2 / 10^11))`.
pub fn compute_f_sell_bps_checked(
    f_base_bps: u16,
    f_max_sell_bps: u16,
    a_buy: u64,
    v_a_down: u64,
    bin_step_bps: u16,
) -> Result<u16, MathError> {
    let (_, surcharge) = checked_sell_surcharge_numerator_and_bps(a_buy, v_a_down, bin_step_bps)?;
    let total = (f_base_bps as u128)
        .checked_add(surcharge)
        .ok_or(MathError::Overflow)?;
    Ok(total.min(f_max_sell_bps as u128) as u16)
}

/// Overflow-immune production `f_sell(t)` evaluation that short-circuits cleanly to
/// `f_max_sell_bps` whenever the checked intermediate multiplication exceeds `u128::MAX`
/// (since any overflowing positive term strictly exceeds the `2,500 bps` cap).
pub fn compute_f_sell_bps_capped(
    f_base_bps: u16,
    f_max_sell_bps: u16,
    a_buy: u64,
    v_a_down: u64,
    bin_step_bps: u16,
) -> u16 {
    if f_base_bps >= f_max_sell_bps {
        return f_max_sell_bps;
    }
    if a_buy == 0 || v_a_down == 0 || bin_step_bps == 0 {
        return f_base_bps;
    }
    match compute_f_sell_bps_checked(f_base_bps, f_max_sell_bps, a_buy, v_a_down, bin_step_bps) {
        Ok(fee) => fee,
        Err(MathError::Overflow) => f_max_sell_bps,
        Err(_) => f_max_sell_bps,
    }
}

/// Computes the OnlyB quote auto-compounding split for a sell swap with zero lamport drift,
/// matching `programs/ratchet-dlmm-damm/src/compounding.rs` and `sdk/` bit-for-bit:
/// - `total_sell_fee = 0` if `gross == 0 || f_sell_bps == 0`, else `div_ceil_u128(Gross_SOL_Out * f_sell_bps, 10_000).min(Gross_SOL_Out)`
/// - `Delta_R_sol_compounded = floor(Gross_SOL_Out * f_sell_bps * compounding_fee_bps / 100_000_000)`
/// - `protocol_residual_fee = total_sell_fee - Delta_R_sol_compounded`
/// - `net_seller_sol = Gross_SOL_Out - total_sell_fee`
///
/// Conservation invariant: `net_seller_sol + delta_r_sol_compounded + protocol_residual_fee == gross_sol_out`.
pub fn compute_auto_compound_split(
    gross_sol_out: u64,
    f_sell_bps: u16,
    compounding_fee_bps: u16,
) -> Result<CompoundSplit, MathError> {
    if f_sell_bps as u128 > BPS_DENOM || compounding_fee_bps as u128 > BPS_DENOM {
        return Err(MathError::InvalidParameter);
    }
    let gross = gross_sol_out as u128;
    let total_fee_u128 = if gross == 0 || f_sell_bps == 0 {
        0
    } else {
        div_ceil_u128(
            gross.checked_mul(f_sell_bps as u128).ok_or(MathError::Overflow)?,
            BPS_DENOM,
        )?
        .min(gross)
    };
    let total_sell_fee = u64::try_from(total_fee_u128).map_err(|_| MathError::Overflow)?;
    let net_seller_sol = gross_sol_out
        .checked_sub(total_sell_fee)
        .ok_or(MathError::Overflow)?;

    let compounded_u128 = gross
        .checked_mul(f_sell_bps as u128)
        .ok_or(MathError::Overflow)?
        .checked_mul(compounding_fee_bps as u128)
        .ok_or(MathError::Overflow)?
        / COMPOUNDING_DENOMINATOR;
    let delta_r_sol_compounded =
        u64::try_from(compounded_u128).map_err(|_| MathError::Overflow)?;
    let protocol_residual_fee = total_sell_fee
        .checked_sub(delta_r_sol_compounded)
        .ok_or(MathError::Overflow)?;

    debug_assert_eq!(
        net_seller_sol as u128 + delta_r_sol_compounded as u128 + protocol_residual_fee as u128,
        gross_sol_out as u128
    );

    Ok(CompoundSplit {
        gross_sol_out,
        total_sell_fee,
        net_seller_sol,
        delta_r_sol_compounded,
        protocol_residual_fee,
    })
}

/// Pre-swap sell price impact check enforcing the buffered 700 bps ceiling inside the 750 bps hard cap.
pub fn check_sell_impact_bps(p_start_q64: u128, p_end_q64: u128, ceiling_bps: u16) -> Result<u16, MathError> {
    if p_start_q64 == 0 {
        return Err(MathError::DivisionByZero);
    }
    if p_end_q64 >= p_start_q64 {
        return Ok(0);
    }
    let drop = p_start_q64 - p_end_q64;
    let impact_u128 = div_ceil_u128(
        drop.checked_mul(BPS_DENOM).ok_or(MathError::Overflow)?,
        p_start_q64,
    )?;
    let impact_bps = impact_u128.min(u16::MAX as u128) as u16;
    if impact_bps > ceiling_bps {
        return Err(MathError::SellImpactExceedsCeiling {
            impact_bps,
            ceiling_bps,
        });
    }
    Ok(impact_bps)
}

/// Verifies `u128` overflow safety of `(v_a^- * s)^2 * 5 * A_buy / 10^11` across:
/// 1. Worst-case realistic operational domain (`v_a^- <= 1_000_000_000`, `s <= 10_000`, `A_buy <= 1_000_000`)
/// 2. 5,000 deterministic fuzzed boundary and extreme `u64` inputs confirming zero silent wrapping.
pub fn verify_u128_overflow_safety() -> U128OverflowSafetyReport {
    let domain_max_v_a_down: u64 = 1_000_000_000;
    let domain_max_bin_step_bps: u16 = 10_000;
    let domain_max_a_buy: u64 = 1_000_000;

    let (domain_peak_numerator, _) = checked_sell_surcharge_numerator_and_bps(
        domain_max_a_buy,
        domain_max_v_a_down,
        domain_max_bin_step_bps,
    )
    .expect("operational domain must never overflow u128");

    let headroom_bits = domain_peak_numerator.leading_zeros();
    let mut rng = DeterministicRng::new(0x5241_5443_4845_5431);
    let mut fuzz_cases_tested = 0usize;
    let mut checked_rejections_on_extreme_u64 = 0usize;
    let mut silent_wraps_observed = 0usize;

    // 2,500 domain fuzz cases: all must succeed cleanly in checked u128 arithmetic.
    for _ in 0..2_500 {
        let v_a = rng.next_u64() % (domain_max_v_a_down + 1);
        let s = (rng.next_u64() % (domain_max_bin_step_bps as u64 + 1)) as u16;
        let a_buy = rng.next_u64() % (domain_max_a_buy + 1);
        match compute_f_sell_bps_checked(25, F_MAX_SELL_BPS, a_buy, v_a, s) {
            Ok(fee) => {
                if !(25..=F_MAX_SELL_BPS).contains(&fee) {
                    silent_wraps_observed += 1;
                }
            }
            Err(_) => {
                silent_wraps_observed += 1;
            }
        }
        fuzz_cases_tested += 1;
    }

    // 2,500 adversarial extreme u64 cases: checked returns Err(Overflow) or valid <= cap;
    // capped production version must ALWAYS return <= F_MAX_SELL_BPS without panicking.
    for i in 0..2_500 {
        let (v_a, s, a_buy) = if i == 0 {
            (u64::MAX, u16::MAX, u64::MAX)
        } else if i == 1 {
            (u64::MAX, 10_000, 5_633)
        } else {
            (rng.next_u64(), (rng.next_u64() & 0xFFFF) as u16, rng.next_u64())
        };
        if let Err(MathError::Overflow) =
            checked_sell_surcharge_numerator_and_bps(a_buy, v_a, s)
        {
            checked_rejections_on_extreme_u64 += 1;
        }
        let safe_fee = compute_f_sell_bps_capped(25, F_MAX_SELL_BPS, a_buy, v_a, s);
        if !(25..=F_MAX_SELL_BPS).contains(&safe_fee) {
            silent_wraps_observed += 1;
        }
        fuzz_cases_tested += 1;
    }

    U128OverflowSafetyReport {
        domain_max_v_a_down,
        domain_max_bin_step_bps,
        domain_max_a_buy,
        domain_peak_numerator,
        u128_max: u128::MAX,
        headroom_bits,
        fuzz_cases_tested,
        silent_wraps_observed,
        checked_rejections_on_extreme_u64,
        safe: domain_peak_numerator < u128::MAX
            && silent_wraps_observed == 0
            && checked_rejections_on_extreme_u64 > 0,
    }
}

/// Deterministic SplitMix64 PRNG for zero-dependency reproducible fuzzing.
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_range_u64(&mut self, min: u64, max_inclusive: u64) -> u64 {
        if min >= max_inclusive {
            return min;
        }
        let span = max_inclusive - min + 1;
        min + (self.next_u64() % span)
    }
}

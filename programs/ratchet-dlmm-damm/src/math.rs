use crate::compounding::{compute_sell_compounding_split, route_compounded_sol_to_floor_bin};
use crate::error::RatchetError;
use crate::state::{
    ActiveBinArrayPda, RatchetPoolPda, DEFAULT_REDUCTION_FACTOR_BPS, HARD_SELL_IMPACT_CAP_BPS,
    Q64_ONE,
};

/// Denominator in the directional volatility variable-fee term (`10^11`).
pub const ACCUMULATOR_DENOMINATOR: u128 = 100_000_000_000;

/// Basis points denominator (`100.00%`).
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Safety cap on buy fee bps (`99.00%`) so valid buy swaps never deduct 100% of input.
pub const MAX_BUY_FEE_BPS: u16 = 9_900;

/// Result of evaluating the High-Water Buy Ratchet fee formula `f_buy(t)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BuyFeeQuote {
    /// Decayed base buy fee `max(f_min, f_0 * gamma^steps_above)` in bps.
    pub base_fee_bps: u16,
    /// Directional upward volatility surcharge `ceil(A_buy * (v_a_up * s)^2 / 10^11)` in bps.
    pub variable_fee_bps: u16,
    /// Combined buy fee in bps (`<= MAX_BUY_FEE_BPS`).
    pub total_fee_bps: u16,
    /// Number of discrete steps `sqrt_p_high_water` stands above `sqrt_p_start`.
    pub steps_above: u32,
}

/// Result of evaluating the Asymmetric Dump-Brake sell fee formula `f_sell(t)`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SellFeeQuote {
    /// Resting base sell fee `f_base` in bps (typically `f_min_bps = 25`).
    pub base_fee_bps: u16,
    /// 5x downward volatility surcharge `ceil(5 * A_buy * (v_a_down * s)^2 / 10^11)` in bps.
    pub variable_fee_bps: u16,
    /// Total sell fee clamped at `f_max_sell_bps` (`2500` bps = 25%).
    pub total_fee_bps: u16,
}

/// Pre-swap quote result for a buy (`quote_in -> base_out`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BuySwapQuote {
    pub quote_in_lamports: u64,
    pub fee_lamports: u64,
    pub net_quote_lamports: u64,
    pub base_out_atoms: u64,
    pub fee_quote: BuyFeeQuote,
    pub start_bin_id: i32,
    pub end_bin_id: i32,
    pub end_sqrt_p_q64: u128,
    pub upward_crossings: u64,
}

/// Pre-swap quote result for a sell (`base_in -> quote_out`), including Gate G09 & OnlyB metrics.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SellSwapQuote {
    pub base_in_atoms: u64,
    pub gross_sol_out_lamports: u64,
    pub total_fee_lamports: u64,
    pub compounded_sol_lamports: u64,
    pub protocol_fee_lamports: u64,
    pub net_user_sol_out_lamports: u64,
    pub fee_quote: SellFeeQuote,
    pub start_bin_id: i32,
    pub end_bin_id: i32,
    pub end_sqrt_p_q64: u128,
    pub downward_crossings: u64,
    pub price_impact_bps: u16,
}

/// Exact overflow-safe ceiling division on `u128`.
///
/// Computes `ceil(num / denom)` using `(num - 1) / denom + 1` for `num > 0` so `num == u128::MAX`
/// never overflows during numerator addition. Uses native 64-bit division when operands fit in `u64`.
#[inline]
pub fn div_ceil_u128(num: u128, denom: u128) -> Result<u128, RatchetError> {
    if denom == 0 {
        return Err(RatchetError::DivisionByZero);
    }
    if num == 0 {
        return Ok(0);
    }
    if num <= u64::MAX as u128 && denom <= u64::MAX as u128 {
        return Ok((((num as u64) - 1) / (denom as u64) + 1) as u128);
    }
    Ok((num - 1) / denom + 1)
}

/// Exact widening 128x128 -> 256-bit unsigned multiplication returning `(hi, lo)`.
#[inline]
pub fn mul_u128_wide(a: u128, b: u128) -> (u128, u128) {
    let mask = u64::MAX as u128;
    let a_lo = a & mask;
    let a_hi = a >> 64;
    let b_lo = b & mask;
    let b_hi = b >> 64;

    let p0 = a_lo * b_lo;
    let p1 = a_lo * b_hi;
    let p2 = a_hi * b_lo;
    let p3 = a_hi * b_hi;

    let mid = (p0 >> 64) + (p1 & mask) + (p2 & mask);
    let lo = (p0 & mask) | ((mid & mask) << 64);
    let hi = p3 + (p1 >> 64) + (p2 >> 64) + (mid >> 64);
    (hi, lo)
}

/// Computes `floor(a * b / denom)` or `ceil(a * b / denom)` using an exact 256-bit intermediate.
#[inline]
pub fn mul_div_u128(
    a: u128,
    b: u128,
    denom: u128,
    round_up: bool,
) -> Result<u128, RatchetError> {
    if denom == 0 {
        return Err(RatchetError::DivisionByZero);
    }
    if a == 0 || b == 0 {
        return Ok(0);
    }
    let (hi, lo) = mul_u128_wide(a, b);
    if denom == Q64_ONE {
        if hi >= Q64_ONE {
            return Err(RatchetError::MathOverflow);
        }
        let mut q = (hi << 64) | (lo >> 64);
        if round_up && (lo as u64) != 0 {
            q = q.checked_add(1).ok_or(RatchetError::MathOverflow)?;
        }
        return Ok(q);
    }
    if hi == 0 {
        return if round_up {
            div_ceil_u128(lo, denom)
        } else {
            Ok(lo / denom)
        };
    }
    if hi >= denom {
        return Err(RatchetError::MathOverflow);
    }

    let mut rem = hi;
    let mut quot: u128 = 0;
    for i in (0..128).rev() {
        let bit = (lo >> i) & 1;
        let rem_msb = rem >> 127;
        rem = (rem << 1) | bit;
        if rem_msb != 0 || rem >= denom {
            rem = rem.wrapping_sub(denom);
            quot |= 1u128 << i;
        }
    }
    if round_up && rem > 0 {
        quot = quot.checked_add(1).ok_or(RatchetError::MathOverflow)?;
    }
    Ok(quot)
}

/// Computes `floor((a * b) / 2^64)` with overflow check.
pub fn q64_mul(a: u128, b: u128) -> Result<u128, RatchetError> {
    let (hi, lo) = mul_u128_wide(a, b);
    if hi >= (1u128 << 64) {
        return Err(RatchetError::MathOverflow);
    }
    Ok((hi << 64) | (lo >> 64))
}

/// Computes round-nearest `((a * b) + 2^63) >> 64` with overflow check.
pub fn q64_mul_round(a: u128, b: u128) -> Result<u128, RatchetError> {
    let (mut hi, lo) = mul_u128_wide(a, b);
    let (lo_rounded, carry) = lo.overflowing_add(1u128 << 63);
    if carry {
        hi = hi.checked_add(1).ok_or(RatchetError::MathOverflow)?;
    }
    if hi >= (1u128 << 64) {
        return Err(RatchetError::MathOverflow);
    }
    Ok((hi << 64) | (lo_rounded >> 64))
}

/// Computes `floor((a * 2^64) / b)` in Q64.64 fixed point.
pub fn q64_div(a: u128, b: u128) -> Result<u128, RatchetError> {
    mul_div_u128(a, Q64_ONE, b, false)
}

/// Exponentiation by squaring in Q64.64 fixed point (round-nearest per step).
pub fn pow_q64(mut base_q64: u128, mut exp: u32) -> Result<u128, RatchetError> {
    let mut acc = Q64_ONE;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = q64_mul_round(acc, base_q64)?;
        }
        exp >>= 1;
        if exp > 0 {
            base_q64 = q64_mul_round(base_q64, base_q64)?;
        }
    }
    Ok(acc)
}

/// Exact integer floor square root of a 256-bit integer `(hi, lo)`.
pub fn isqrt_u256(hi: u128, lo: u128) -> u128 {
    let mut low: u128 = 0;
    let mut high: u128 = u128::MAX;
    while low < high {
        let mid = low + ((high - low) >> 1) + 1;
        let (m_hi, m_lo) = mul_u128_wide(mid, mid);
        if m_hi < hi || (m_hi == hi && m_lo <= lo) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

/// Computes exact Q64.64 multiplicative step `sqrt(1 + bin_step_bps / 10_000)`.
pub fn sqrt_bin_step_q64(bin_step_bps: u16) -> Result<u128, RatchetError> {
    if bin_step_bps == 0 || bin_step_bps > 10_000 {
        return Err(RatchetError::InvalidBinStep);
    }
    let num = BPS_DENOMINATOR + bin_step_bps as u128;
    let q_hi = num / BPS_DENOMINATOR;
    let rem = num % BPS_DENOMINATOR;
    let top_mid = (rem << 64) / BPS_DENOMINATOR;
    let rem_mid = (rem << 64) % BPS_DENOMINATOR;
    let low_mid = (rem_mid << 64) / BPS_DENOMINATOR;
    let q_lo = (top_mid << 64) | low_mid;
    Ok(isqrt_u256(q_hi, q_lo))
}

/// Converts Q64.64 `sqrt(P)` into Q64.64 spot price `P = (sqrt_P * sqrt_P) >> 64`.
pub fn price_q64_from_sqrt_p(sqrt_p_q64: u128) -> Result<u128, RatchetError> {
    q64_mul(sqrt_p_q64, sqrt_p_q64)
}

/// Computes the integer number of discrete steps `sqrt_p_high_water` sits above `sqrt_p_start`.
pub fn steps_above_high_water(
    sqrt_p_high_water: u128,
    sqrt_p_start: u128,
    step_q64: u128,
) -> Result<u32, RatchetError> {
    if sqrt_p_start == 0 || step_q64 <= Q64_ONE {
        return Err(RatchetError::InvalidBinStep);
    }
    if sqrt_p_high_water <= sqrt_p_start {
        return Ok(0);
    }

    // Fast path for the common 1..=32 step range matching `ActiveBinArrayPda` Q64 rounding.
    let mut cur = sqrt_p_start;
    for step in 1..=32u32 {
        match q64_mul_round(cur, step_q64) {
            Ok(next) if next <= sqrt_p_high_water => {
                if next <= cur {
                    return Ok(step - 1);
                }
                cur = next;
            }
            _ => return Ok(step - 1),
        }
    }

    let mut low: u32 = 32;
    let mut high: u32 = 64;
    loop {
        match pow_q64(step_q64, high).and_then(|factor| q64_mul(sqrt_p_start, factor)) {
            Ok(val) if val <= sqrt_p_high_water => {
                low = high;
                high = high.saturating_mul(2);
                if high == low {
                    return Ok(low);
                }
            }
            _ => break,
        }
    }

    while low < high {
        let mid = low + ((high - low) >> 1) + 1;
        let fits = pow_q64(step_q64, mid)
            .and_then(|factor| q64_mul(sqrt_p_start, factor))
            .map(|val| val <= sqrt_p_high_water)
            .unwrap_or(false);
        if fits {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok(low)
}

/// Monotonically updates `pool.sqrt_p_current` and `pool.sqrt_p_high_water = max(high_water, current)`.
pub fn update_high_water_mark(pool: &mut RatchetPoolPda, sqrt_p_new: u128) -> u128 {
    pool.sqrt_p_current = sqrt_p_new;
    if sqrt_p_new > pool.sqrt_p_high_water {
        pool.sqrt_p_high_water = sqrt_p_new;
    }
    pool.sqrt_p_high_water
}

/// Computes the decaying base buy fee `max(f_min, f_0 * gamma^steps_above)` in basis points.
pub fn compute_buy_base_fee_bps(
    f_0_bps: u16,
    f_min_bps: u16,
    gamma_bps: u16,
    steps_above: u32,
) -> Result<u16, RatchetError> {
    if f_min_bps > f_0_bps || f_0_bps > 10_000 || gamma_bps > 10_000 {
        return Err(RatchetError::InvalidFeeConfig);
    }
    if steps_above == 0 || gamma_bps == 10_000 {
        return Ok(f_0_bps.max(f_min_bps));
    }
    if gamma_bps == 0 {
        return Ok(f_min_bps);
    }
    let gamma_q64 = (((gamma_bps as u128) << 64) + (BPS_DENOMINATOR / 2)) / BPS_DENOMINATOR;
    let gamma_pow = pow_q64(gamma_q64, steps_above)?;
    let decayed = (((f_0_bps as u128) * gamma_pow + (Q64_ONE >> 1)) >> 64) as u16;
    Ok(decayed.max(f_min_bps))
}

/// Computes the directional variable fee term `ceil(coeff_a * (v_a * s)^2 / 10^11)` using checked `u128`.
pub fn compute_variable_fee_bps_u128(
    coeff_a: u64,
    v_a: u64,
    bin_step_bps: u16,
) -> Result<u128, RatchetError> {
    if bin_step_bps == 0 || bin_step_bps > 10_000 {
        return Err(RatchetError::InvalidBinStep);
    }
    let v_s = (v_a as u128)
        .checked_mul(bin_step_bps as u128)
        .ok_or(RatchetError::MathOverflow)?;
    let v_s_sq = v_s.checked_mul(v_s).ok_or(RatchetError::MathOverflow)?;
    let numerator = (coeff_a as u128)
        .checked_mul(v_s_sq)
        .ok_or(RatchetError::MathOverflow)?;
    div_ceil_u128(numerator, ACCUMULATOR_DENOMINATOR)
}

/// Computes High-Water Buy Ratchet fee:
/// `f_buy(t) = max(f_min, f_0 * gamma^(steps_above(sqrt_P_max))) + ceil(A_buy * (v_a^+ * s)^2 / 10^11)`.
pub fn compute_buy_fee_bps(
    f_0_bps: u16,
    f_min_bps: u16,
    gamma_bps: u16,
    steps_above: u32,
    a_buy: u64,
    v_a_up: u64,
    bin_step_bps: u16,
) -> Result<BuyFeeQuote, RatchetError> {
    let base_fee_bps = compute_buy_base_fee_bps(f_0_bps, f_min_bps, gamma_bps, steps_above)?;
    let var_u128 = compute_variable_fee_bps_u128(a_buy, v_a_up, bin_step_bps)?;
    let total_u128 = (base_fee_bps as u128)
        .checked_add(var_u128)
        .ok_or(RatchetError::MathOverflow)?;
    let total_fee_bps = total_u128.min(MAX_BUY_FEE_BPS as u128) as u16;
    let variable_fee_bps = var_u128.min(MAX_BUY_FEE_BPS as u128) as u16;

    Ok(BuyFeeQuote {
        base_fee_bps,
        variable_fee_bps,
        total_fee_bps,
        steps_above,
    })
}

/// Computes Asymmetric Dump-Brake sell fee:
/// `f_sell(t) = min(25%, f_base + ceil(5 * A_buy * (v_a^- * s)^2 / 10^11))`.
pub fn compute_sell_fee_bps(
    f_base_bps: u16,
    f_max_sell_bps: u16,
    a_buy: u64,
    v_a_down: u64,
    bin_step_bps: u16,
) -> Result<SellFeeQuote, RatchetError> {
    if f_max_sell_bps > 2_500 || f_base_bps > f_max_sell_bps {
        return Err(RatchetError::InvalidFeeConfig);
    }
    let a_sell = a_buy.checked_mul(5).ok_or(RatchetError::MathOverflow)?;
    let var_u128 = compute_variable_fee_bps_u128(a_sell, v_a_down, bin_step_bps)?;
    let total_u128 = (f_base_bps as u128)
        .checked_add(var_u128)
        .ok_or(RatchetError::MathOverflow)?;
    let total_fee_bps = total_u128.min(f_max_sell_bps as u128) as u16;
    let variable_fee_bps = var_u128.min(f_max_sell_bps as u128) as u16;

    Ok(SellFeeQuote {
        base_fee_bps: f_base_bps,
        variable_fee_bps,
        total_fee_bps,
    })
}

/// Applies time-based decay to directional volatility accumulators `v_a_up` and `v_a_down`.
pub fn decay_volatility_accumulators(
    pool: &mut RatchetPoolPda,
    current_timestamp: i64,
    reduction_factor_bps: u16,
) -> Result<(), RatchetError> {
    if reduction_factor_bps > 10_000 {
        return Err(RatchetError::InvalidFeeConfig);
    }
    if current_timestamp <= pool.last_decay_timestamp {
        return Ok(());
    }
    let elapsed = current_timestamp - pool.last_decay_timestamp;
    if elapsed < pool.filter_period_seconds as i64 {
        return Ok(());
    }
    if elapsed >= pool.decay_period_seconds as i64 {
        pool.v_a_up = 0;
        pool.v_a_down = 0;
        pool.last_decay_timestamp = current_timestamp;
        return Ok(());
    }

    let filter = (pool.filter_period_seconds as i64).max(1);
    let steps = (elapsed / filter) as u32;
    let factor_q64 = (((reduction_factor_bps as u128) << 64) + (BPS_DENOMINATOR / 2)) / BPS_DENOMINATOR;
    let decay_q64 = pow_q64(factor_q64, steps)?;
    pool.v_a_up = ((pool.v_a_up as u128 * decay_q64) >> 64) as u64;
    pool.v_a_down = ((pool.v_a_down as u128 * decay_q64) >> 64) as u64;
    pool.last_decay_timestamp = current_timestamp;
    Ok(())
}

/// Computes single-leg downward sell price impact in bps: `ceil((P_start - P_end) * 10_000 / P_start)`.
pub fn compute_sell_price_impact_bps(
    sqrt_p_start: u128,
    sqrt_p_end: u128,
) -> Result<u16, RatchetError> {
    if sqrt_p_start == 0 || sqrt_p_end == 0 {
        return Err(RatchetError::InvalidParameter);
    }
    if sqrt_p_end >= sqrt_p_start {
        return Ok(0);
    }
    let p_start = price_q64_from_sqrt_p(sqrt_p_start)?;
    let p_end = price_q64_from_sqrt_p(sqrt_p_end)?;
    if p_end >= p_start {
        return Ok(0);
    }
    let drop = p_start - p_end;
    let impact = mul_div_u128(drop, BPS_DENOMINATOR, p_start, true)?;
    u16::try_from(impact).map_err(|_| RatchetError::MathOverflow)
}

/// Pre-swap sell simulation enforcing Gate G09 (700 bps buffered inside 750 bps hard cap) and bin depletion checks.
pub fn quote_sell(
    pool: &RatchetPoolPda,
    bin_array: &ActiveBinArrayPda,
    base_in_atoms: u64,
) -> Result<SellSwapQuote, RatchetError> {
    pool.validate()?;
    if base_in_atoms == 0 {
        return Err(RatchetError::ZeroSwapAmount);
    }
    if pool.max_sell_impact_bps > HARD_SELL_IMPACT_CAP_BPS {
        return Err(RatchetError::InvalidFeeConfig);
    }

    let start_bin_id = bin_array.active_bin_id;
    let start_sqrt_p = bin_array.bin(start_bin_id)?.sqrt_price_q64;

    let mut remaining_base = base_in_atoms as u128;
    let mut gross_sol_out: u128 = 0;
    let mut current_bin_id = start_bin_id;
    let mut end_sqrt_p = start_sqrt_p;

    while remaining_base > 0 {
        if current_bin_id < bin_array.floor_bin_id || current_bin_id < bin_array.start_bin_id {
            return Err(RatchetError::InsufficientBinLiquidity);
        }
        let bin = bin_array.bin(current_bin_id)?;
        if bin.reserve_quote_lamports == 0 {
            if current_bin_id == bin_array.floor_bin_id || current_bin_id == bin_array.start_bin_id {
                return Err(RatchetError::InsufficientBinLiquidity);
            }
            current_bin_id -= 1;
            continue;
        }
        end_sqrt_p = bin.sqrt_price_q64;
        let price_q64 = price_q64_from_sqrt_p(bin.sqrt_price_q64)?;
        if price_q64 == 0 {
            return Err(RatchetError::DivisionByZero);
        }

        let quote_needed = mul_div_u128(remaining_base, price_q64, Q64_ONE, false)?;
        if quote_needed <= bin.reserve_quote_lamports as u128 {
            gross_sol_out = gross_sol_out
                .checked_add(quote_needed)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_base = 0;
        } else {
            let bin_quote = bin.reserve_quote_lamports as u128;
            let base_absorbed = mul_div_u128(bin_quote, Q64_ONE, price_q64, true)?.min(remaining_base);
            gross_sol_out = gross_sol_out
                .checked_add(bin_quote)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_base -= base_absorbed;
            if remaining_base > 0 {
                if current_bin_id == bin_array.floor_bin_id || current_bin_id == bin_array.start_bin_id {
                    return Err(RatchetError::InsufficientBinLiquidity);
                }
                current_bin_id -= 1;
            }
        }
    }

    let end_bin_id = current_bin_id;
    let price_impact_bps = compute_sell_price_impact_bps(start_sqrt_p, end_sqrt_p)?;
    if price_impact_bps > pool.max_sell_impact_bps || price_impact_bps > HARD_SELL_IMPACT_CAP_BPS {
        return Err(RatchetError::SellImpactExceedsCap {
            impact_bps: price_impact_bps,
            max_bps: pool.max_sell_impact_bps.min(HARD_SELL_IMPACT_CAP_BPS),
        });
    }

    let gross_sol_u64 = u64::try_from(gross_sol_out).map_err(|_| RatchetError::MathOverflow)?;
    let downward_crossings = (start_bin_id - end_bin_id) as u64;
    let projected_v_a_down = pool.v_a_down.saturating_add(downward_crossings);
    let fee_quote = compute_sell_fee_bps(
        pool.f_min_bps,
        pool.f_max_sell_bps,
        pool.a_buy,
        projected_v_a_down,
        pool.bin_step_bps,
    )?;

    let split = compute_sell_compounding_split(
        gross_sol_u64,
        fee_quote.total_fee_bps,
        pool.compounding_fee_bps,
    )?;

    Ok(SellSwapQuote {
        base_in_atoms,
        gross_sol_out_lamports: gross_sol_u64,
        total_fee_lamports: split.total_fee_lamports,
        compounded_sol_lamports: split.compounded_sol_lamports,
        protocol_fee_lamports: split.protocol_fee_lamports,
        net_user_sol_out_lamports: split.net_user_sol_out_lamports,
        fee_quote,
        start_bin_id,
        end_bin_id,
        end_sqrt_p_q64: end_sqrt_p,
        downward_crossings,
        price_impact_bps,
    })
}

/// Executes an atomic sell swap with pre-swap Gate G09 impact & bin-depletion checks,
/// 5x directional dump-brake update, and same-instruction OnlyB floor-bin auto-compounding.
pub fn execute_sell_swap(
    pool: &mut RatchetPoolPda,
    bin_array: &mut ActiveBinArrayPda,
    base_in_atoms: u64,
    min_quote_out_lamports: u64,
    current_timestamp: i64,
) -> Result<SellSwapQuote, RatchetError> {
    decay_volatility_accumulators(pool, current_timestamp, DEFAULT_REDUCTION_FACTOR_BPS)?;
    let quote = quote_sell(pool, bin_array, base_in_atoms)?;
    if quote.net_user_sol_out_lamports < min_quote_out_lamports {
        return Err(RatchetError::SlippageExceeded {
            actual_out: quote.net_user_sol_out_lamports,
            min_out: min_quote_out_lamports,
        });
    }

    let mut remaining_base = base_in_atoms as u128;
    let mut current_bin_id = quote.start_bin_id;

    while remaining_base > 0 {
        let bin = bin_array.bin_mut(current_bin_id)?;
        if bin.reserve_quote_lamports == 0 {
            current_bin_id -= 1;
            continue;
        }
        let price_q64 = price_q64_from_sqrt_p(bin.sqrt_price_q64)?;
        let quote_needed = mul_div_u128(remaining_base, price_q64, Q64_ONE, false)?;

        if quote_needed <= bin.reserve_quote_lamports as u128 {
            let quote_taken = u64::try_from(quote_needed).map_err(|_| RatchetError::MathOverflow)?;
            let base_added = u64::try_from(remaining_base).map_err(|_| RatchetError::MathOverflow)?;
            bin.reserve_quote_lamports = bin
                .reserve_quote_lamports
                .checked_sub(quote_taken)
                .ok_or(RatchetError::InsufficientBinLiquidity)?;
            bin.reserve_base_atoms = bin
                .reserve_base_atoms
                .checked_add(base_added)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_base = 0;
        } else {
            let bin_quote = bin.reserve_quote_lamports as u128;
            let base_absorbed = mul_div_u128(bin_quote, Q64_ONE, price_q64, true)?.min(remaining_base);
            let base_added = u64::try_from(base_absorbed).map_err(|_| RatchetError::MathOverflow)?;
            bin.reserve_quote_lamports = 0;
            bin.reserve_base_atoms = bin
                .reserve_base_atoms
                .checked_add(base_added)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_base -= base_absorbed;
            if remaining_base > 0 {
                current_bin_id -= 1;
            }
        }
    }

    let _receipt = route_compounded_sol_to_floor_bin(
        pool,
        bin_array,
        quote.gross_sol_out_lamports,
        quote.fee_quote.total_fee_bps,
    )?;

    pool.v_a_down = pool.v_a_down.saturating_add(quote.downward_crossings);
    pool.active_bin_id = quote.end_bin_id;
    bin_array.active_bin_id = quote.end_bin_id;
    pool.sqrt_p_current = quote.end_sqrt_p_q64;
    pool.total_volume_quote = pool
        .total_volume_quote
        .checked_add(quote.gross_sol_out_lamports)
        .ok_or(RatchetError::MathOverflow)?;

    Ok(quote)
}

/// Pre-swap buy simulation (`quote_in -> base_out`) with High-Water Buy Ratchet pricing.
pub fn quote_buy(
    pool: &RatchetPoolPda,
    bin_array: &ActiveBinArrayPda,
    quote_in_lamports: u64,
) -> Result<BuySwapQuote, RatchetError> {
    pool.validate()?;
    if quote_in_lamports == 0 {
        return Err(RatchetError::ZeroSwapAmount);
    }

    let steps_above =
        steps_above_high_water(pool.sqrt_p_high_water, pool.sqrt_p_start, bin_array.bin_step_q64)?;
    let start_bin_id = bin_array.active_bin_id;
    let max_bin_id = bin_array
        .start_bin_id
        .checked_add(crate::state::ACTIVE_BIN_ARRAY_LEN as i32 - 1)
        .ok_or(RatchetError::MathOverflow)?;

    // First estimate crossings at current v_a_up, then price with projected crossings.
    let initial_fee_quote = compute_buy_fee_bps(
        pool.f_0_bps,
        pool.f_min_bps,
        pool.gamma_bps,
        steps_above,
        pool.a_buy,
        pool.v_a_up,
        pool.bin_step_bps,
    )?;
    let initial_fee_lamports = div_ceil_u128(
        (quote_in_lamports as u128) * (initial_fee_quote.total_fee_bps as u128),
        BPS_DENOMINATOR,
    )? as u64;
    let initial_net_quote = quote_in_lamports.saturating_sub(initial_fee_lamports);
    let est_crossings = simulate_buy_crossings(bin_array, start_bin_id, max_bin_id, initial_net_quote)?;

    let projected_v_a_up = pool.v_a_up.saturating_add(est_crossings);
    let fee_quote = compute_buy_fee_bps(
        pool.f_0_bps,
        pool.f_min_bps,
        pool.gamma_bps,
        steps_above,
        pool.a_buy,
        projected_v_a_up,
        pool.bin_step_bps,
    )?;
    let fee_lamports = div_ceil_u128(
        (quote_in_lamports as u128) * (fee_quote.total_fee_bps as u128),
        BPS_DENOMINATOR,
    )? as u64;
    let net_quote_lamports = quote_in_lamports
        .checked_sub(fee_lamports)
        .ok_or(RatchetError::MathOverflow)?;

    let mut remaining_quote = net_quote_lamports as u128;
    let mut base_out: u128 = 0;
    let mut current_bin_id = start_bin_id;
    let mut end_sqrt_p = bin_array.bin(start_bin_id)?.sqrt_price_q64;

    while remaining_quote > 0 {
        if current_bin_id > max_bin_id {
            return Err(RatchetError::InsufficientBinLiquidity);
        }
        let bin = bin_array.bin(current_bin_id)?;
        if bin.reserve_base_atoms == 0 {
            if current_bin_id == max_bin_id {
                return Err(RatchetError::InsufficientBinLiquidity);
            }
            current_bin_id += 1;
            continue;
        }
        end_sqrt_p = bin.sqrt_price_q64;
        let price_q64 = price_q64_from_sqrt_p(bin.sqrt_price_q64)?;
        if price_q64 == 0 {
            return Err(RatchetError::DivisionByZero);
        }

        let max_quote_for_bin = mul_div_u128(bin.reserve_base_atoms as u128, price_q64, Q64_ONE, true)?;
        if remaining_quote <= max_quote_for_bin {
            let base_bought = mul_div_u128(remaining_quote, Q64_ONE, price_q64, false)?
                .min(bin.reserve_base_atoms as u128);
            base_out = base_out
                .checked_add(base_bought)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_quote = 0;
        } else {
            base_out = base_out
                .checked_add(bin.reserve_base_atoms as u128)
                .ok_or(RatchetError::MathOverflow)?;
            remaining_quote -= max_quote_for_bin;
            if remaining_quote > 0 {
                if current_bin_id == max_bin_id {
                    return Err(RatchetError::InsufficientBinLiquidity);
                }
                current_bin_id += 1;
            }
        }
    }

    let end_bin_id = current_bin_id;
    let upward_crossings = (end_bin_id - start_bin_id) as u64;
    let base_out_u64 = u64::try_from(base_out).map_err(|_| RatchetError::MathOverflow)?;

    Ok(BuySwapQuote {
        quote_in_lamports,
        fee_lamports,
        net_quote_lamports,
        base_out_atoms: base_out_u64,
        fee_quote,
        start_bin_id,
        end_bin_id,
        end_sqrt_p_q64: end_sqrt_p,
        upward_crossings,
    })
}

fn simulate_buy_crossings(
    bin_array: &ActiveBinArrayPda,
    start_bin_id: i32,
    max_bin_id: i32,
    net_quote_lamports: u64,
) -> Result<u64, RatchetError> {
    let mut remaining_quote = net_quote_lamports as u128;
    let mut current_bin_id = start_bin_id;
    while remaining_quote > 0 {
        if current_bin_id > max_bin_id {
            return Err(RatchetError::InsufficientBinLiquidity);
        }
        let bin = bin_array.bin(current_bin_id)?;
        if bin.reserve_base_atoms == 0 {
            if current_bin_id == max_bin_id {
                return Err(RatchetError::InsufficientBinLiquidity);
            }
            current_bin_id += 1;
            continue;
        }
        let price_q64 = price_q64_from_sqrt_p(bin.sqrt_price_q64)?;
        let max_quote_for_bin = mul_div_u128(bin.reserve_base_atoms as u128, price_q64, Q64_ONE, true)?;
        if remaining_quote <= max_quote_for_bin {
            remaining_quote = 0;
        } else {
            remaining_quote -= max_quote_for_bin;
            if remaining_quote > 0 {
                if current_bin_id == max_bin_id {
                    return Err(RatchetError::InsufficientBinLiquidity);
                }
                current_bin_id += 1;
            }
        }
    }
    Ok((current_bin_id - start_bin_id) as u64)
}

/// Executes an atomic buy swap, updating directional accumulator `v_a_up`, ratcheting `sqrt_p_high_water`,
/// and maintaining exact vault solvency (`LHS == RHS`).
pub fn execute_buy_swap(
    pool: &mut RatchetPoolPda,
    bin_array: &mut ActiveBinArrayPda,
    quote_in_lamports: u64,
    min_base_out_atoms: u64,
    current_timestamp: i64,
) -> Result<BuySwapQuote, RatchetError> {
    decay_volatility_accumulators(pool, current_timestamp, DEFAULT_REDUCTION_FACTOR_BPS)?;
    let quote = quote_buy(pool, bin_array, quote_in_lamports)?;
    if quote.base_out_atoms < min_base_out_atoms {
        return Err(RatchetError::SlippageExceeded {
            actual_out: quote.base_out_atoms,
            min_out: min_base_out_atoms,
        });
    }

    let mut remaining_quote = quote.net_quote_lamports as u128;
    let mut current_bin_id = quote.start_bin_id;

    while remaining_quote > 0 {
        let bin = bin_array.bin_mut(current_bin_id)?;
        if bin.reserve_base_atoms == 0 {
            current_bin_id += 1;
            continue;
        }
        let price_q64 = price_q64_from_sqrt_p(bin.sqrt_price_q64)?;
        let max_quote_for_bin = mul_div_u128(bin.reserve_base_atoms as u128, price_q64, Q64_ONE, true)?;
        if remaining_quote <= max_quote_for_bin {
            let base_bought = mul_div_u128(remaining_quote, Q64_ONE, price_q64, false)?
                .min(bin.reserve_base_atoms as u128);
            let quote_added = u64::try_from(remaining_quote).map_err(|_| RatchetError::MathOverflow)?;
            let base_taken = u64::try_from(base_bought).map_err(|_| RatchetError::MathOverflow)?;
            bin.reserve_quote_lamports = bin
                .reserve_quote_lamports
                .checked_add(quote_added)
                .ok_or(RatchetError::MathOverflow)?;
            bin.reserve_base_atoms = bin
                .reserve_base_atoms
                .checked_sub(base_taken)
                .ok_or(RatchetError::InsufficientBinLiquidity)?;
            remaining_quote = 0;
        } else {
            let quote_added =
                u64::try_from(max_quote_for_bin).map_err(|_| RatchetError::MathOverflow)?;
            bin.reserve_quote_lamports = bin
                .reserve_quote_lamports
                .checked_add(quote_added)
                .ok_or(RatchetError::MathOverflow)?;
            bin.reserve_base_atoms = 0;
            remaining_quote -= max_quote_for_bin;
            if remaining_quote > 0 {
                current_bin_id += 1;
            }
        }
    }

    pool.v_a_up = pool.v_a_up.saturating_add(quote.upward_crossings);
    pool.active_bin_id = quote.end_bin_id;
    bin_array.active_bin_id = quote.end_bin_id;
    update_high_water_mark(pool, quote.end_sqrt_p_q64);

    pool.real_vault_lamports = pool
        .real_vault_lamports
        .checked_add(quote.quote_in_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    pool.max_extractable_lamports = pool
        .max_extractable_lamports
        .checked_add(quote.net_quote_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    pool.protocol_fee_lamports = pool
        .protocol_fee_lamports
        .checked_add(quote.fee_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    pool.total_volume_quote = pool
        .total_volume_quote
        .checked_add(quote.quote_in_lamports)
        .ok_or(RatchetError::MathOverflow)?;

    Ok(quote)
}

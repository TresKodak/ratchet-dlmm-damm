use crate::error::RatchetError;
use crate::math::{div_ceil_u128, BPS_DENOMINATOR};
use crate::state::{ActiveBinArrayPda, RatchetPoolPda, DEFAULT_COMPOUNDING_FEE_BPS};

/// Combined bps denominator for `compounding_fee_bps * f_sell_bps` (`10_000 * 10_000 = 10^8`).
pub const COMPOUNDING_DENOMINATOR: u128 = 100_000_000;

/// Exact lamport-conserving fee split for OnlyB quote auto-compounding.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SellCompoundingSplit {
    /// Gross SOL lamports extracted from traversed liquidity bins before fees.
    pub gross_sol_out_lamports: u64,
    /// Total sell fee withheld from `gross_sol_out_lamports`.
    pub total_fee_lamports: u64,
    /// Portion routed directly into the active floor bin (`floor(compounding_bps * f_sell * Gross_SOL_Out)`).
    pub compounded_sol_lamports: u64,
    /// Remaining fee routed to protocol/creator fee accounting (`total_fee - compounded_sol`).
    pub protocol_fee_lamports: u64,
    /// Net SOL lamports delivered to the seller (`gross_sol_out - total_fee`).
    pub net_user_sol_out_lamports: u64,
}

/// Settlement receipt returned after routing compounded SOL into the active floor bin.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SellCompoundingReceipt {
    pub split: SellCompoundingSplit,
    pub floor_bin_id: i32,
    pub floor_bin_quote_before: u64,
    pub floor_bin_quote_after: u64,
    /// Conservation verification left-hand side (`real_vault_before`).
    pub conservation_lhs: u128,
    /// Conservation verification right-hand side (`real_vault_after + net_user_sol_out`).
    pub conservation_rhs: u128,
}

/// Computes `Delta_R_sol_compounded = floor((compounding_fee_bps / 10^4) * (f_sell_bps / 10^4) * Gross_SOL_Out)`
/// and verifies 0-lamport conservation (`net_user + compounded + protocol == gross_sol_out`).
pub fn compute_sell_compounding_split(
    gross_sol_out_lamports: u64,
    f_sell_bps: u16,
    compounding_fee_bps: u16,
) -> Result<SellCompoundingSplit, RatchetError> {
    if f_sell_bps > 2_500 {
        return Err(RatchetError::InvalidFeeConfig);
    }
    if !(DEFAULT_COMPOUNDING_FEE_BPS..=10_000).contains(&compounding_fee_bps) {
        return Err(RatchetError::InvalidFeeConfig);
    }

    let gross = gross_sol_out_lamports as u128;
    let total_fee = if gross == 0 || f_sell_bps == 0 {
        0
    } else {
        div_ceil_u128(gross * (f_sell_bps as u128), BPS_DENOMINATOR)?.min(gross)
    };

    // Exact floor(0.75 * f_sell(t) * Gross_SOL_Out) when compounding_fee_bps == 7500.
    let compounded = (gross * (f_sell_bps as u128) * (compounding_fee_bps as u128))
        / COMPOUNDING_DENOMINATOR;
    let protocol_fee = total_fee
        .checked_sub(compounded)
        .ok_or(RatchetError::MathOverflow)?;
    let net_user_out = gross
        .checked_sub(total_fee)
        .ok_or(RatchetError::MathOverflow)?;

    let split = SellCompoundingSplit {
        gross_sol_out_lamports,
        total_fee_lamports: u64::try_from(total_fee).map_err(|_| RatchetError::MathOverflow)?,
        compounded_sol_lamports: u64::try_from(compounded).map_err(|_| RatchetError::MathOverflow)?,
        protocol_fee_lamports: u64::try_from(protocol_fee).map_err(|_| RatchetError::MathOverflow)?,
        net_user_sol_out_lamports: u64::try_from(net_user_out)
            .map_err(|_| RatchetError::MathOverflow)?,
    };

    let lhs = gross;
    let rhs = split.net_user_sol_out_lamports as u128
        + split.compounded_sol_lamports as u128
        + split.protocol_fee_lamports as u128;
    if lhs != rhs {
        return Err(RatchetError::ConservationViolation { lhs, rhs });
    }

    Ok(split)
}

/// Routes `Delta_R_sol_compounded` directly into the active floor bin's quote reserve in the
/// same swap instruction and updates pool vault accounting with 0-lamport conservation drift (`LHS == RHS`).
pub fn route_compounded_sol_to_floor_bin(
    pool: &mut RatchetPoolPda,
    bin_array: &mut ActiveBinArrayPda,
    gross_sol_out_lamports: u64,
    f_sell_bps: u16,
) -> Result<SellCompoundingReceipt, RatchetError> {
    pool.validate()?;
    let split = compute_sell_compounding_split(
        gross_sol_out_lamports,
        f_sell_bps,
        pool.compounding_fee_bps,
    )?;

    let floor_bin_id = bin_array.floor_bin_id;
    let floor_bin = bin_array.floor_bin_mut()?;
    let floor_bin_quote_before = floor_bin.reserve_quote_lamports;
    let floor_bin_quote_after = floor_bin_quote_before
        .checked_add(split.compounded_sol_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    floor_bin.reserve_quote_lamports = floor_bin_quote_after;

    let vault_before = pool.real_vault_lamports as u128;
    let vault_after = pool
        .real_vault_lamports
        .checked_sub(split.net_user_sol_out_lamports)
        .ok_or(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: pool.real_vault_lamports,
        })?;
    pool.real_vault_lamports = vault_after;

    // Update max extractable reserves identically to net bin quote change (-gross + compounded).
    pool.max_extractable_lamports = pool
        .max_extractable_lamports
        .saturating_sub(gross_sol_out_lamports)
        .checked_add(split.compounded_sol_lamports)
        .ok_or(RatchetError::MathOverflow)?;

    if pool.max_extractable_lamports > pool.real_vault_lamports {
        return Err(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: pool.real_vault_lamports,
        });
    }

    pool.compounded_sol_total = pool
        .compounded_sol_total
        .checked_add(split.compounded_sol_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    pool.protocol_fee_lamports = pool
        .protocol_fee_lamports
        .checked_add(split.protocol_fee_lamports)
        .ok_or(RatchetError::MathOverflow)?;

    let conservation_lhs = vault_before;
    let conservation_rhs = (vault_after as u128) + (split.net_user_sol_out_lamports as u128);
    if conservation_lhs != conservation_rhs {
        return Err(RatchetError::ConservationViolation {
            lhs: conservation_lhs,
            rhs: conservation_rhs,
        });
    }

    Ok(SellCompoundingReceipt {
        split,
        floor_bin_id,
        floor_bin_quote_before,
        floor_bin_quote_after,
        conservation_lhs,
        conservation_rhs,
    })
}

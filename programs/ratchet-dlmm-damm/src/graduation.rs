use crate::error::RatchetError;
use crate::state::{
    ActiveBinArrayPda, RatchetPoolPda, DEFAULT_COMPOUNDING_FEE_BPS, DEFAULT_F_MAX_SELL_BPS,
    GRADUATION_STATUS_ACTIVE_CURVE, GRADUATION_STATUS_GRADUATED_IN_PLACE,
    HARD_SELL_IMPACT_CAP_BPS,
};

/// Solvency verification report proving `max_extractable_lamports <= real_vault_lamports` (Gate G06).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SolvencyReport {
    pub real_vault_lamports: u64,
    pub max_extractable_lamports: u64,
    pub active_bins_quote_lamports: u64,
    pub surplus_lamports: u64,
}

/// Receipt emitted upon atomic zero-dead-zone in-place graduation (Gate G10).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GraduationReceipt {
    pub previous_status: u8,
    pub new_status: u8,
    pub graduation_slot: u64,
    /// Always `0`: in-place state flip introduces zero halted/dead-zone slots and no `PartialMigrationError`.
    pub dead_zone_slots: u64,
    pub solvency: SolvencyReport,
}

/// Enforces Gate G06 solvency (`max_extractable_lamports <= real_vault_lamports` and bin backing).
pub fn verify_solvency(
    pool: &RatchetPoolPda,
    bin_array: &ActiveBinArrayPda,
) -> Result<SolvencyReport, RatchetError> {
    if pool.max_extractable_lamports > pool.real_vault_lamports {
        return Err(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: pool.real_vault_lamports,
        });
    }
    let active_bins_quote_lamports = bin_array.total_quote_lamports()?;
    if active_bins_quote_lamports > pool.real_vault_lamports {
        return Err(RatchetError::InsolventPool {
            max_extractable: active_bins_quote_lamports,
            real_vault: pool.real_vault_lamports,
        });
    }
    let surplus_lamports = pool.real_vault_lamports - pool.max_extractable_lamports;
    Ok(SolvencyReport {
        real_vault_lamports: pool.real_vault_lamports,
        max_extractable_lamports: pool.max_extractable_lamports,
        active_bins_quote_lamports,
        surplus_lamports,
    })
}

/// Atomically graduates the pool in-place with zero dead-zone slots (Gate G10) and verified vault solvency (Gate G06).
///
/// Rather than migrating reserves via external CPI into a secondary AMM (which risks `PartialMigrationError`
/// and multi-slot liquidity locks), this flips `graduation_status` in-place from `0` to `1` and locks
/// immutable fee bounds inside a single instruction.
pub fn graduate_in_place(
    pool: &mut RatchetPoolPda,
    bin_array: &ActiveBinArrayPda,
    current_slot: u64,
) -> Result<GraduationReceipt, RatchetError> {
    pool.validate()?;
    if pool.graduation_status != GRADUATION_STATUS_ACTIVE_CURVE {
        return Err(RatchetError::AlreadyGraduated);
    }
    if pool.sqrt_p_current < pool.sqrt_p_graduation {
        return Err(RatchetError::GraduationThresholdNotReached);
    }
    if pool.compounding_fee_bps < DEFAULT_COMPOUNDING_FEE_BPS {
        return Err(RatchetError::InvalidFeeConfig);
    }
    if pool.f_max_sell_bps > DEFAULT_F_MAX_SELL_BPS {
        return Err(RatchetError::InvalidFeeConfig);
    }
    if pool.max_sell_impact_bps > HARD_SELL_IMPACT_CAP_BPS {
        return Err(RatchetError::InvalidFeeConfig);
    }

    let solvency = verify_solvency(pool, bin_array)?;
    let previous_status = pool.graduation_status;

    pool.graduation_status = GRADUATION_STATUS_GRADUATED_IN_PLACE;
    pool.graduation_slot = current_slot;
    if pool.authority_revoked == 0 {
        pool.authority_revoked = 1;
        pool.authority = [0u8; 32];
    }

    Ok(GraduationReceipt {
        previous_status,
        new_status: pool.graduation_status,
        graduation_slot: current_slot,
        dead_zone_slots: 0,
        solvency,
    })
}

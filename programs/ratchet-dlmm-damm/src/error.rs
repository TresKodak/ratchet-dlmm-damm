use core::fmt;

/// Error variants for Ratchet-DLMM-DAMM Phases 1-3 state, math, compounding, and graduation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RatchetError {
    /// Checked arithmetic overflow or underflow.
    MathOverflow,
    /// Attempted division by zero.
    DivisionByZero,
    /// Invalid bin step configuration (must be > 0 and <= 10_000 bps).
    InvalidBinStep,
    /// Invalid fee configuration (e.g., f_min > f_0, a_sell != 5 * a_buy, compounding < 7500, etc.).
    InvalidFeeConfig,
    /// Invalid general parameter or byte buffer length/alignment.
    InvalidParameter,
    /// Requested bin id is outside the active bin array window.
    InvalidBinIndex,
    /// Insufficient quote or base liquidity across available bins (bin depletion).
    InsufficientBinLiquidity,
    /// Single-leg sell price impact exceeds the buffered guardrail (700 bps inside 750 bps hard cap).
    SellImpactExceedsCap { impact_bps: u16, max_bps: u16 },
    /// Swap output fell below caller's minimum slippage threshold.
    SlippageExceeded { actual_out: u64, min_out: u64 },
    /// Vault solvency invariant violated (`max_extractable_lamports > real_vault_lamports`).
    InsolventPool { max_extractable: u64, real_vault: u64 },
    /// Pool has already graduated in-place.
    AlreadyGraduated,
    /// Pool has not yet reached the graduation sqrt(P) threshold.
    GraduationThresholdNotReached,
    /// Operation rejected because mutable pool authority was permanently revoked.
    AuthorityRevoked,
    /// Swap input amount must be strictly positive.
    ZeroSwapAmount,
    /// Caller does not match the configured Token-2022 withdraw withheld authority.
    TransferFeeAuthorityMismatch,
    /// Conservation invariant failed (`LHS != RHS`).
    ConservationViolation { lhs: u128, rhs: u128 },
}

impl fmt::Display for RatchetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MathOverflow => write!(f, "checked arithmetic overflow"),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::InvalidBinStep => write!(f, "invalid bin step bps"),
            Self::InvalidFeeConfig => write!(f, "invalid fee configuration"),
            Self::InvalidParameter => write!(f, "invalid parameter or account buffer"),
            Self::InvalidBinIndex => write!(f, "bin id out of bounds for ActiveBinArrayPda"),
            Self::InsufficientBinLiquidity => write!(f, "insufficient discrete bin liquidity"),
            Self::SellImpactExceedsCap { impact_bps, max_bps } => {
                write!(
                    f,
                    "sell price impact {} bps exceeds guardrail {} bps",
                    impact_bps, max_bps
                )
            }
            Self::SlippageExceeded { actual_out, min_out } => {
                write!(f, "slippage exceeded: out {} < min {}", actual_out, min_out)
            }
            Self::InsolventPool {
                max_extractable,
                real_vault,
            } => write!(
                f,
                "insolvent pool: max_extractable {} > real_vault {}",
                max_extractable, real_vault
            ),
            Self::AlreadyGraduated => write!(f, "pool already graduated in-place"),
            Self::GraduationThresholdNotReached => {
                write!(f, "graduation price threshold not reached")
            }
            Self::AuthorityRevoked => write!(f, "pool authority permanently revoked"),
            Self::ZeroSwapAmount => write!(f, "swap amount must be non-zero"),
            Self::TransferFeeAuthorityMismatch => {
                write!(f, "transfer fee withdraw authority mismatch")
            }
            Self::ConservationViolation { lhs, rhs } => {
                write!(f, "lamport conservation violation: lhs {} != rhs {}", lhs, rhs)
            }
        }
    }
}

impl std::error::Error for RatchetError {}

impl From<RatchetError> for solana_program::program_error::ProgramError {
    fn from(err: RatchetError) -> Self {
        let code: u32 = match err {
            RatchetError::MathOverflow => 6000,
            RatchetError::DivisionByZero => 6001,
            RatchetError::InvalidBinStep => 6002,
            RatchetError::InvalidFeeConfig => 6003,
            RatchetError::InvalidParameter => 6004,
            RatchetError::InvalidBinIndex => 6005,
            RatchetError::InsufficientBinLiquidity => 6006,
            RatchetError::SellImpactExceedsCap { .. } => 6007,
            RatchetError::SlippageExceeded { .. } => 6008,
            RatchetError::InsolventPool { .. } => 6009,
            RatchetError::AlreadyGraduated => 6010,
            RatchetError::GraduationThresholdNotReached => 6011,
            RatchetError::AuthorityRevoked => 6012,
            RatchetError::ZeroSwapAmount => 6013,
            RatchetError::TransferFeeAuthorityMismatch => 6014,
            RatchetError::ConservationViolation { .. } => 6015,
        };
        solana_program::program_error::ProgramError::Custom(code)
    }
}

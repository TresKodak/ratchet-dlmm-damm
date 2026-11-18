use bytemuck::{Pod, Zeroable};

use crate::error::RatchetError;

/// SIMD-0437 per-account metadata overhead in bytes.
pub const SIMD0437_ACCOUNT_OVERHEAD_BYTES: usize = 128;

/// Exact data byte size of `RatchetPoolPda`.
pub const RATCHET_POOL_PDA_DATA_BYTES: usize = 480;
/// Total billed bytes for `RatchetPoolPda` under SIMD-0437 (480 + 128).
pub const RATCHET_POOL_PDA_TOTAL_BYTES: usize =
    RATCHET_POOL_PDA_DATA_BYTES + SIMD0437_ACCOUNT_OVERHEAD_BYTES;

/// Exact data byte size of `ActiveBinArrayPda`.
pub const ACTIVE_BIN_ARRAY_PDA_DATA_BYTES: usize = 1_024;
/// Total billed bytes for `ActiveBinArrayPda` under SIMD-0437 (1,024 + 128).
pub const ACTIVE_BIN_ARRAY_PDA_TOTAL_BYTES: usize =
    ACTIVE_BIN_ARRAY_PDA_DATA_BYTES + SIMD0437_ACCOUNT_OVERHEAD_BYTES;

/// SIMD-0437 Step 2 rent rate in lamports per byte (5,080 L/B).
pub const SIMD0437_STEP2_LAMPORTS_PER_BYTE: u64 = 5_080;
/// SIMD-0437 Step 5 rent rate in lamports per byte (696 L/B).
pub const SIMD0437_STEP5_LAMPORTS_PER_BYTE: u64 = 696;

/// Default share of sell fee auto-compounded into the active floor bin (75.00%).
pub const DEFAULT_COMPOUNDING_FEE_BPS: u16 = 7_500;
/// Default Slot-0 initial buy fee ceiling in bps (80.00%).
pub const DEFAULT_F_0_BPS: u16 = 8_000;
/// Default minimum decaying fee floor in bps (0.25%).
pub const DEFAULT_F_MIN_BPS: u16 = 25;
/// Immutable sell fee hard ceiling in bps (25.00%).
pub const DEFAULT_F_MAX_SELL_BPS: u16 = 2_500;
/// Buffered single-leg sell price impact cap in bps (7.00%, inside 7.50% hard cap).
pub const DEFAULT_MAX_SELL_IMPACT_BPS: u16 = 700;
/// Protocol-wide hard ceiling for single-leg sell price impact in bps (7.50%).
pub const HARD_SELL_IMPACT_CAP_BPS: u16 = 750;
/// Default market-cap ratchet decay multiplier per step in bps (0.90 = 9000 bps).
pub const DEFAULT_GAMMA_BPS: u16 = 9_000;

/// Default real quote SOL vault balance at graduation solvency benchmark (86.5 SOL).
pub const DEFAULT_REAL_VAULT_LAMPORTS: u64 = 86_500_000_000;
/// Default max extractable SOL bound at graduation solvency benchmark (85.0 SOL).
pub const DEFAULT_MAX_EXTRACTABLE_LAMPORTS: u64 = 85_000_000_000;

/// Default DLMM filter window before volatility accumulator starts decaying (seconds).
pub const DEFAULT_FILTER_PERIOD_SECONDS: u8 = 30;
/// Default DLMM full decay horizon for volatility accumulator (seconds).
pub const DEFAULT_DECAY_PERIOD_SECONDS: u32 = 600;
/// Default reduction factor applied per elapsed decay interval (50.00%).
pub const DEFAULT_REDUCTION_FACTOR_BPS: u16 = 5_000;

/// Number of packed discrete liquidity bins stored in `ActiveBinArrayPda`.
pub const ACTIVE_BIN_ARRAY_LEN: usize = 30;

/// Pool graduation status: active pre-graduation bonding curve phase.
pub const GRADUATION_STATUS_ACTIVE_CURVE: u8 = 0;
/// Pool graduation status: atomically graduated in-place to DAMM steady state.
pub const GRADUATION_STATUS_GRADUATED_IN_PLACE: u8 = 1;

/// Current PDA state version tag.
pub const STATE_VERSION: u8 = 1;

/// Q64.64 unit scalar (`1 << 64`).
pub const Q64_ONE: u128 = 1u128 << 64;

/// Zero-copy primary pool state PDA (`480` data bytes, `608` total bytes with 128B SIMD-0437 overhead).
///
/// Field ordering guarantees strict natural alignment and zero implicit compiler padding so
/// `bytemuck::Pod` casts directly from account data slices are sound.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct RatchetPoolPda {
    /// Monotonic high-water `sqrt(P)` in Q64.64 fixed point (`0..16`).
    pub sqrt_p_high_water: u128,
    /// Current pool `sqrt(P)` in Q64.64 fixed point (`16..32`).
    pub sqrt_p_current: u128,
    /// Genesis Slot-0 baseline `sqrt(P)` in Q64.64 fixed point (`32..48`).
    pub sqrt_p_start: u128,
    /// Target `sqrt(P)` threshold enabling atomic in-place graduation (`48..64`).
    pub sqrt_p_graduation: u128,

    /// Upward directional volatility accumulator (bin crossings) (`64..72`).
    pub v_a_up: u64,
    /// Downward directional volatility accumulator (bin crossings) (`72..80`).
    pub v_a_down: u64,
    /// Buy-side volatility fee coefficient `A_buy` (`80..88`).
    pub a_buy: u64,
    /// Sell-side dump-brake fee coefficient `A_sell = 5 * A_buy` (`88..96`).
    pub a_sell: u64,
    /// Unix timestamp of the last volatility accumulator decay update (`96..104`).
    pub last_decay_timestamp: i64,
    /// Actual physical SOL lamports held in the pool quote vault (`104..112`).
    pub real_vault_lamports: u64,
    /// Maximum SOL lamports extractable by holders against bin reserves (`112..120`).
    pub max_extractable_lamports: u64,
    /// Cumulative quote SOL lamports auto-compounded into the active floor bin (`120..128`).
    pub compounded_sol_total: u64,
    /// Accumulated non-compounded protocol/creator SOL fee lamports (`128..136`).
    pub protocol_fee_lamports: u64,
    /// Slot number at which in-place graduation occurred (`0` before graduation) (`136..144`).
    pub graduation_slot: u64,
    /// Cumulative quote volume in lamports (`144..152`).
    pub total_volume_quote: u64,
    /// Token-2022 `TransferFeeConfig` fallback fee atoms harvested out-of-band (`152..160`).
    pub harvested_transfer_fee_atoms: u64,

    /// Current active discrete bin id (`160..164`).
    pub active_bin_id: i32,
    /// Active floor bin id receiving OnlyB auto-compounded quote reserves (`164..168`).
    pub floor_bin_id: i32,
    /// Starting bin id mapped to `ActiveBinArrayPda.bins[0]` (`168..172`).
    pub bin_array_start_id: i32,
    /// Horizon in seconds after which directional accumulators fully decay (`172..176`).
    pub decay_period_seconds: u32,

    /// Share of sell fee routed back into the active floor bin in bps (default `7500`) (`176..178`).
    pub compounding_fee_bps: u16,
    /// Initial Slot-0 base buy fee in bps (`8000` = 80%) (`178..180`).
    pub f_0_bps: u16,
    /// Minimum steady-state fee floor in bps (`25` = 0.25%) (`180..182`).
    pub f_min_bps: u16,
    /// Immutable maximum sell fee ceiling in bps (`2500` = 25%) (`182..184`).
    pub f_max_sell_bps: u16,
    /// Discrete price step size `s` between adjacent bins in bps (`184..186`).
    pub bin_step_bps: u16,
    /// Buffered single-leg sell price impact limit in bps (default `700` <= `750`) (`186..188`).
    pub max_sell_impact_bps: u16,
    /// High-water ratchet decay multiplier `gamma` per step in bps (e.g. `9000`) (`188..190`).
    pub gamma_bps: u16,
    /// Fallback Token-2022 `TransferFeeConfig` fee rate in bps (`190..192`).
    pub transfer_fee_bps: u16,

    /// `0` = active bonding curve, `1` = graduated in-place (`192..193`).
    pub graduation_status: u8,
    /// `0` = mutable genesis authority, `1` = permanently revoked (`193..194`).
    pub authority_revoked: u8,
    /// PDA bump seed for quote/base vaults (`194..195`).
    pub vault_bump: u8,
    /// PDA bump seed for `RatchetPoolPda` (`195..196`).
    pub pool_bump: u8,
    /// PDA bump seed for `ActiveBinArrayPda` (`196..197`).
    pub bin_array_bump: u8,
    /// State layout version (`197..198`).
    pub version: u8,
    /// `1` if Token-2022 `TransferFeeConfig` fallback harvesting is active (`198..199`).
    pub transfer_fee_enabled: u8,
    /// Filter window in seconds before accumulator decay engages (`199..200`).
    pub filter_period_seconds: u8,

    /// Base Token-2022 mint pubkey (`200..232`).
    pub base_mint: [u8; 32],
    /// Quote mint pubkey (e.g., WSOL) (`232..264`).
    pub quote_mint: [u8; 32],
    /// Pool base token vault pubkey (`264..296`).
    pub base_vault: [u8; 32],
    /// Pool quote SOL vault pubkey (`296..328`).
    pub quote_vault: [u8; 32],
    /// Pool authority pubkey (zeroed once revoked) (`328..360`).
    pub authority: [u8; 32],
    /// Linked `ActiveBinArrayPda` pubkey (`360..392`).
    pub active_bin_array: [u8; 32],
    /// Creator/protocol fee destination pubkey (`392..424`).
    pub creator_fee_vault: [u8; 32],

    /// Explicit zero padding to exactly 480 bytes (`424..480`).
    pub padding: [u8; 56],
}

const _: () = assert!(core::mem::size_of::<RatchetPoolPda>() == RATCHET_POOL_PDA_DATA_BYTES);

impl RatchetPoolPda {
    /// Creates a new `RatchetPoolPda` initialized with canonical protocol safety defaults.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_mint: [u8; 32],
        quote_mint: [u8; 32],
        base_vault: [u8; 32],
        quote_vault: [u8; 32],
        authority: [u8; 32],
        active_bin_array: [u8; 32],
        creator_fee_vault: [u8; 32],
        sqrt_p_start: u128,
        sqrt_p_graduation: u128,
        a_buy: u64,
        bin_step_bps: u16,
        active_bin_id: i32,
        floor_bin_id: i32,
        bin_array_start_id: i32,
        pool_bump: u8,
        vault_bump: u8,
        bin_array_bump: u8,
        initial_timestamp: i64,
    ) -> Result<Self, RatchetError> {
        if bin_step_bps == 0 || bin_step_bps > 10_000 {
            return Err(RatchetError::InvalidBinStep);
        }
        if sqrt_p_start == 0 || sqrt_p_graduation < sqrt_p_start {
            return Err(RatchetError::InvalidParameter);
        }
        let a_sell = a_buy.checked_mul(5).ok_or(RatchetError::MathOverflow)?;

        let pool = Self {
            sqrt_p_high_water: sqrt_p_start,
            sqrt_p_current: sqrt_p_start,
            sqrt_p_start,
            sqrt_p_graduation,
            v_a_up: 0,
            v_a_down: 0,
            a_buy,
            a_sell,
            last_decay_timestamp: initial_timestamp,
            real_vault_lamports: DEFAULT_REAL_VAULT_LAMPORTS,
            max_extractable_lamports: DEFAULT_MAX_EXTRACTABLE_LAMPORTS,
            compounded_sol_total: 0,
            protocol_fee_lamports: 0,
            graduation_slot: 0,
            total_volume_quote: 0,
            harvested_transfer_fee_atoms: 0,
            active_bin_id,
            floor_bin_id,
            bin_array_start_id,
            decay_period_seconds: DEFAULT_DECAY_PERIOD_SECONDS,
            compounding_fee_bps: DEFAULT_COMPOUNDING_FEE_BPS,
            f_0_bps: DEFAULT_F_0_BPS,
            f_min_bps: DEFAULT_F_MIN_BPS,
            f_max_sell_bps: DEFAULT_F_MAX_SELL_BPS,
            bin_step_bps,
            max_sell_impact_bps: DEFAULT_MAX_SELL_IMPACT_BPS,
            gamma_bps: DEFAULT_GAMMA_BPS,
            transfer_fee_bps: DEFAULT_F_MIN_BPS,
            graduation_status: GRADUATION_STATUS_ACTIVE_CURVE,
            authority_revoked: 0,
            vault_bump,
            pool_bump,
            bin_array_bump,
            version: STATE_VERSION,
            transfer_fee_enabled: 1,
            filter_period_seconds: DEFAULT_FILTER_PERIOD_SECONDS,
            base_mint,
            quote_mint,
            base_vault,
            quote_vault,
            authority,
            active_bin_array,
            creator_fee_vault,
            padding: [0u8; 56],
        };
        pool.validate()?;
        Ok(pool)
    }

    /// Zero-copy immutable borrow from a 16-byte-aligned account data buffer.
    pub fn try_from_bytes(data: &[u8]) -> Result<&Self, RatchetError> {
        if data.len() != RATCHET_POOL_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_from_bytes::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Zero-copy mutable borrow from a 16-byte-aligned account data buffer.
    pub fn try_from_bytes_mut(data: &mut [u8]) -> Result<&mut Self, RatchetError> {
        if data.len() != RATCHET_POOL_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_from_bytes_mut::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Safe Pod decode from an arbitrarily aligned 480-byte account slice.
    pub fn from_bytes_unaligned(data: &[u8]) -> Result<Self, RatchetError> {
        if data.len() != RATCHET_POOL_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_pod_read_unaligned::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Raw zero-copy byte representation of this PDA.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    /// Validates core protocol invariants on fee limits, dump-brake asymmetry, impact caps, and solvency.
    pub fn validate(&self) -> Result<(), RatchetError> {
        if self.bin_step_bps == 0 || self.bin_step_bps > 10_000 {
            return Err(RatchetError::InvalidBinStep);
        }
        if self.f_min_bps > self.f_0_bps || self.f_0_bps > 10_000 {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if self.f_max_sell_bps > DEFAULT_F_MAX_SELL_BPS || self.f_min_bps > self.f_max_sell_bps {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if self.compounding_fee_bps < DEFAULT_COMPOUNDING_FEE_BPS || self.compounding_fee_bps > 10_000 {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if self.max_sell_impact_bps > HARD_SELL_IMPACT_CAP_BPS {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if self.gamma_bps > 10_000 {
            return Err(RatchetError::InvalidFeeConfig);
        }
        let expected_a_sell = self.a_buy.checked_mul(5).ok_or(RatchetError::MathOverflow)?;
        if self.a_sell != expected_a_sell {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if self.max_extractable_lamports > self.real_vault_lamports {
            return Err(RatchetError::InsolventPool {
                max_extractable: self.max_extractable_lamports,
                real_vault: self.real_vault_lamports,
            });
        }
        Ok(())
    }

    /// Permanently revokes the mutable genesis authority and zeroes the authority pubkey.
    pub fn revoke_authority(&mut self) -> Result<(), RatchetError> {
        if self.authority_revoked != 0 {
            return Err(RatchetError::AuthorityRevoked);
        }
        self.authority_revoked = 1;
        self.authority = [0u8; 32];
        Ok(())
    }

    /// Returns `true` if the pool has graduated in-place.
    pub fn is_graduated(&self) -> bool {
        self.graduation_status == GRADUATION_STATUS_GRADUATED_IN_PLACE
    }

    /// Stores the derived `PoolAuthorityPda` bump in reserved padding (`padding[0..2]`)
    /// so hot-path swaps validate via `create_program_address` without `find_program_address` CU overhead.
    pub fn set_pool_authority_bump(&mut self, bump: u8) {
        self.padding[0] = bump;
        self.padding[1] = 1;
    }

    /// Returns the cached `PoolAuthorityPda` bump if set at pool initialization.
    pub fn pool_authority_bump(&self) -> Option<u8> {
        if self.padding[1] == 1 {
            Some(self.padding[0])
        } else {
            None
        }
    }
}

/// Single discrete liquidity bin entry (`32` bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct LiquidityBin {
    /// Fixed bin `sqrt(P)` in Q64.64 fixed point (`0..16`).
    pub sqrt_price_q64: u128,
    /// Quote SOL lamports held in this bin (`16..24`).
    pub reserve_quote_lamports: u64,
    /// Base token atoms held in this bin (`24..32`).
    pub reserve_base_atoms: u64,
}

const _: () = assert!(core::mem::size_of::<LiquidityBin>() == 32);

/// Zero-copy packed discrete liquidity bin array PDA (`1,024` data bytes, `1,152` total bytes with 128B SIMD-0437 overhead).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct ActiveBinArrayPda {
    /// Multiplicative Q64.64 step `sqrt(1 + bin_step_bps / 10_000)` (`0..16`).
    pub bin_step_q64: u128,
    /// Parent `RatchetPoolPda` pubkey (`16..48`).
    pub pool: [u8; 32],
    /// Current active bin index (`48..52`).
    pub active_bin_id: i32,
    /// Active floor bin index targeted by OnlyB quote auto-compounding (`52..56`).
    pub floor_bin_id: i32,
    /// Lowest bin index represented by `bins[0]` (`56..60`).
    pub start_bin_id: i32,
    /// Bin step size in bps (`60..62`).
    pub bin_step_bps: u16,
    /// PDA bump seed (`62..63`).
    pub bump: u8,
    /// State layout version (`63..64`).
    pub version: u8,
    /// Packed discrete liquidity bins (`30 * 32 = 960` bytes, `64..1024`).
    pub bins: [LiquidityBin; ACTIVE_BIN_ARRAY_LEN],
}

const _: () = assert!(core::mem::size_of::<ActiveBinArrayPda>() == ACTIVE_BIN_ARRAY_PDA_DATA_BYTES);

impl ActiveBinArrayPda {
    /// Constructs a new `ActiveBinArrayPda` and seeds each bin's Q64.64 `sqrt_price_q64`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: [u8; 32],
        start_bin_id: i32,
        active_bin_id: i32,
        floor_bin_id: i32,
        bin_step_bps: u16,
        bin_step_q64: u128,
        start_sqrt_price_q64: u128,
        bump: u8,
    ) -> Result<Self, RatchetError> {
        if bin_step_bps == 0 || bin_step_bps > 10_000 || bin_step_q64 <= Q64_ONE || start_sqrt_price_q64 == 0 {
            return Err(RatchetError::InvalidBinStep);
        }
        let end_bin_id = start_bin_id
            .checked_add(ACTIVE_BIN_ARRAY_LEN as i32)
            .ok_or(RatchetError::MathOverflow)?;
        if active_bin_id < start_bin_id || active_bin_id >= end_bin_id {
            return Err(RatchetError::InvalidBinIndex);
        }
        if floor_bin_id < start_bin_id || floor_bin_id > active_bin_id {
            return Err(RatchetError::InvalidBinIndex);
        }

        let mut bins = [LiquidityBin::default(); ACTIVE_BIN_ARRAY_LEN];
        let mut current_sqrt_p = start_sqrt_price_q64;
        for (idx, bin) in bins.iter_mut().enumerate() {
            bin.sqrt_price_q64 = current_sqrt_p;
            if idx + 1 < ACTIVE_BIN_ARRAY_LEN {
                current_sqrt_p = crate::math::q64_mul_round(current_sqrt_p, bin_step_q64)?;
            }
        }

        Ok(Self {
            bin_step_q64,
            pool,
            active_bin_id,
            floor_bin_id,
            start_bin_id,
            bin_step_bps,
            bump,
            version: STATE_VERSION,
            bins,
        })
    }

    /// Zero-copy immutable borrow from a 16-byte-aligned account data buffer.
    pub fn try_from_bytes(data: &[u8]) -> Result<&Self, RatchetError> {
        if data.len() != ACTIVE_BIN_ARRAY_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_from_bytes::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Zero-copy mutable borrow from a 16-byte-aligned account data buffer.
    pub fn try_from_bytes_mut(data: &mut [u8]) -> Result<&mut Self, RatchetError> {
        if data.len() != ACTIVE_BIN_ARRAY_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_from_bytes_mut::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Safe Pod decode from an arbitrarily aligned 1,024-byte account slice.
    pub fn from_bytes_unaligned(data: &[u8]) -> Result<Self, RatchetError> {
        if data.len() != ACTIVE_BIN_ARRAY_PDA_DATA_BYTES {
            return Err(RatchetError::InvalidParameter);
        }
        bytemuck::try_pod_read_unaligned::<Self>(data).map_err(|_| RatchetError::InvalidParameter)
    }

    /// Raw zero-copy byte representation of this PDA.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    /// Resolves a global `bin_id` to an index in `self.bins`.
    pub fn bin_index(&self, bin_id: i32) -> Result<usize, RatchetError> {
        let offset = bin_id.checked_sub(self.start_bin_id).ok_or(RatchetError::InvalidBinIndex)?;
        if offset < 0 || (offset as usize) >= ACTIVE_BIN_ARRAY_LEN {
            return Err(RatchetError::InvalidBinIndex);
        }
        Ok(offset as usize)
    }

    /// Immutable reference to bin `bin_id`.
    pub fn bin(&self, bin_id: i32) -> Result<&LiquidityBin, RatchetError> {
        let idx = self.bin_index(bin_id)?;
        Ok(&self.bins[idx])
    }

    /// Mutable reference to bin `bin_id`.
    pub fn bin_mut(&mut self, bin_id: i32) -> Result<&mut LiquidityBin, RatchetError> {
        let idx = self.bin_index(bin_id)?;
        Ok(&mut self.bins[idx])
    }

    /// Immutable reference to the current active bin.
    pub fn active_bin(&self) -> Result<&LiquidityBin, RatchetError> {
        self.bin(self.active_bin_id)
    }

    /// Mutable reference to the active floor bin receiving auto-compounded quote reserves.
    pub fn floor_bin_mut(&mut self) -> Result<&mut LiquidityBin, RatchetError> {
        self.bin_mut(self.floor_bin_id)
    }

    /// Sums quote SOL lamports across all bins in this array with overflow checking.
    pub fn total_quote_lamports(&self) -> Result<u64, RatchetError> {
        let mut total: u64 = 0;
        for bin in &self.bins {
            total = total
                .checked_add(bin.reserve_quote_lamports)
                .ok_or(RatchetError::MathOverflow)?;
        }
        Ok(total)
    }

    /// Sums base token atoms across all bins in this array with overflow checking.
    pub fn total_base_atoms(&self) -> Result<u64, RatchetError> {
        let mut total: u64 = 0;
        for bin in &self.bins {
            total = total
                .checked_add(bin.reserve_base_atoms)
                .ok_or(RatchetError::MathOverflow)?;
        }
        Ok(total)
    }
}

/// SPL Token-2022 extension discriminant subset needed for write-lock-free fallback harvesting.
#[repr(u16)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExtensionType {
    /// Uninitialized extension slot.
    Uninitialized = 0,
    /// Token-2022 `TransferFeeConfig` mint extension (`1`).
    TransferFeeConfig = 1,
    /// Token-2022 `TransferFeeAmount` token-account extension (`2`).
    TransferFeeAmount = 2,
}

/// Token-2022 epoch-scheduled transfer fee specification.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct TransferFee {
    /// Epoch from which this fee schedule becomes active.
    pub epoch: u64,
    /// Hard per-transfer fee cap in token atoms.
    pub maximum_fee: u64,
    /// Transfer fee rate in basis points (`0..=10_000`).
    pub transfer_fee_basis_points: u16,
    /// Explicit zero padding for natural `Pod` alignment.
    pub _padding: [u8; 6],
}

impl TransferFee {
    /// Serializes to the canonical 18-byte Token-2022 `TransferFee` wire layout.
    pub fn to_spl_wire_bytes(&self) -> [u8; 18] {
        let mut out = [0u8; 18];
        out[0..8].copy_from_slice(&self.epoch.to_le_bytes());
        out[8..16].copy_from_slice(&self.maximum_fee.to_le_bytes());
        out[16..18].copy_from_slice(&self.transfer_fee_basis_points.to_le_bytes());
        out
    }

    /// Computes ceiling transfer fee `min(maximum_fee, ceil(amount * bps / 10_000))`.
    pub fn calculate_fee(&self, pre_fee_amount: u64) -> Result<u64, RatchetError> {
        if self.transfer_fee_basis_points > 10_000 {
            return Err(RatchetError::InvalidFeeConfig);
        }
        if pre_fee_amount == 0 || self.transfer_fee_basis_points == 0 {
            return Ok(0);
        }
        let numerator = (pre_fee_amount as u128)
            .checked_mul(self.transfer_fee_basis_points as u128)
            .ok_or(RatchetError::MathOverflow)?;
        let raw_fee = crate::math::div_ceil_u128(numerator, 10_000)?;
        let capped = raw_fee.min(self.maximum_fee as u128);
        u64::try_from(capped).map_err(|_| RatchetError::MathOverflow)
    }
}

/// Token-2022 `ExtensionType::TransferFeeConfig` fallback harvesting metadata & helper.
///
/// Enables write-lock-free fee withholding on external token transfers without acquiring a write
/// lock on `RatchetPoolPda`, followed by permissionless harvest-to-mint and batched settlement.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TransferFeeConfig {
    /// Token-2022 extension type tag (`ExtensionType::TransferFeeConfig`).
    pub extension_type: ExtensionType,
    /// Authority permitted to update fee parameters (`[0; 32]` once revoked).
    pub transfer_fee_config_authority: [u8; 32],
    /// Authority permitted to withdraw harvested fees from the mint.
    pub withdraw_withheld_authority: [u8; 32],
    /// Total withheld base token atoms currently accumulated on the mint.
    pub withheld_amount: u64,
    /// Transfer fee parameters effective before `newer_transfer_fee.epoch`.
    pub older_transfer_fee: TransferFee,
    /// Transfer fee parameters effective at or after `newer_transfer_fee.epoch`.
    pub newer_transfer_fee: TransferFee,
}

impl TransferFeeConfig {
    /// Constructs a new `ExtensionType::TransferFeeConfig` fallback configuration.
    pub fn new_fallback(
        transfer_fee_config_authority: [u8; 32],
        withdraw_withheld_authority: [u8; 32],
        transfer_fee_basis_points: u16,
        maximum_fee: u64,
        epoch: u64,
    ) -> Result<Self, RatchetError> {
        if transfer_fee_basis_points > 10_000 {
            return Err(RatchetError::InvalidFeeConfig);
        }
        let fee = TransferFee {
            epoch,
            maximum_fee,
            transfer_fee_basis_points,
            _padding: [0u8; 6],
        };
        Ok(Self {
            extension_type: ExtensionType::TransferFeeConfig,
            transfer_fee_config_authority,
            withdraw_withheld_authority,
            withheld_amount: 0,
            older_transfer_fee: fee,
            newer_transfer_fee: fee,
        })
    }

    /// Permanently revokes `transfer_fee_config_authority` so fee caps can never be raised.
    pub fn revoke_config_authority(&mut self) {
        self.transfer_fee_config_authority = [0u8; 32];
    }

    /// Returns `true` if fee modification authority has been permanently revoked.
    pub fn is_immutable(&self) -> bool {
        self.transfer_fee_config_authority == [0u8; 32]
    }

    /// Selects the active `TransferFee` epoch schedule for `epoch`.
    pub fn active_fee_for_epoch(&self, epoch: u64) -> &TransferFee {
        if epoch >= self.newer_transfer_fee.epoch {
            &self.newer_transfer_fee
        } else {
            &self.older_transfer_fee
        }
    }

    /// Calculates fallback transfer fee atoms for `pre_fee_amount` at `epoch`.
    pub fn calculate_fee(&self, pre_fee_amount: u64, epoch: u64) -> Result<u64, RatchetError> {
        self.active_fee_for_epoch(epoch).calculate_fee(pre_fee_amount)
    }

    /// Permissionless, write-lock-free fallback harvest from a recipient token account to the mint.
    ///
    /// Notice this deliberately does not borrow `RatchetPoolPda`, eliminating pool write-lock contention.
    pub fn harvest_withheld_tokens_to_mint(
        &mut self,
        account_withheld_amount: &mut u64,
    ) -> Result<u64, RatchetError> {
        let harvested = *account_withheld_amount;
        if harvested == 0 {
            return Ok(0);
        }
        self.withheld_amount = self
            .withheld_amount
            .checked_add(harvested)
            .ok_or(RatchetError::MathOverflow)?;
        *account_withheld_amount = 0;
        Ok(harvested)
    }

    /// Batched withdrawal of mint-accumulated fallback fees into `pool.harvested_transfer_fee_atoms`.
    pub fn withdraw_withheld_tokens_from_mint(
        &mut self,
        pool: &mut RatchetPoolPda,
        signer: &[u8; 32],
    ) -> Result<u64, RatchetError> {
        if *signer != self.withdraw_withheld_authority || *signer == [0u8; 32] {
            return Err(RatchetError::TransferFeeAuthorityMismatch);
        }
        let amount = self.withheld_amount;
        pool.harvested_transfer_fee_atoms = pool
            .harvested_transfer_fee_atoms
            .checked_add(amount)
            .ok_or(RatchetError::MathOverflow)?;
        self.withheld_amount = 0;
        Ok(amount)
    }

    /// Serializes the canonical 108-byte SPL Token-2022 `TransferFeeConfig` TLV payload.
    pub fn to_spl_wire_bytes(&self) -> [u8; 108] {
        let mut out = [0u8; 108];
        out[0..32].copy_from_slice(&self.transfer_fee_config_authority);
        out[32..64].copy_from_slice(&self.withdraw_withheld_authority);
        out[64..72].copy_from_slice(&self.withheld_amount.to_le_bytes());
        out[72..90].copy_from_slice(&self.older_transfer_fee.to_spl_wire_bytes());
        out[90..108].copy_from_slice(&self.newer_transfer_fee.to_spl_wire_bytes());
        out
    }
}

/// SIMD-0437 rent schedule breakdown across the two protocol PDAs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Simd0437RentSchedule {
    /// Rate in lamports per byte (`5_080` at Step 2, `696` at Step 5).
    pub lamports_per_byte: u64,
    /// Rent lamports for `ratchet_pool_pda` (`480 + 128 = 608` billed bytes).
    pub ratchet_pool_pda_lamports: u64,
    /// Rent lamports for `active_bin_array_pda` (`1,024 + 128 = 1,152` billed bytes).
    pub active_bin_array_pda_lamports: u64,
    /// Combined rent lamports for both PDAs.
    pub total_lamports: u64,
}

/// Computes total billed bytes for an account under SIMD-0437 (`data_bytes + 128`).
pub fn simd0437_billed_bytes(data_bytes: usize) -> Result<usize, RatchetError> {
    data_bytes
        .checked_add(SIMD0437_ACCOUNT_OVERHEAD_BYTES)
        .ok_or(RatchetError::MathOverflow)
}

/// Computes SIMD-0437 rent in lamports as `(data_bytes + 128) * lamports_per_byte`.
pub fn simd0437_rent_lamports(data_bytes: usize, lamports_per_byte: u64) -> Result<u64, RatchetError> {
    let billed_bytes = u64::try_from(simd0437_billed_bytes(data_bytes)?)
        .map_err(|_| RatchetError::MathOverflow)?;
    billed_bytes
        .checked_mul(lamports_per_byte)
        .ok_or(RatchetError::MathOverflow)
}

/// Computes the SIMD-0437 rent breakdown for `RatchetPoolPda` and `ActiveBinArrayPda`.
pub fn calculate_pda_rent_schedule(lamports_per_byte: u64) -> Result<Simd0437RentSchedule, RatchetError> {
    let ratchet_pool_pda_lamports =
        simd0437_rent_lamports(core::mem::size_of::<RatchetPoolPda>(), lamports_per_byte)?;
    let active_bin_array_pda_lamports =
        simd0437_rent_lamports(core::mem::size_of::<ActiveBinArrayPda>(), lamports_per_byte)?;
    let total_lamports = ratchet_pool_pda_lamports
        .checked_add(active_bin_array_pda_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    Ok(Simd0437RentSchedule {
        lamports_per_byte,
        ratchet_pool_pda_lamports,
        active_bin_array_pda_lamports,
        total_lamports,
    })
}

/// Returns the canonical SIMD-0437 Step 2 rent schedule (`5,080 L/B` => `8,940,800 L`).
pub fn step2_rent_schedule() -> Result<Simd0437RentSchedule, RatchetError> {
    calculate_pda_rent_schedule(SIMD0437_STEP2_LAMPORTS_PER_BYTE)
}

/// Returns the canonical SIMD-0437 Step 5 rent schedule (`696 L/B` => `1,224,960 L`).
pub fn step5_rent_schedule() -> Result<Simd0437RentSchedule, RatchetError> {
    calculate_pda_rent_schedule(SIMD0437_STEP5_LAMPORTS_PER_BYTE)
}

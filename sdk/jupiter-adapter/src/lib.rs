//! Standalone zero-dependency Jupiter `Amm`-compatible adapter for **Ratchet-DLMM-DAMM**.
//!
//! - Parses the exact 480-byte `RatchetPoolPda` header and 1,024-byte `ActiveBinArrayPda`
//!   matching `programs/ratchet-dlmm-damm/src/state.rs` byte-for-byte on every slot update.
//! - Simulates exact Q64.64 quotes with high-water buy ratchet, 5x asymmetric dump-brake,
//!   75% OnlyB quote auto-compounding (`Delta_R_sol_compounded`), and 700 bps sell impact cap.
//! - Resolves the exact 28-account swap instruction metas within CPI depth 2 (<= 4) and
//!   SIMD-0339 255-account execution limits.

use std::collections::HashMap;
use std::fmt;

/// SIMD-0437 per-account metadata overhead in bytes.
pub const SIMD0437_ACCOUNT_OVERHEAD_BYTES: usize = 128;
/// Exact serialized size of `RatchetPoolPda`.
pub const RATCHET_POOL_PDA_DATA_BYTES: usize = 480;
/// Exact serialized size of `ActiveBinArrayPda`.
pub const ACTIVE_BIN_ARRAY_PDA_DATA_BYTES: usize = 1_024;
/// Number of discrete liquidity bins inside `ActiveBinArrayPda`.
pub const ACTIVE_BIN_ARRAY_LEN: usize = 30;

/// Basis points denominator (`10_000` = 100%).
pub const BPS_DENOMINATOR: u128 = 10_000;
/// Volatility surcharge denominator (`10^11`).
pub const VOLATILITY_DENOMINATOR: u128 = 100_000_000_000;
/// Q64.64 fixed-point unit (`2^64`).
pub const Q64_ONE: u128 = 1u128 << 64;

/// Protocol defaults and immutable ceilings.
pub const DEFAULT_COMPOUNDING_FEE_BPS: u16 = 7_500;
pub const DEFAULT_F_0_BPS: u16 = 8_000;
pub const DEFAULT_F_MIN_BPS: u16 = 25;
pub const IMMUTABLE_F_MAX_SELL_BPS: u16 = 2_500;
pub const BUFFERED_MAX_SELL_IMPACT_BPS: u16 = 700;
pub const HARD_MAX_SELL_IMPACT_BPS: u16 = 750;
pub const DEFAULT_GAMMA_BPS: u16 = 9_000;
pub const MAX_BUY_FEE_BPS: u16 = 9_900;

/// Gate G05 execution bounds.
pub const EXACT_SWAP_ACCOUNT_COUNT: usize = 28;
pub const RATCHET_SWAP_CPI_DEPTH: u8 = 2;
pub const MAX_CPI_DEPTH: u8 = 4;
pub const SIMD0339_MAX_ACCOUNTS: usize = 255;

/// Graduation status flags.
pub const GRADUATION_STATUS_ACTIVE_CURVE: u8 = 0;
pub const GRADUATION_STATUS_GRADUATED_IN_PLACE: u8 = 1;

/// Well-known Solana program and sysvar pubkeys (deterministic 32-byte constants).
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey([0u8; 32]);
pub const WSOL_MINT: Pubkey = Pubkey([
    0x06, 0x9b, 0x88, 0x57, 0xfe, 0xab, 0x81, 0x84, 0xfb, 0x68, 0x7f, 0x63, 0x46, 0x18, 0xc0, 0x35,
    0xda, 0xc4, 0x39, 0xdc, 0x1a, 0xeb, 0x3b, 0x55, 0x98, 0xa0, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x01,
]);
pub const TOKEN_2022_PROGRAM_ID: Pubkey = Pubkey([
    0x06, 0xdd, 0xf6, 0xe1, 0xee, 0x75, 0x8f, 0xde, 0x18, 0x42, 0x5d, 0xbc, 0xe4, 0x6c, 0xcd, 0xda,
    0xb6, 0x1a, 0xfc, 0x4d, 0x83, 0xb9, 0x0d, 0x27, 0xfe, 0xbd, 0xf9, 0x28, 0xd8, 0xa1, 0x8b, 0xfc,
]);
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = Pubkey([
    0x06, 0xdd, 0xf6, 0xe1, 0xd7, 0x65, 0xa1, 0x93, 0xd9, 0xcb, 0xe1, 0x46, 0xce, 0xeb, 0x79, 0xac,
    0x1c, 0xb4, 0x85, 0xed, 0x5f, 0x5b, 0x37, 0x91, 0x3a, 0x8c, 0xf5, 0x85, 0x7e, 0xff, 0x00, 0xa9,
]);
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey([
    0x8c, 0x97, 0x25, 0x8f, 0x4e, 0x24, 0x89, 0xf1, 0xbb, 0x3d, 0x10, 0x29, 0x14, 0x8e, 0x0d, 0x83,
    0x0b, 0x5a, 0x13, 0x99, 0xda, 0xff, 0x10, 0x84, 0x04, 0x8e, 0x7b, 0xd8, 0xdb, 0xe9, 0xf8, 0x59,
]);
pub const RENT_SYSVAR_ID: Pubkey = Pubkey([
    0x06, 0xa7, 0xd5, 0x17, 0x19, 0x2c, 0x5c, 0x51, 0x21, 0x8c, 0xc9, 0x4c, 0x3d, 0x4a, 0xf1, 0x7f,
    0x58, 0xda, 0xee, 0x08, 0x9b, 0xa1, 0xfd, 0x44, 0xe3, 0xdb, 0xd9, 0x8a, 0x00, 0x00, 0x00, 0x00,
]);
pub const CLOCK_SYSVAR_ID: Pubkey = Pubkey([
    0x06, 0xa7, 0xd5, 0x17, 0x18, 0xc7, 0x74, 0xc9, 0x28, 0x56, 0x63, 0x98, 0x69, 0x1d, 0x5e, 0xb6,
    0x8b, 0x5e, 0xb8, 0xa3, 0x9b, 0x4b, 0x6d, 0x5c, 0x73, 0x55, 0x5b, 0x21, 0x00, 0x00, 0x00, 0x00,
]);
pub const INSTRUCTIONS_SYSVAR_ID: Pubkey = Pubkey([
    0x06, 0xa7, 0xd5, 0x17, 0x18, 0x7b, 0xd1, 0x66, 0x35, 0xda, 0xd4, 0x04, 0x55, 0xfd, 0xc2, 0xc0,
    0xc1, 0x24, 0xc6, 0x8f, 0x21, 0x56, 0x75, 0xa5, 0xdb, 0xba, 0xcb, 0x5f, 0x08, 0x00, 0x00, 0x00,
]);
pub const MEMO_PROGRAM_ID: Pubkey = Pubkey([
    0x05, 0x4a, 0x53, 0x5a, 0x99, 0x29, 0x21, 0x06, 0x4d, 0x24, 0xe8, 0x71, 0x60, 0xda, 0x38, 0x7c,
    0x7c, 0x35, 0xb5, 0xdd, 0xbc, 0x92, 0xbb, 0x81, 0xe4, 0x1f, 0xa8, 0x40, 0x41, 0x05, 0x44, 0x8d,
]);
/// Canonical deployed program ID string (`deploy/ratchet_dlmm_damm-keypair.json`).
pub const RATCHET_PROGRAM_ID_STR: &str = "5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5";

/// Canonical PDA seeds matching `programs/ratchet-dlmm-damm/src/instruction.rs`.
pub const POOL_SEED: &[u8] = b"ratchet_pool";
pub const BIN_ARRAY_SEED: &[u8] = b"bin_array";
pub const POOL_AUTHORITY_SEED: &[u8] = b"pool_authority";
pub const BASE_VAULT_SEED: &[u8] = b"base_vault";
pub const QUOTE_VAULT_SEED: &[u8] = b"quote_vault";

/// Binary instruction discriminators (`0x01`..`0x07`).
pub const IX_INITIALIZE_POOL: u8 = 0x01;
pub const IX_INITIALIZE_BIN_ARRAY: u8 = 0x02;
pub const IX_SEED_INITIAL_BINS: u8 = 0x03;
pub const IX_REVOKE_AND_OPEN_SLOT0: u8 = 0x04;
pub const IX_SWAP: u8 = 0x05;
pub const IX_GRADUATE_IN_PLACE: u8 = 0x06;
pub const IX_HARVEST_FALLBACK_TRANSFER_FEES: u8 = 0x07;

/// Jupiter v6 aggregator program ID (`JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4`).
pub const DEFAULT_JUPITER_PROGRAM_ID: Pubkey = Pubkey([
    0x04, 0x79, 0xd5, 0x5b, 0xf2, 0x31, 0xc0, 0x6e, 0xee, 0x74, 0xc5, 0x6e, 0xce, 0x68, 0x15, 0x07,
    0xfd, 0xb1, 0xb2, 0xde, 0xa3, 0xf4, 0x8e, 0x51, 0x02, 0xb1, 0xcd, 0xa2, 0x56, 0xbc, 0x13, 0x8f,
]);

/// Canonical Ratchet-DLMM-DAMM program ID (`5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5`).
pub const DEFAULT_RATCHET_PROGRAM_ID: Pubkey = Pubkey([
    0x41, 0xac, 0x0d, 0x22, 0xaa, 0x70, 0x8b, 0xee, 0x9e, 0xd2, 0x4f, 0xfa, 0x38, 0xaf, 0x1a, 0x84,
    0x93, 0xd1, 0xc1, 0x23, 0x47, 0x4e, 0xc1, 0x18, 0x85, 0x27, 0xd1, 0xba, 0x45, 0x75, 0x1e, 0x96,
]);

/// Lightweight 32-byte Solana public key wrapper (zero dependencies).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub const fn new_from_array(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Deterministic domain-separated address derivation for off-chain PDA mocks & adapters.
    pub fn derived(tag: &[u8], seed_a: &[u8], seed_b: &[u8]) -> Self {
        let mut out = [0u8; 32];
        let mut h1: u64 = 0xcbf29ce484222325;
        let mut h2: u64 = 0x9e3779b97f4a7c15;
        for &b in tag.iter().chain(seed_a.iter()).chain(seed_b.iter()) {
            h1 ^= b as u64;
            h1 = h1.wrapping_mul(0x100000001b3);
            h2 = h2.wrapping_add(b as u64).rotate_left(13);
        }
        for i in 0..4 {
            h1 = (h1 ^ (i as u64 + 1)).wrapping_mul(0x9e3779b97f4a7c15);
            h2 = (h2 ^ (i as u64 + 11)).wrapping_mul(0xbf58476d1ce4e5b9);
            let mix = h1 ^ h2.rotate_left(17);
            out[i * 8..(i + 1) * 8].copy_from_slice(&mix.to_le_bytes());
        }
        Self(out)
    }

    /// Computes exact Solana `create_program_address(seeds, program_id)` using FIPS 180-4 SHA-256.
    pub fn create_program_address(seeds: &[&[u8]], program_id: &Self) -> Self {
        let mut input = Vec::new();
        for seed in seeds {
            input.extend_from_slice(seed);
        }
        input.extend_from_slice(&program_id.0);
        input.extend_from_slice(b"ProgramDerivedAddress");
        Self(sha256(&input))
    }
}

/// Minimal zero-dependency SHA-256 implementation for exact Solana `create_program_address` PDA hashing.
fn sha256(msg: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    let bit_len = (msg.len() as u64) * 8;
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

impl fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pubkey({:02x}{:02x}..{:02x}{:02x})", self.0[0], self.0[1], self.0[30], self.0[31])
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..6] {
            write!(f, "{:02x}", b)?;
        }
        write!(f, "..")?;
        for b in &self.0[28..] {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

/// Adapter & simulation error variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    InvalidAccountDataLen { expected: usize, actual: usize },
    MissingAccount(Pubkey),
    InvalidPoolConfig,
    InvalidBinIndex,
    ZeroAmount,
    UnsupportedMint(Pubkey),
    UnsupportedSwapMode,
    InsufficientBinLiquidity,
    MathOverflow,
    DivisionByZero,
    SellImpactExceedsCap { impact_bps: u16, max_bps: u16 },
    InsolventPool { max_extractable: u64, real_vault: u64 },
    AccountLimitViolation,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAccountDataLen { expected, actual } => {
                write!(f, "invalid account data length: expected {}, got {}", expected, actual)
            }
            Self::MissingAccount(k) => write!(f, "missing required account in update map: {}", k),
            Self::InvalidPoolConfig => write!(f, "invalid pool configuration or invariants"),
            Self::InvalidBinIndex => write!(f, "bin index out of ActiveBinArrayPda bounds"),
            Self::ZeroAmount => write!(f, "swap amount must be > 0"),
            Self::UnsupportedMint(m) => write!(f, "unsupported mint for pool: {}", m),
            Self::UnsupportedSwapMode => write!(f, "only ExactIn swap mode is supported"),
            Self::InsufficientBinLiquidity => write!(f, "insufficient discrete bin liquidity"),
            Self::MathOverflow => write!(f, "checked arithmetic overflow"),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::SellImpactExceedsCap { impact_bps, max_bps } => {
                write!(f, "sell impact {} bps exceeds guardrail {} bps", impact_bps, max_bps)
            }
            Self::InsolventPool { max_extractable, real_vault } => {
                write!(f, "insolvent pool: max_extractable {} > real_vault {}", max_extractable, real_vault)
            }
            Self::AccountLimitViolation => write!(f, "swap account or CPI limit violated"),
        }
    }
}

impl std::error::Error for AdapterError {}

/// Account state snapshot passed into Jupiter `Amm::from_keyed_account` and `Amm::update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
}

pub type AccountMap = HashMap<Pubkey, Account>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedAccount {
    pub key: Pubkey,
    pub account: Account,
    pub params: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AmmContext {
    pub clock_timestamp: i64,
    pub slot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapMode {
    ExactIn,
    ExactOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    /// Quote SOL -> Base Token (buy).
    Buy,
    /// Base Token -> Quote SOL (sell).
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteParams {
    pub amount: u64,
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub swap_mode: SwapMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quote {
    pub in_amount: u64,
    pub out_amount: u64,
    pub fee_amount: u64,
    pub fee_mint: Pubkey,
    pub fee_pct_bps: u16,
    pub base_fee_bps: u16,
    pub variable_fee_bps: u16,
    pub compounded_sol_lamports: u64,
    pub protocol_fee_lamports: u64,
    pub price_impact_bps: u16,
    pub post_swap_sqrt_p: u128,
    pub post_swap_active_bin_id: i32,
    pub post_swap_v_a_up: u64,
    pub post_swap_v_a_down: u64,
    pub not_enough_liquidity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapParams {
    pub swap_mode: SwapMode,
    pub in_amount: u64,
    pub out_amount: u64,
    pub source_mint: Pubkey,
    pub destination_mint: Pubkey,
    pub source_token_account: Pubkey,
    pub destination_token_account: Pubkey,
    pub token_transfer_authority: Pubkey,
    pub open_orders_address: Option<Pubkey>,
    pub quote_mint_to_referrer: Option<Pubkey>,
    pub jupiter_program_id: Option<Pubkey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swap {
    RatchetDlmmDamm {
        direction: SwapDirection,
        compounding_fee_bps: u16,
        max_sell_impact_bps: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapAndAccountMetas {
    pub swap: Swap,
    pub account_metas: Vec<AccountMeta>,
    pub instruction_data: [u8; 18],
    pub cpi_depth: u8,
}

/// Packs `0x05 Swap` binary payload matching `programs/ratchet-dlmm-damm/src/instruction.rs::SwapArgs`.
pub fn pack_swap_ix_data(is_buy: bool, amount_in: u64, min_amount_out: u64) -> [u8; 18] {
    let mut out = [0u8; 18];
    out[0] = IX_SWAP;
    out[1] = u8::from(is_buy);
    out[2..10].copy_from_slice(&amount_in.to_le_bytes());
    out[10..18].copy_from_slice(&min_amount_out.to_le_bytes());
    out
}

/// Packs `0x01 InitializePool` binary payload (64 bytes) matching `instruction.rs`.
#[allow(clippy::too_many_arguments)]
pub fn pack_initialize_pool_ix_data(
    sqrt_p_start: u128,
    sqrt_p_graduation: u128,
    a_buy: u64,
    bin_step_bps: u16,
    active_bin_id: i32,
    floor_bin_id: i32,
    bin_array_start_id: i32,
    compounding_fee_bps: u16,
    gamma_bps: u16,
    max_sell_impact_bps: u16,
    transfer_fee_bps: u16,
    flags: u8,
) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[0] = IX_INITIALIZE_POOL;
    out[1..17].copy_from_slice(&sqrt_p_start.to_le_bytes());
    out[17..33].copy_from_slice(&sqrt_p_graduation.to_le_bytes());
    out[33..41].copy_from_slice(&a_buy.to_le_bytes());
    out[41..43].copy_from_slice(&bin_step_bps.to_le_bytes());
    out[43..47].copy_from_slice(&active_bin_id.to_le_bytes());
    out[47..51].copy_from_slice(&floor_bin_id.to_le_bytes());
    out[51..55].copy_from_slice(&bin_array_start_id.to_le_bytes());
    out[55..57].copy_from_slice(&compounding_fee_bps.to_le_bytes());
    out[57..59].copy_from_slice(&gamma_bps.to_le_bytes());
    out[59..61].copy_from_slice(&max_sell_impact_bps.to_le_bytes());
    out[61..63].copy_from_slice(&transfer_fee_bps.to_le_bytes());
    out[63] = flags;
    out
}

/// Packs `0x02 InitializeBinArray` binary payload (31 bytes) matching `instruction.rs`.
pub fn pack_initialize_bin_array_ix_data(
    start_bin_id: i32,
    active_bin_id: i32,
    floor_bin_id: i32,
    bin_step_bps: u16,
    start_sqrt_price_q64: u128,
) -> [u8; 31] {
    let mut out = [0u8; 31];
    out[0] = IX_INITIALIZE_BIN_ARRAY;
    out[1..5].copy_from_slice(&start_bin_id.to_le_bytes());
    out[5..9].copy_from_slice(&active_bin_id.to_le_bytes());
    out[9..13].copy_from_slice(&floor_bin_id.to_le_bytes());
    out[13..15].copy_from_slice(&bin_step_bps.to_le_bytes());
    out[15..31].copy_from_slice(&start_sqrt_price_q64.to_le_bytes());
    out
}

/// Packs `0x03 SeedInitialBins` binary payload (`18 + 20 * entries.len()` bytes) matching `instruction.rs`.
pub fn pack_seed_initial_bins_ix_data(
    quote_deposit_lamports: u64,
    base_deposit_atoms: u64,
    entries: &[(i32, u64, u64)],
) -> Vec<u8> {
    let count = entries.len().min(u8::MAX as usize);
    let mut out = Vec::with_capacity(18 + count * 20);
    out.push(IX_SEED_INITIAL_BINS);
    out.push(count as u8);
    out.extend_from_slice(&quote_deposit_lamports.to_le_bytes());
    out.extend_from_slice(&base_deposit_atoms.to_le_bytes());
    for &(bin_id, reserve_quote, reserve_base) in entries.iter().take(count) {
        out.extend_from_slice(&bin_id.to_le_bytes());
        out.extend_from_slice(&reserve_quote.to_le_bytes());
        out.extend_from_slice(&reserve_base.to_le_bytes());
    }
    out
}

/// Packs `0x04 RevokeAndOpenSlot0` payload.
pub const fn pack_revoke_and_open_slot0_ix_data() -> [u8; 1] {
    [IX_REVOKE_AND_OPEN_SLOT0]
}

/// Packs `0x06 GraduateInPlace` payload.
pub const fn pack_graduate_in_place_ix_data() -> [u8; 1] {
    [IX_GRADUATE_IN_PLACE]
}

/// Packs `0x07 HarvestFallbackTransferFees` payload.
pub const fn pack_harvest_fallback_transfer_fees_ix_data(mode: u8) -> [u8; 2] {
    [IX_HARVEST_FALLBACK_TRANSFER_FEES, mode]
}

/// Jupiter `Amm`-compatible trait.
pub trait Amm {
    fn from_keyed_account(
        keyed_account: &KeyedAccount,
        amm_context: &AmmContext,
    ) -> Result<Self, AdapterError>
    where
        Self: Sized;
    fn label(&self) -> String;
    fn program_id(&self) -> Pubkey;
    fn key(&self) -> Pubkey;
    fn get_reserve_mints(&self) -> Vec<Pubkey>;
    fn get_accounts_to_update(&self) -> Vec<Pubkey>;
    fn update(&mut self, account_map: &AccountMap) -> Result<(), AdapterError>;
    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote, AdapterError>;
    fn get_swap_and_account_metas(
        &self,
        swap_params: &SwapParams,
    ) -> Result<SwapAndAccountMetas, AdapterError>;
    fn has_dynamic_accounts(&self) -> bool {
        false
    }
    fn supports_exact_out(&self) -> bool {
        false
    }
}

/// Exact decoded 480-byte `RatchetPoolPda` state matching `programs/ratchet-dlmm-damm/src/state.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatchetPoolState {
    pub sqrt_p_high_water: u128,
    pub sqrt_p_current: u128,
    pub sqrt_p_start: u128,
    pub sqrt_p_graduation: u128,
    pub v_a_up: u64,
    pub v_a_down: u64,
    pub a_buy: u64,
    pub a_sell: u64,
    pub last_decay_timestamp: i64,
    pub real_vault_lamports: u64,
    pub max_extractable_lamports: u64,
    pub compounded_sol_total: u64,
    pub protocol_fee_lamports: u64,
    pub graduation_slot: u64,
    pub total_volume_quote: u64,
    pub harvested_transfer_fee_atoms: u64,
    pub active_bin_id: i32,
    pub floor_bin_id: i32,
    pub bin_array_start_id: i32,
    pub decay_period_seconds: u32,
    pub compounding_fee_bps: u16,
    pub f_0_bps: u16,
    pub f_min_bps: u16,
    pub f_max_sell_bps: u16,
    pub bin_step_bps: u16,
    pub max_sell_impact_bps: u16,
    pub gamma_bps: u16,
    pub transfer_fee_bps: u16,
    pub graduation_status: u8,
    pub authority_revoked: u8,
    pub vault_bump: u8,
    pub pool_bump: u8,
    pub bin_array_bump: u8,
    pub version: u8,
    pub transfer_fee_enabled: u8,
    pub filter_period_seconds: u8,
    pub base_mint: [u8; 32],
    pub quote_mint: [u8; 32],
    pub base_vault: [u8; 32],
    pub quote_vault: [u8; 32],
    pub authority: [u8; 32],
    pub active_bin_array: [u8; 32],
    pub creator_fee_vault: [u8; 32],
    pub padding: [u8; 56],
}

/// Exact decoded 32-byte `LiquidityBin`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiquidityBin {
    pub sqrt_price_q64: u128,
    pub reserve_quote_lamports: u64,
    pub reserve_base_atoms: u64,
}

/// Exact decoded 1,024-byte `ActiveBinArrayPda` state matching `programs/ratchet-dlmm-damm/src/state.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveBinArrayState {
    pub bin_step_q64: u128,
    pub pool: [u8; 32],
    pub active_bin_id: i32,
    pub floor_bin_id: i32,
    pub start_bin_id: i32,
    pub bin_step_bps: u16,
    pub bump: u8,
    pub version: u8,
    pub bins: [LiquidityBin; ACTIVE_BIN_ARRAY_LEN],
}

fn read_u8(data: &[u8], off: usize) -> u8 {
    data[off]
}
fn read_u16(data: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([data[off], data[off + 1]])
}
fn read_u32(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}
fn read_i32(data: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}
fn read_u64(data: &[u8], off: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[off..off + 8]);
    u64::from_le_bytes(buf)
}
fn read_i64(data: &[u8], off: usize) -> i64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[off..off + 8]);
    i64::from_le_bytes(buf)
}
fn read_u128(data: &[u8], off: usize) -> u128 {
    let mut buf = [0u8; 16];
    buf.copy_from_slice(&data[off..off + 16]);
    u128::from_le_bytes(buf)
}
fn read_32(data: &[u8], off: usize) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&data[off..off + 32]);
    buf
}

impl RatchetPoolState {
    /// Parses the 480-byte zero-copy `RatchetPoolPda` buffer at exact field offsets.
    pub fn from_bytes(data: &[u8]) -> Result<Self, AdapterError> {
        if data.len() != RATCHET_POOL_PDA_DATA_BYTES {
            return Err(AdapterError::InvalidAccountDataLen {
                expected: RATCHET_POOL_PDA_DATA_BYTES,
                actual: data.len(),
            });
        }
        let mut padding = [0u8; 56];
        padding.copy_from_slice(&data[424..480]);

        let state = Self {
            sqrt_p_high_water: read_u128(data, 0),
            sqrt_p_current: read_u128(data, 16),
            sqrt_p_start: read_u128(data, 32),
            sqrt_p_graduation: read_u128(data, 48),
            v_a_up: read_u64(data, 64),
            v_a_down: read_u64(data, 72),
            a_buy: read_u64(data, 80),
            a_sell: read_u64(data, 88),
            last_decay_timestamp: read_i64(data, 96),
            real_vault_lamports: read_u64(data, 104),
            max_extractable_lamports: read_u64(data, 112),
            compounded_sol_total: read_u64(data, 120),
            protocol_fee_lamports: read_u64(data, 128),
            graduation_slot: read_u64(data, 136),
            total_volume_quote: read_u64(data, 144),
            harvested_transfer_fee_atoms: read_u64(data, 152),
            active_bin_id: read_i32(data, 160),
            floor_bin_id: read_i32(data, 164),
            bin_array_start_id: read_i32(data, 168),
            decay_period_seconds: read_u32(data, 172),
            compounding_fee_bps: read_u16(data, 176),
            f_0_bps: read_u16(data, 178),
            f_min_bps: read_u16(data, 180),
            f_max_sell_bps: read_u16(data, 182),
            bin_step_bps: read_u16(data, 184),
            max_sell_impact_bps: read_u16(data, 186),
            gamma_bps: read_u16(data, 188),
            transfer_fee_bps: read_u16(data, 190),
            graduation_status: read_u8(data, 192),
            authority_revoked: read_u8(data, 193),
            vault_bump: read_u8(data, 194),
            pool_bump: read_u8(data, 195),
            bin_array_bump: read_u8(data, 196),
            version: read_u8(data, 197),
            transfer_fee_enabled: read_u8(data, 198),
            filter_period_seconds: read_u8(data, 199),
            base_mint: read_32(data, 200),
            quote_mint: read_32(data, 232),
            base_vault: read_32(data, 264),
            quote_vault: read_32(data, 296),
            authority: read_32(data, 328),
            active_bin_array: read_32(data, 360),
            creator_fee_vault: read_32(data, 392),
            padding,
        };
        state.validate()?;
        Ok(state)
    }

    /// Serializes into the exact 480-byte `RatchetPoolPda` layout.
    pub fn to_bytes(&self) -> [u8; RATCHET_POOL_PDA_DATA_BYTES] {
        let mut out = [0u8; RATCHET_POOL_PDA_DATA_BYTES];
        out[0..16].copy_from_slice(&self.sqrt_p_high_water.to_le_bytes());
        out[16..32].copy_from_slice(&self.sqrt_p_current.to_le_bytes());
        out[32..48].copy_from_slice(&self.sqrt_p_start.to_le_bytes());
        out[48..64].copy_from_slice(&self.sqrt_p_graduation.to_le_bytes());
        out[64..72].copy_from_slice(&self.v_a_up.to_le_bytes());
        out[72..80].copy_from_slice(&self.v_a_down.to_le_bytes());
        out[80..88].copy_from_slice(&self.a_buy.to_le_bytes());
        out[88..96].copy_from_slice(&self.a_sell.to_le_bytes());
        out[96..104].copy_from_slice(&self.last_decay_timestamp.to_le_bytes());
        out[104..112].copy_from_slice(&self.real_vault_lamports.to_le_bytes());
        out[112..120].copy_from_slice(&self.max_extractable_lamports.to_le_bytes());
        out[120..128].copy_from_slice(&self.compounded_sol_total.to_le_bytes());
        out[128..136].copy_from_slice(&self.protocol_fee_lamports.to_le_bytes());
        out[136..144].copy_from_slice(&self.graduation_slot.to_le_bytes());
        out[144..152].copy_from_slice(&self.total_volume_quote.to_le_bytes());
        out[152..160].copy_from_slice(&self.harvested_transfer_fee_atoms.to_le_bytes());
        out[160..164].copy_from_slice(&self.active_bin_id.to_le_bytes());
        out[164..168].copy_from_slice(&self.floor_bin_id.to_le_bytes());
        out[168..172].copy_from_slice(&self.bin_array_start_id.to_le_bytes());
        out[172..176].copy_from_slice(&self.decay_period_seconds.to_le_bytes());
        out[176..178].copy_from_slice(&self.compounding_fee_bps.to_le_bytes());
        out[178..180].copy_from_slice(&self.f_0_bps.to_le_bytes());
        out[180..182].copy_from_slice(&self.f_min_bps.to_le_bytes());
        out[182..184].copy_from_slice(&self.f_max_sell_bps.to_le_bytes());
        out[184..186].copy_from_slice(&self.bin_step_bps.to_le_bytes());
        out[186..188].copy_from_slice(&self.max_sell_impact_bps.to_le_bytes());
        out[188..190].copy_from_slice(&self.gamma_bps.to_le_bytes());
        out[190..192].copy_from_slice(&self.transfer_fee_bps.to_le_bytes());
        out[192] = self.graduation_status;
        out[193] = self.authority_revoked;
        out[194] = self.vault_bump;
        out[195] = self.pool_bump;
        out[196] = self.bin_array_bump;
        out[197] = self.version;
        out[198] = self.transfer_fee_enabled;
        out[199] = self.filter_period_seconds;
        out[200..232].copy_from_slice(&self.base_mint);
        out[232..264].copy_from_slice(&self.quote_mint);
        out[264..296].copy_from_slice(&self.base_vault);
        out[296..328].copy_from_slice(&self.quote_vault);
        out[328..360].copy_from_slice(&self.authority);
        out[360..392].copy_from_slice(&self.active_bin_array);
        out[392..424].copy_from_slice(&self.creator_fee_vault);
        out[424..480].copy_from_slice(&self.padding);
        out
    }

    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.bin_step_bps == 0 || self.bin_step_bps > 10_000 {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.f_min_bps > self.f_0_bps || self.f_0_bps > 10_000 {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.f_max_sell_bps > IMMUTABLE_F_MAX_SELL_BPS || self.f_min_bps > self.f_max_sell_bps {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.compounding_fee_bps < DEFAULT_COMPOUNDING_FEE_BPS || self.compounding_fee_bps > 10_000 {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.max_sell_impact_bps > HARD_MAX_SELL_IMPACT_BPS {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.gamma_bps > 10_000 {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.a_buy.checked_mul(5) != Some(self.a_sell) {
            return Err(AdapterError::InvalidPoolConfig);
        }
        if self.max_extractable_lamports > self.real_vault_lamports {
            return Err(AdapterError::InsolventPool {
                max_extractable: self.max_extractable_lamports,
                real_vault: self.real_vault_lamports,
            });
        }
        Ok(())
    }
}

impl ActiveBinArrayState {
    /// Parses the 1,024-byte zero-copy `ActiveBinArrayPda` buffer at exact field offsets.
    pub fn from_bytes(data: &[u8]) -> Result<Self, AdapterError> {
        if data.len() != ACTIVE_BIN_ARRAY_PDA_DATA_BYTES {
            return Err(AdapterError::InvalidAccountDataLen {
                expected: ACTIVE_BIN_ARRAY_PDA_DATA_BYTES,
                actual: data.len(),
            });
        }
        let mut bins = [LiquidityBin::default(); ACTIVE_BIN_ARRAY_LEN];
        for (i, slot) in bins.iter_mut().enumerate() {
            let base = 64 + i * 32;
            slot.sqrt_price_q64 = read_u128(data, base);
            slot.reserve_quote_lamports = read_u64(data, base + 16);
            slot.reserve_base_atoms = read_u64(data, base + 24);
        }
        Ok(Self {
            bin_step_q64: read_u128(data, 0),
            pool: read_32(data, 16),
            active_bin_id: read_i32(data, 48),
            floor_bin_id: read_i32(data, 52),
            start_bin_id: read_i32(data, 56),
            bin_step_bps: read_u16(data, 60),
            bump: read_u8(data, 62),
            version: read_u8(data, 63),
            bins,
        })
    }

    /// Serializes into the exact 1,024-byte `ActiveBinArrayPda` layout.
    pub fn to_bytes(&self) -> [u8; ACTIVE_BIN_ARRAY_PDA_DATA_BYTES] {
        let mut out = [0u8; ACTIVE_BIN_ARRAY_PDA_DATA_BYTES];
        out[0..16].copy_from_slice(&self.bin_step_q64.to_le_bytes());
        out[16..48].copy_from_slice(&self.pool);
        out[48..52].copy_from_slice(&self.active_bin_id.to_le_bytes());
        out[52..56].copy_from_slice(&self.floor_bin_id.to_le_bytes());
        out[56..60].copy_from_slice(&self.start_bin_id.to_le_bytes());
        out[60..62].copy_from_slice(&self.bin_step_bps.to_le_bytes());
        out[62] = self.bump;
        out[63] = self.version;
        for (i, bin) in self.bins.iter().enumerate() {
            let base = 64 + i * 32;
            out[base..base + 16].copy_from_slice(&bin.sqrt_price_q64.to_le_bytes());
            out[base + 16..base + 24].copy_from_slice(&bin.reserve_quote_lamports.to_le_bytes());
            out[base + 24..base + 32].copy_from_slice(&bin.reserve_base_atoms.to_le_bytes());
        }
        out
    }

    pub fn bin_index(&self, bin_id: i32) -> Result<usize, AdapterError> {
        let idx = bin_id
            .checked_sub(self.start_bin_id)
            .ok_or(AdapterError::InvalidBinIndex)?;
        if idx < 0 || (idx as usize) >= ACTIVE_BIN_ARRAY_LEN {
            return Err(AdapterError::InvalidBinIndex);
        }
        Ok(idx as usize)
    }
}

/// Overflow-safe ceiling integer division `ceil(num / den)` in `u128`.
#[inline]
pub fn div_ceil_u128(num: u128, den: u128) -> Result<u128, AdapterError> {
    if den == 0 {
        return Err(AdapterError::DivisionByZero);
    }
    if num == 0 {
        return Ok(0);
    }
    Ok((num - 1) / den + 1)
}

/// Exact unsigned 256-bit helper `[u64; 4]` (little-endian limbs) for Q64.64 `sqrt_p^2` pricing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct U256([u64; 4]);

impl U256 {
    fn mul_u128(a: u128, b: u128) -> Self {
        let a_lo = a as u64 as u128;
        let a_hi = (a >> 64) as u64 as u128;
        let b_lo = b as u64 as u128;
        let b_hi = (b >> 64) as u64 as u128;

        let ll = a_lo * b_lo;
        let lh = a_lo * b_hi;
        let hl = a_hi * b_lo;
        let hh = a_hi * b_hi;

        let l0 = ll as u64;
        let mid1 = (ll >> 64) + (lh as u64 as u128) + (hl as u64 as u128);
        let l1 = mid1 as u64;
        let mid2 = (mid1 >> 64) + (lh >> 64) + (hl >> 64) + (hh as u64 as u128);
        let l2 = mid2 as u64;
        let l3 = ((mid2 >> 64) + (hh >> 64)) as u64;
        Self([l0, l1, l2, l3])
    }

    fn mul_u64(self, rhs: u64) -> Result<Self, AdapterError> {
        let mut out = [0u64; 4];
        let mut carry: u128 = 0;
        for (i, slot) in out.iter_mut().enumerate() {
            let prod = (self.0[i] as u128) * (rhs as u128) + carry;
            *slot = prod as u64;
            carry = prod >> 64;
        }
        if carry != 0 {
            return Err(AdapterError::MathOverflow);
        }
        Ok(Self(out))
    }

    fn shr_128(self) -> u128 {
        ((self.0[3] as u128) << 64) | (self.0[2] as u128)
    }

    fn shl_128_from_u64(v: u64) -> Self {
        Self([0, 0, v, 0])
    }

    fn bit_len(&self) -> u32 {
        for i in (0..4).rev() {
            if self.0[i] != 0 {
                return (i as u32) * 64 + (64 - self.0[i].leading_zeros());
            }
        }
        0
    }

    fn bit(&self, idx: u32) -> bool {
        let limb = (idx / 64) as usize;
        let bit = idx % 64;
        (self.0[limb] >> bit) & 1 == 1
    }

    fn set_bit(&mut self, idx: u32) {
        let limb = (idx / 64) as usize;
        let bit = idx % 64;
        self.0[limb] |= 1u64 << bit;
    }

    fn shl1_with_bit(&mut self, bit: bool) -> bool {
        let carry_out = (self.0[3] >> 63) == 1;
        for i in (1..4).rev() {
            self.0[i] = (self.0[i] << 1) | (self.0[i - 1] >> 63);
        }
        self.0[0] = (self.0[0] << 1) | (bit as u64);
        carry_out
    }

    fn cmp_val(&self, other: &Self) -> std::cmp::Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&other.0[i]) {
                std::cmp::Ordering::Equal => continue,
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
    }

    fn sub_assign(&mut self, rhs: &Self) {
        let mut borrow: u128 = 0;
        for i in 0..4 {
            let lhs_i = self.0[i] as u128;
            let rhs_i = rhs.0[i] as u128 + borrow;
            if lhs_i >= rhs_i {
                self.0[i] = (lhs_i - rhs_i) as u64;
                borrow = 0;
            } else {
                self.0[i] = ((1u128 << 64) + lhs_i - rhs_i) as u64;
                borrow = 1;
            }
        }
    }

    fn div_rem(self, den: Self) -> Result<(Self, Self), AdapterError> {
        if den == Self([0; 4]) {
            return Err(AdapterError::DivisionByZero);
        }
        if self.cmp_val(&den) == std::cmp::Ordering::Less {
            return Ok((Self([0; 4]), self));
        }
        let mut q = Self([0; 4]);
        let mut r = Self([0; 4]);
        let bits = self.bit_len();
        for idx in (0..bits).rev() {
            let overflow = r.shl1_with_bit(self.bit(idx));
            if overflow || r.cmp_val(&den) != std::cmp::Ordering::Less {
                r.sub_assign(&den);
                q.set_bit(idx);
            }
        }
        Ok((q, r))
    }

    fn to_u64_checked(self) -> Result<u64, AdapterError> {
        if self.0[1] != 0 || self.0[2] != 0 || self.0[3] != 0 {
            return Err(AdapterError::MathOverflow);
        }
        Ok(self.0[0])
    }

    fn to_u128_checked(self) -> Result<u128, AdapterError> {
        if self.0[2] != 0 || self.0[3] != 0 {
            return Err(AdapterError::MathOverflow);
        }
        Ok(((self.0[1] as u128) << 64) | (self.0[0] as u128))
    }
}

/// Converts `base_atoms` to `quote_lamports` at bin price `P = (sqrt_p_q64 / 2^64)^2`, floored.
fn base_to_quote_floor(base_atoms: u64, sqrt_p_q64: u128) -> Result<u64, AdapterError> {
    if base_atoms == 0 || sqrt_p_q64 == 0 {
        return Ok(0);
    }
    let p_q128 = U256::mul_u128(sqrt_p_q64, sqrt_p_q64);
    let q_shifted = p_q128.mul_u64(base_atoms)?;
    let quote_u128 = q_shifted.shr_128();
    u64::try_from(quote_u128).map_err(|_| AdapterError::MathOverflow)
}

/// Converts `quote_lamports` to `base_atoms` at bin price `P = (sqrt_p_q64 / 2^64)^2`, floored.
fn quote_to_base_floor(quote_lamports: u64, sqrt_p_q64: u128) -> Result<u64, AdapterError> {
    if quote_lamports == 0 {
        return Ok(0);
    }
    if sqrt_p_q64 == 0 {
        return Err(AdapterError::DivisionByZero);
    }
    let p_q128 = U256::mul_u128(sqrt_p_q64, sqrt_p_q64);
    let num = U256::shl_128_from_u64(quote_lamports);
    let (q, _) = num.div_rem(p_q128)?;
    q.to_u64_checked()
}

/// Computes `ceil((base_atoms * P) / 1)` in quote lamports (needed to clear a bin on buy).
fn base_to_quote_ceil(base_atoms: u64, sqrt_p_q64: u128) -> Result<u64, AdapterError> {
    if base_atoms == 0 || sqrt_p_q64 == 0 {
        return Ok(0);
    }
    let p_q128 = U256::mul_u128(sqrt_p_q64, sqrt_p_q64);
    let q_shifted = p_q128.mul_u64(base_atoms)?;
    let has_frac = q_shifted.0[0] != 0 || q_shifted.0[1] != 0;
    let whole = q_shifted.shr_128();
    let res = if has_frac {
        whole.checked_add(1).ok_or(AdapterError::MathOverflow)?
    } else {
        whole
    };
    u64::try_from(res).map_err(|_| AdapterError::MathOverflow)
}

/// Computes `ceil((P_before - P_after) * 10_000 / P_before)` in bps using exact Q128.128 `U256` math.
pub fn compute_sell_impact_bps(sqrt_p_before: u128, sqrt_p_after: u128) -> Result<u16, AdapterError> {
    if sqrt_p_before == 0 {
        return Err(AdapterError::DivisionByZero);
    }
    if sqrt_p_after >= sqrt_p_before {
        return Ok(0);
    }
    let p_before = U256::mul_u128(sqrt_p_before, sqrt_p_before);
    let p_after = U256::mul_u128(sqrt_p_after, sqrt_p_after);
    let mut diff = p_before;
    diff.sub_assign(&p_after);
    let scaled = diff.mul_u64(10_000)?;
    let (q, r) = scaled.div_rem(p_before)?;
    let ceil_q = if r == U256([0; 4]) {
        q.to_u128_checked()?
    } else {
        q.to_u128_checked()?.checked_add(1).ok_or(AdapterError::MathOverflow)?
    };
    Ok(ceil_q.min(u16::MAX as u128) as u16)
}

/// Fixed-point Q64.64 round-nearest multiplication `((a * b) + 2^63) >> 64`.
#[inline]
fn q64_mul_round(a: u128, b: u128) -> Result<u128, AdapterError> {
    let prod = U256::mul_u128(a, b);
    let lo = ((prod.0[1] as u128) << 64) | (prod.0[0] as u128);
    let mut hi = ((prod.0[3] as u128) << 64) | (prod.0[2] as u128);
    let (lo_rounded, carry) = lo.overflowing_add(1u128 << 63);
    if carry {
        hi = hi.checked_add(1).ok_or(AdapterError::MathOverflow)?;
    }
    if hi >= Q64_ONE {
        return Err(AdapterError::MathOverflow);
    }
    Ok((hi << 64) | (lo_rounded >> 64))
}

/// Exponentiation by squaring in Q64.64 fixed point matching `programs/ratchet-dlmm-damm/src/math.rs::pow_q64`.
fn pow_q64(mut base_q64: u128, mut exp: u32) -> Result<u128, AdapterError> {
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

/// Computes exact integer discrete bin steps above `sqrt_p_start` reached by `sqrt_p_high_water`.
pub fn steps_above_sqrt_p_max(
    sqrt_p_high_water: u128,
    sqrt_p_start: u128,
    bin_step_q64: u128,
) -> u32 {
    if sqrt_p_high_water <= sqrt_p_start || sqrt_p_start == 0 || bin_step_q64 <= Q64_ONE {
        return 0;
    }
    let mut cur = sqrt_p_start;
    let mut steps: u32 = 0;
    while steps < 10_000 {
        let next = U256::mul_u128(cur, bin_step_q64);
        let next_q64 = ((next.0[2] as u128) << 64) | (next.0[1] as u128);
        if next_q64 <= cur || next_q64 > sqrt_p_high_water {
            break;
        }
        cur = next_q64;
        steps += 1;
    }
    steps
}

/// Computes decayed base buy fee `max(f_min, f_0 * gamma^(steps_above(sqrt_P_max)))`.
/// Matches `programs/ratchet-dlmm-damm/src/math.rs::compute_buy_base_fee_bps`.
pub fn compute_high_water_base_fee_bps(
    f_0_bps: u16,
    f_min_bps: u16,
    gamma_bps: u16,
    steps_above: u32,
) -> Result<u16, AdapterError> {
    if gamma_bps > 10_000 {
        return Err(AdapterError::InvalidPoolConfig);
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

/// Default reduction factor per elapsed filter interval (`50.00%` = `5_000 bps`).
pub const DEFAULT_REDUCTION_FACTOR_BPS: u16 = 5_000;

/// Computes real-time decayed directional accumulator at `current_timestamp` matching
/// `programs/ratchet-dlmm-damm/src/math.rs::decay_volatility_accumulators`:
///
/// - Inside `[0, filter_period_seconds)`: 0% decay (accumulator stays hot).
/// - Inside `[filter_period_seconds, decay_period_seconds)`: geometric decay by `reduction_factor_bps`
///   (`5_000 bps` = 50%) per elapsed `filter_period_seconds` interval.
/// - At `>= decay_period_seconds`: 100% decayed (`0`).
pub fn decay_accumulator(
    v_a: u64,
    last_decay_timestamp: i64,
    current_timestamp: i64,
    filter_period_seconds: u8,
    decay_period_seconds: u32,
) -> u64 {
    if v_a == 0 || current_timestamp <= last_decay_timestamp {
        return v_a;
    }
    let elapsed = (current_timestamp - last_decay_timestamp) as u64;
    let filter = (filter_period_seconds as u64).max(1);
    let decay = decay_period_seconds as u64;
    if elapsed < filter_period_seconds as u64 {
        return v_a;
    }
    if elapsed >= decay {
        return 0;
    }
    let steps = (elapsed / filter) as u32;
    let factor_q64 =
        (((DEFAULT_REDUCTION_FACTOR_BPS as u128) << 64) + (BPS_DENOMINATOR / 2)) / BPS_DENOMINATOR;
    let decay_q64 = pow_q64(factor_q64, steps).unwrap_or(0);
    ((v_a as u128 * decay_q64) >> 64) as u64
}

/// Computes `f_buy(t) = max(f_min, f_0 * gamma^(steps_above(sqrt_P_max))) + ceil(A_buy * (v_a^+ * s)^2 / 10^11)`.
pub fn compute_f_buy_bps(
    f_0_bps: u16,
    f_min_bps: u16,
    gamma_bps: u16,
    steps_above: u32,
    a_buy: u64,
    v_a_up: u64,
    bin_step_bps: u16,
) -> Result<(u16, u16, u16), AdapterError> {
    let base_fee = compute_high_water_base_fee_bps(f_0_bps, f_min_bps, gamma_bps, steps_above)?;
    let vs = (v_a_up as u128)
        .checked_mul(bin_step_bps as u128)
        .ok_or(AdapterError::MathOverflow)?;
    let vs_sq = vs.checked_mul(vs).ok_or(AdapterError::MathOverflow)?;
    let num = (a_buy as u128)
        .checked_mul(vs_sq)
        .ok_or(AdapterError::MathOverflow)?;
    let var_fee = div_ceil_u128(num, VOLATILITY_DENOMINATOR)?;
    let total = ((base_fee as u128) + var_fee).min(MAX_BUY_FEE_BPS as u128) as u16;
    Ok((base_fee, var_fee.min(u16::MAX as u128) as u16, total))
}

/// Computes `f_sell(t) = min(25%, f_base + ceil(5 * A_buy * (v_a^- * s)^2 / 10^11))`,
/// overflow-safe across `[0, u64::MAX]`.
pub fn compute_f_sell_bps(
    f_base_bps: u16,
    f_max_sell_bps: u16,
    a_buy: u64,
    v_a_down: u64,
    bin_step_bps: u16,
) -> Result<(u16, u16, u16), AdapterError> {
    let cap = f_max_sell_bps.min(IMMUTABLE_F_MAX_SELL_BPS);
    if f_base_bps >= cap {
        return Ok((f_base_bps, 0, cap));
    }
    if a_buy == 0 || v_a_down == 0 || bin_step_bps == 0 {
        return Ok((f_base_bps, 0, f_base_bps));
    }
    let a_sell = (a_buy as u128)
        .checked_mul(5)
        .ok_or(AdapterError::MathOverflow)?;
    let vs = (v_a_down as u128)
        .checked_mul(bin_step_bps as u128)
        .ok_or(AdapterError::MathOverflow)?;

    let var_u128 = match vs.checked_mul(vs).and_then(|sq| a_sell.checked_mul(sq)) {
        Some(num) => div_ceil_u128(num, VOLATILITY_DENOMINATOR)?,
        None => cap as u128,
    };
    let uncapped = (f_base_bps as u128).saturating_add(var_u128);
    let total = uncapped.min(cap as u128) as u16;
    Ok((f_base_bps, var_u128.min(cap as u128) as u16, total))
}

/// Main Jupiter `Amm` adapter state holding parsed 480B pool + 1,024B active bin array PDAs.
#[derive(Debug, Clone)]
pub struct RatchetJupiterAmm {
    pub program_id: Pubkey,
    pub pool_key: Pubkey,
    pub pool: RatchetPoolState,
    pub bin_array: ActiveBinArrayState,
    pub clock_timestamp: i64,
    pub current_slot: u64,
}

impl RatchetJupiterAmm {
    /// Applies a simulated swap directly onto the in-memory `RatchetPoolState` and `ActiveBinArrayState`
    /// (used by shadow mainnet dry-run replay to advance real pool & bin state across wallets/slots).
    pub fn execute_simulated_swap(
        &mut self,
        quote_params: &QuoteParams,
        timestamp: i64,
    ) -> Result<Quote, AdapterError> {
        // Real-time decay update prior to swap
        let decayed_up = decay_accumulator(
            self.pool.v_a_up,
            self.pool.last_decay_timestamp,
            timestamp,
            self.pool.filter_period_seconds,
            self.pool.decay_period_seconds,
        );
        let decayed_down = decay_accumulator(
            self.pool.v_a_down,
            self.pool.last_decay_timestamp,
            timestamp,
            self.pool.filter_period_seconds,
            self.pool.decay_period_seconds,
        );
        self.pool.v_a_up = decayed_up;
        self.pool.v_a_down = decayed_down;
        self.pool.last_decay_timestamp = timestamp;
        self.clock_timestamp = timestamp;

        let (quote, mut next_pool, mut next_bins) = self.simulate_swap_internal(quote_params)?;
        next_pool.last_decay_timestamp = timestamp;
        next_bins.active_bin_id = next_pool.active_bin_id;
        self.pool = next_pool;
        self.bin_array = next_bins;
        Ok(quote)
    }

    fn simulate_swap_internal(
        &self,
        quote_params: &QuoteParams,
    ) -> Result<(Quote, RatchetPoolState, ActiveBinArrayState), AdapterError> {
        if quote_params.swap_mode != SwapMode::ExactIn {
            return Err(AdapterError::UnsupportedSwapMode);
        }
        if quote_params.amount == 0 {
            return Err(AdapterError::ZeroAmount);
        }
        self.pool.validate()?;

        let base_mint = Pubkey(self.pool.base_mint);
        let quote_mint = Pubkey(self.pool.quote_mint);
        let is_buy = if quote_params.input_mint == quote_mint && quote_params.output_mint == base_mint
        {
            true
        } else if quote_params.input_mint == base_mint && quote_params.output_mint == quote_mint {
            false
        } else {
            return Err(AdapterError::UnsupportedMint(quote_params.input_mint));
        };

        let mut pool = self.pool;
        let mut bins = self.bin_array;

        // Apply real-time accumulator decay based on current `clock_timestamp`
        let v_a_up_start = decay_accumulator(
            pool.v_a_up,
            pool.last_decay_timestamp,
            self.clock_timestamp,
            pool.filter_period_seconds,
            pool.decay_period_seconds,
        );
        let v_a_down_start = decay_accumulator(
            pool.v_a_down,
            pool.last_decay_timestamp,
            self.clock_timestamp,
            pool.filter_period_seconds,
            pool.decay_period_seconds,
        );

        let sqrt_p_before = pool.sqrt_p_current;

        if is_buy {
            let steps_above = steps_above_sqrt_p_max(
                pool.sqrt_p_high_water,
                pool.sqrt_p_start,
                bins.bin_step_q64,
            );
            let (_, _, init_fee_bps) = compute_f_buy_bps(
                pool.f_0_bps,
                pool.f_min_bps,
                pool.gamma_bps,
                steps_above,
                pool.a_buy,
                v_a_up_start,
                pool.bin_step_bps,
            )?;

            let gross_in = quote_params.amount;
            let init_fee_lamports = div_ceil_u128(
                (gross_in as u128) * (init_fee_bps as u128),
                BPS_DENOMINATOR,
            )?
            .min(gross_in as u128) as u64;
            let init_net_quote = gross_in.saturating_sub(init_fee_lamports);
            let est_crossings = {
                let mut probe = init_net_quote;
                let mut probe_bin = pool.active_bin_id;
                while probe > 0 {
                    let idx = match bins.bin_index(probe_bin) {
                        Ok(i) => i,
                        Err(_) => break,
                    };
                    let bin_sqrt_p = bins.bins[idx].sqrt_price_q64;
                    let avail_base = bins.bins[idx].reserve_base_atoms;
                    if avail_base > 0 {
                        let q_all = base_to_quote_ceil(avail_base, bin_sqrt_p)?;
                        if probe <= q_all {
                            break;
                        }
                        probe -= q_all;
                    }
                    let next_bin = probe_bin + 1;
                    if bins.bin_index(next_bin).is_err() {
                        break;
                    }
                    probe_bin = next_bin;
                }
                (probe_bin - pool.active_bin_id).max(0) as u64
            };

            let projected_v_a_up = v_a_up_start.saturating_add(est_crossings);
            let (base_fee_bps, var_fee_bps, fee_bps) = compute_f_buy_bps(
                pool.f_0_bps,
                pool.f_min_bps,
                pool.gamma_bps,
                steps_above,
                pool.a_buy,
                projected_v_a_up,
                pool.bin_step_bps,
            )?;

            let total_fee = div_ceil_u128(
                (gross_in as u128) * (fee_bps as u128),
                BPS_DENOMINATOR,
            )?
            .min(gross_in as u128) as u64;
            let mut rem_quote = gross_in.saturating_sub(total_fee);
            let compounded_sol = 0u64;
            let protocol_fee = total_fee;

            let mut total_base_out: u64 = 0;
            let mut cur_bin_id = pool.active_bin_id;
            let mut v_a_up = v_a_up_start;

            while rem_quote > 0 {
                let idx = bins.bin_index(cur_bin_id)?;
                let bin_sqrt_p = bins.bins[idx].sqrt_price_q64;
                let avail_base = bins.bins[idx].reserve_base_atoms;

                if avail_base > 0 {
                    let quote_for_all = base_to_quote_ceil(avail_base, bin_sqrt_p)?;
                    if rem_quote <= quote_for_all {
                        let base_bought = quote_to_base_floor(rem_quote, bin_sqrt_p)?.min(avail_base);
                        bins.bins[idx].reserve_base_atoms -= base_bought;
                        bins.bins[idx].reserve_quote_lamports = bins.bins[idx]
                            .reserve_quote_lamports
                            .checked_add(rem_quote)
                            .ok_or(AdapterError::MathOverflow)?;
                        total_base_out = total_base_out
                            .checked_add(base_bought)
                            .ok_or(AdapterError::MathOverflow)?;
                        pool.sqrt_p_current = bin_sqrt_p;
                        pool.sqrt_p_high_water = pool.sqrt_p_high_water.max(bin_sqrt_p);
                        break;
                    } else {
                        bins.bins[idx].reserve_base_atoms = 0;
                        bins.bins[idx].reserve_quote_lamports = bins.bins[idx]
                            .reserve_quote_lamports
                            .checked_add(quote_for_all)
                            .ok_or(AdapterError::MathOverflow)?;
                        total_base_out = total_base_out
                            .checked_add(avail_base)
                            .ok_or(AdapterError::MathOverflow)?;
                        rem_quote = rem_quote.saturating_sub(quote_for_all);
                        pool.sqrt_p_current = bin_sqrt_p;
                        pool.sqrt_p_high_water = pool.sqrt_p_high_water.max(bin_sqrt_p);
                    }
                }

                let next_bin = cur_bin_id + 1;
                if bins.bin_index(next_bin).is_err() {
                    return Err(AdapterError::InsufficientBinLiquidity);
                }
                cur_bin_id = next_bin;
                v_a_up = v_a_up.saturating_add(1);
                let next_idx = bins.bin_index(cur_bin_id)?;
                pool.sqrt_p_current = bins.bins[next_idx].sqrt_price_q64;
                pool.sqrt_p_high_water = pool.sqrt_p_high_water.max(pool.sqrt_p_current);
            }

            pool.active_bin_id = cur_bin_id;
            pool.v_a_up = v_a_up;
            pool.v_a_down = v_a_down_start;
            pool.protocol_fee_lamports = pool.protocol_fee_lamports.saturating_add(protocol_fee);
            pool.real_vault_lamports = pool.real_vault_lamports.saturating_add(gross_in);
            pool.max_extractable_lamports = pool
                .max_extractable_lamports
                .saturating_add(gross_in.saturating_sub(total_fee));
            pool.total_volume_quote = pool.total_volume_quote.saturating_add(gross_in);

            let price_impact_bps = compute_sell_impact_bps(pool.sqrt_p_current, sqrt_p_before)?;
            let quote = Quote {
                in_amount: gross_in,
                out_amount: total_base_out,
                fee_amount: total_fee,
                fee_mint: quote_mint,
                fee_pct_bps: fee_bps,
                base_fee_bps,
                variable_fee_bps: var_fee_bps,
                compounded_sol_lamports: compounded_sol,
                protocol_fee_lamports: protocol_fee,
                price_impact_bps,
                post_swap_sqrt_p: pool.sqrt_p_current,
                post_swap_active_bin_id: pool.active_bin_id,
                post_swap_v_a_up: pool.v_a_up,
                post_swap_v_a_down: pool.v_a_down,
                not_enough_liquidity: false,
            };
            Ok((quote, pool, bins))
        } else {
            // SELL: Base Token -> Quote SOL (subject to Gate G09 700 bps impact guardrail + 5x dump-brake)
            let mut rem_base = quote_params.amount;
            let mut gross_sol_out: u64 = 0;
            let mut cur_bin_id = pool.active_bin_id;
            let mut v_a_down = v_a_down_start;

            while rem_base > 0 {
                let idx = bins.bin_index(cur_bin_id)?;
                let bin_sqrt_p = bins.bins[idx].sqrt_price_q64;
                let avail_quote = bins.bins[idx].reserve_quote_lamports;

                if avail_quote > 0 {
                    let quote_wanted = base_to_quote_floor(rem_base, bin_sqrt_p)?;
                    if quote_wanted <= avail_quote {
                        bins.bins[idx].reserve_quote_lamports -= quote_wanted;
                        bins.bins[idx].reserve_base_atoms = bins.bins[idx]
                            .reserve_base_atoms
                            .checked_add(rem_base)
                            .ok_or(AdapterError::MathOverflow)?;
                        gross_sol_out = gross_sol_out
                            .checked_add(quote_wanted)
                            .ok_or(AdapterError::MathOverflow)?;
                        pool.sqrt_p_current = bin_sqrt_p;
                        break;
                    } else {
                        let base_consumed = quote_to_base_floor(avail_quote, bin_sqrt_p)?;
                        if base_consumed == 0 {
                            return Err(AdapterError::InsufficientBinLiquidity);
                        }
                        let actual_used_base = base_consumed.min(rem_base);
                        bins.bins[idx].reserve_quote_lamports = 0;
                        bins.bins[idx].reserve_base_atoms = bins.bins[idx]
                            .reserve_base_atoms
                            .checked_add(actual_used_base)
                            .ok_or(AdapterError::MathOverflow)?;
                        gross_sol_out = gross_sol_out
                            .checked_add(avail_quote)
                            .ok_or(AdapterError::MathOverflow)?;
                        rem_base = rem_base.saturating_sub(actual_used_base);
                        pool.sqrt_p_current = bin_sqrt_p;
                    }
                }

                let prev_bin = cur_bin_id - 1;
                if bins.bin_index(prev_bin).is_err() {
                    return Err(AdapterError::InsufficientBinLiquidity);
                }
                cur_bin_id = prev_bin;
                v_a_down = v_a_down.saturating_add(1);
                let prev_idx = bins.bin_index(cur_bin_id)?;
                pool.sqrt_p_current = bins.bins[prev_idx].sqrt_price_q64;

                // Pre-mutation Gate G09 sell price impact check along traversal
                let impact_bps = compute_sell_impact_bps(sqrt_p_before, pool.sqrt_p_current)?;
                if impact_bps > pool.max_sell_impact_bps {
                    return Err(AdapterError::SellImpactExceedsCap {
                        impact_bps,
                        max_bps: pool.max_sell_impact_bps,
                    });
                }
            }

            let impact_bps = compute_sell_impact_bps(sqrt_p_before, pool.sqrt_p_current)?;
            if impact_bps > pool.max_sell_impact_bps {
                return Err(AdapterError::SellImpactExceedsCap {
                    impact_bps,
                    max_bps: pool.max_sell_impact_bps,
                });
            }

            let (base_fee_bps, var_fee_bps, sell_fee_bps) = compute_f_sell_bps(
                pool.f_min_bps,
                pool.f_max_sell_bps,
                pool.a_buy,
                v_a_down,
                pool.bin_step_bps,
            )?;

            // OnlyB quote auto-compounding matching `compounding.rs`:
            // total_sell_fee = ceil(Gross_SOL_Out * f_sell / 10_000)
            // compounded_sol = floor(Gross_SOL_Out * f_sell * compounding_fee_bps / 100_000_000)
            let gross_u128 = gross_sol_out as u128;
            let total_sell_fee = if gross_u128 == 0 || sell_fee_bps == 0 {
                0u64
            } else {
                div_ceil_u128(gross_u128 * (sell_fee_bps as u128), BPS_DENOMINATOR)?
                    .min(gross_u128) as u64
            };
            let compounded_sol = (gross_u128
                * (sell_fee_bps as u128)
                * (pool.compounding_fee_bps as u128)
                / 100_000_000u128) as u64;
            let protocol_fee = total_sell_fee
                .checked_sub(compounded_sol)
                .ok_or(AdapterError::MathOverflow)?;
            let net_sol_out = gross_sol_out - total_sell_fee;

            debug_assert_eq!(
                net_sol_out as u128 + compounded_sol as u128 + protocol_fee as u128,
                gross_sol_out as u128
            );

            let floor_idx = bins.bin_index(pool.floor_bin_id)?;
            bins.bins[floor_idx].reserve_quote_lamports = bins.bins[floor_idx]
                .reserve_quote_lamports
                .checked_add(compounded_sol)
                .ok_or(AdapterError::MathOverflow)?;

            pool.real_vault_lamports = pool
                .real_vault_lamports
                .checked_sub(net_sol_out)
                .ok_or(AdapterError::InsufficientBinLiquidity)?;
            pool.max_extractable_lamports = pool
                .max_extractable_lamports
                .saturating_sub(gross_sol_out)
                .checked_add(compounded_sol)
                .ok_or(AdapterError::MathOverflow)?;
            if pool.max_extractable_lamports > pool.real_vault_lamports {
                return Err(AdapterError::InsolventPool {
                    max_extractable: pool.max_extractable_lamports,
                    real_vault: pool.real_vault_lamports,
                });
            }

            pool.active_bin_id = cur_bin_id;
            pool.v_a_up = v_a_up_start;
            pool.v_a_down = v_a_down;
            pool.compounded_sol_total = pool.compounded_sol_total.saturating_add(compounded_sol);
            pool.protocol_fee_lamports = pool.protocol_fee_lamports.saturating_add(protocol_fee);
            pool.total_volume_quote = pool.total_volume_quote.saturating_add(gross_sol_out);

            let quote = Quote {
                in_amount: quote_params.amount,
                out_amount: net_sol_out,
                fee_amount: total_sell_fee,
                fee_mint: quote_mint,
                fee_pct_bps: sell_fee_bps,
                base_fee_bps,
                variable_fee_bps: var_fee_bps,
                compounded_sol_lamports: compounded_sol,
                protocol_fee_lamports: protocol_fee,
                price_impact_bps: impact_bps,
                post_swap_sqrt_p: pool.sqrt_p_current,
                post_swap_active_bin_id: pool.active_bin_id,
                post_swap_v_a_up: pool.v_a_up,
                post_swap_v_a_down: pool.v_a_down,
                not_enough_liquidity: false,
            };
            Ok((quote, pool, bins))
        }
    }
}

impl Amm for RatchetJupiterAmm {
    fn from_keyed_account(
        keyed_account: &KeyedAccount,
        amm_context: &AmmContext,
    ) -> Result<Self, AdapterError> {
        let pool = RatchetPoolState::from_bytes(&keyed_account.account.data)?;
        let program_id = if keyed_account.account.owner == SYSTEM_PROGRAM_ID {
            DEFAULT_RATCHET_PROGRAM_ID
        } else {
            keyed_account.account.owner
        };
        let placeholder_bins = ActiveBinArrayState {
            bin_step_q64: Q64_ONE,
            pool: keyed_account.key.to_bytes(),
            active_bin_id: pool.active_bin_id,
            floor_bin_id: pool.floor_bin_id,
            start_bin_id: pool.bin_array_start_id,
            bin_step_bps: pool.bin_step_bps,
            bump: pool.bin_array_bump,
            version: pool.version,
            bins: [LiquidityBin::default(); ACTIVE_BIN_ARRAY_LEN],
        };
        Ok(Self {
            program_id,
            pool_key: keyed_account.key,
            pool,
            bin_array: placeholder_bins,
            clock_timestamp: amm_context.clock_timestamp,
            current_slot: amm_context.slot,
        })
    }

    fn label(&self) -> String {
        "Ratchet-DLMM-DAMM".to_string()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    fn key(&self) -> Pubkey {
        self.pool_key
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        vec![Pubkey(self.pool.base_mint), Pubkey(self.pool.quote_mint)]
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        vec![self.pool_key, Pubkey(self.pool.active_bin_array), CLOCK_SYSVAR_ID]
    }

    fn update(&mut self, account_map: &AccountMap) -> Result<(), AdapterError> {
        let pool_acct = account_map
            .get(&self.pool_key)
            .ok_or(AdapterError::MissingAccount(self.pool_key))?;
        self.pool = RatchetPoolState::from_bytes(&pool_acct.data)?;

        let bin_array_key = Pubkey(self.pool.active_bin_array);
        let bin_acct = account_map
            .get(&bin_array_key)
            .ok_or(AdapterError::MissingAccount(bin_array_key))?;
        self.bin_array = ActiveBinArrayState::from_bytes(&bin_acct.data)?;

        if let Some(clock_acct) = account_map.get(&CLOCK_SYSVAR_ID) {
            if clock_acct.data.len() >= 40 {
                // Solana Clock sysvar layout: slot (0..8), ..., unix_timestamp (32..40)
                self.current_slot = read_u64(&clock_acct.data, 0);
                self.clock_timestamp = read_i64(&clock_acct.data, 32);
            }
        }
        Ok(())
    }

    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote, AdapterError> {
        let (quote, _, _) = self.simulate_swap_internal(quote_params)?;
        Ok(quote)
    }

    fn get_swap_and_account_metas(
        &self,
        swap_params: &SwapParams,
    ) -> Result<SwapAndAccountMetas, AdapterError> {
        let base_mint = Pubkey(self.pool.base_mint);
        let quote_mint = Pubkey(self.pool.quote_mint);
        let (direction, user_base_account, user_quote_account) =
            if swap_params.source_mint == quote_mint && swap_params.destination_mint == base_mint {
                (
                    SwapDirection::Buy,
                    swap_params.destination_token_account,
                    swap_params.source_token_account,
                )
            } else if swap_params.source_mint == base_mint
                && swap_params.destination_mint == quote_mint
            {
                (
                    SwapDirection::Sell,
                    swap_params.source_token_account,
                    swap_params.destination_token_account,
                )
            } else {
                return Err(AdapterError::UnsupportedMint(swap_params.source_mint));
            };

        let transfer_fee_treasury =
            Pubkey::derived(b"transfer_fee_treasury", &self.pool_key.0, &self.pool.base_mint);
        let compounding_reserve_vault =
            Pubkey::derived(b"compounding_reserve", &self.pool_key.0, &self.pool.quote_mint);
        let event_authority =
            Pubkey::derived(b"event_authority", &self.program_id.0, &[]);
        let pool_authority_pda = if self.pool.padding[1] == 1 {
            Pubkey::create_program_address(
                &[POOL_AUTHORITY_SEED, &self.pool_key.0, &[self.pool.padding[0]]],
                &self.program_id,
            )
        } else {
            Pubkey::derived(b"pool_authority", &self.pool_key.0, &[])
        };
        let high_water_oracle_pda =
            Pubkey::derived(b"high_water_oracle", &self.pool_key.0, &[]);
        let lower_bin_array_ext =
            Pubkey::derived(b"lower_bin_ext", &self.pool_key.0, &self.pool.active_bin_array);
        let upper_bin_array_ext =
            Pubkey::derived(b"upper_bin_ext", &self.pool_key.0, &self.pool.active_bin_array);

        let jupiter_program = swap_params
            .jupiter_program_id
            .unwrap_or(DEFAULT_JUPITER_PROGRAM_ID);
        let referral_fee_account = swap_params
            .quote_mint_to_referrer
            .unwrap_or(Pubkey(self.pool.creator_fee_vault));

        // Exact 28-account layout matching `sdk/ts-client/src/accounts.ts::resolveSwapAccounts28`
        let account_metas = vec![
            AccountMeta { pubkey: self.pool_key, is_signer: false, is_writable: true },
            AccountMeta { pubkey: Pubkey(self.pool.active_bin_array), is_signer: false, is_writable: true },
            AccountMeta { pubkey: base_mint, is_signer: false, is_writable: false },
            AccountMeta { pubkey: quote_mint, is_signer: false, is_writable: false },
            AccountMeta { pubkey: Pubkey(self.pool.base_vault), is_signer: false, is_writable: true },
            AccountMeta { pubkey: Pubkey(self.pool.quote_vault), is_signer: false, is_writable: true },
            AccountMeta { pubkey: swap_params.token_transfer_authority, is_signer: true, is_writable: false },
            AccountMeta { pubkey: user_base_account, is_signer: false, is_writable: true },
            AccountMeta { pubkey: user_quote_account, is_signer: false, is_writable: true },
            AccountMeta { pubkey: Pubkey(self.pool.creator_fee_vault), is_signer: false, is_writable: true },
            AccountMeta { pubkey: transfer_fee_treasury, is_signer: false, is_writable: true },
            AccountMeta { pubkey: compounding_reserve_vault, is_signer: false, is_writable: true },
            AccountMeta { pubkey: TOKEN_2022_PROGRAM_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: SPL_TOKEN_PROGRAM_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: ASSOCIATED_TOKEN_PROGRAM_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: SYSTEM_PROGRAM_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: RENT_SYSVAR_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: CLOCK_SYSVAR_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: INSTRUCTIONS_SYSVAR_ID, is_signer: false, is_writable: false },
            AccountMeta { pubkey: event_authority, is_signer: false, is_writable: false },
            AccountMeta { pubkey: self.program_id, is_signer: false, is_writable: false },
            AccountMeta { pubkey: pool_authority_pda, is_signer: false, is_writable: false },
            AccountMeta { pubkey: high_water_oracle_pda, is_signer: false, is_writable: true },
            AccountMeta { pubkey: lower_bin_array_ext, is_signer: false, is_writable: true },
            AccountMeta { pubkey: upper_bin_array_ext, is_signer: false, is_writable: true },
            AccountMeta { pubkey: jupiter_program, is_signer: false, is_writable: false },
            AccountMeta { pubkey: referral_fee_account, is_signer: false, is_writable: true },
            AccountMeta { pubkey: MEMO_PROGRAM_ID, is_signer: false, is_writable: false },
        ];

        if account_metas.len() != EXACT_SWAP_ACCOUNT_COUNT
            || account_metas.len() > SIMD0339_MAX_ACCOUNTS
            || RATCHET_SWAP_CPI_DEPTH > MAX_CPI_DEPTH
        {
            return Err(AdapterError::AccountLimitViolation);
        }

        let instruction_data = pack_swap_ix_data(
            matches!(direction, SwapDirection::Buy),
            swap_params.in_amount,
            swap_params.out_amount,
        );

        Ok(SwapAndAccountMetas {
            swap: Swap::RatchetDlmmDamm {
                direction,
                compounding_fee_bps: self.pool.compounding_fee_bps,
                max_sell_impact_bps: self.pool.max_sell_impact_bps,
            },
            account_metas,
            instruction_data,
            cpi_depth: RATCHET_SWAP_CPI_DEPTH,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q64_step_from_bps(bin_step_bps: u16) -> u128 {
        // Exact floor(sqrt(1 + bin_step_bps / 10_000) * 2^64) via 256-bit integer square root
        let num = BPS_DENOMINATOR + bin_step_bps as u128;
        let q_hi = num / BPS_DENOMINATOR;
        let rem = num % BPS_DENOMINATOR;
        let top_mid = (rem << 64) / BPS_DENOMINATOR;
        let rem_mid = (rem << 64) % BPS_DENOMINATOR;
        let low_mid = (rem_mid << 64) / BPS_DENOMINATOR;
        let q_lo = (top_mid << 64) | low_mid;

        let mut low: u128 = 0;
        let mut high: u128 = u128::MAX;
        while low < high {
            let mid = low + ((high - low) >> 1) + 1;
            let sq = U256::mul_u128(mid, mid);
            let m_lo = ((sq.0[1] as u128) << 64) | (sq.0[0] as u128);
            let m_hi = ((sq.0[3] as u128) << 64) | (sq.0[2] as u128);
            if m_hi < q_hi || (m_hi == q_hi && m_lo <= q_lo) {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        low
    }

    fn q64_mul(a: u128, b: u128) -> u128 {
        let prod = U256::mul_u128(a, b);
        ((prod.0[2] as u128) << 64) | (prod.0[1] as u128)
    }

    fn make_clock_account(slot: u64, unix_timestamp: i64) -> Account {
        let mut data = vec![0u8; 40];
        data[0..8].copy_from_slice(&slot.to_le_bytes());
        data[32..40].copy_from_slice(&unix_timestamp.to_le_bytes());
        Account {
            lamports: 1_000_000,
            data,
            owner: SYSTEM_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        }
    }

    fn build_test_fixture(
        bin_step_bps: u16,
        a_buy: u64,
        quote_per_lower_bin: u64,
        base_per_upper_bin: u64,
    ) -> (Pubkey, RatchetPoolState, ActiveBinArrayState) {
        let program_id = DEFAULT_RATCHET_PROGRAM_ID;
        let pool_key = Pubkey::derived(b"pool", b"test", b"v1");
        let bin_array_key = Pubkey::derived(b"bins", &pool_key.0, &[]);
        let base_mint = Pubkey::derived(b"base_mint", &pool_key.0, &[]);
        let quote_mint = WSOL_MINT;

        let start_bin_id = -10;
        let active_bin_id = 0;
        let floor_bin_id = -5;
        let bin_step_q64 = q64_step_from_bps(bin_step_bps);
        // Start sqrt(P) = 0.01 SOL/token => sqrt(0.01) = 0.1 in Q64.64
        let start_sqrt_p = Q64_ONE / 10;

        let mut bins = [LiquidityBin::default(); ACTIVE_BIN_ARRAY_LEN];
        // Build monotonic sqrt_p across bins [-10 .. 19]
        let mut cur_sqrt = start_sqrt_p;
        for _ in 0..(active_bin_id - start_bin_id) {
            cur_sqrt = (cur_sqrt * Q64_ONE) / bin_step_q64;
        }
        let mut s = cur_sqrt;
        for (i, slot) in bins.iter_mut().enumerate() {
            let bin_id = start_bin_id + i as i32;
            *slot = LiquidityBin {
                sqrt_price_q64: s,
                reserve_quote_lamports: if bin_id <= active_bin_id { quote_per_lower_bin } else { 0 },
                reserve_base_atoms: if bin_id >= active_bin_id { base_per_upper_bin } else { 0 },
            };
            s = q64_mul(s, bin_step_q64);
        }

        let active_idx = (active_bin_id - start_bin_id) as usize;
        let sqrt_p_start = bins[active_idx].sqrt_price_q64;
        let sqrt_p_grad = bins[ACTIVE_BIN_ARRAY_LEN - 1].sqrt_price_q64;

        let pool = RatchetPoolState {
            sqrt_p_high_water: sqrt_p_start,
            sqrt_p_current: sqrt_p_start,
            sqrt_p_start,
            sqrt_p_graduation: sqrt_p_grad,
            v_a_up: 0,
            v_a_down: 0,
            a_buy,
            a_sell: a_buy * 5,
            last_decay_timestamp: 1_700_000_000,
            real_vault_lamports: 86_500_000_000,
            max_extractable_lamports: 85_000_000_000,
            compounded_sol_total: 0,
            protocol_fee_lamports: 0,
            graduation_slot: 0,
            total_volume_quote: 0,
            harvested_transfer_fee_atoms: 0,
            active_bin_id,
            floor_bin_id,
            bin_array_start_id: start_bin_id,
            decay_period_seconds: 600,
            compounding_fee_bps: DEFAULT_COMPOUNDING_FEE_BPS,
            f_0_bps: DEFAULT_F_0_BPS,
            f_min_bps: DEFAULT_F_MIN_BPS,
            f_max_sell_bps: IMMUTABLE_F_MAX_SELL_BPS,
            bin_step_bps,
            max_sell_impact_bps: BUFFERED_MAX_SELL_IMPACT_BPS,
            gamma_bps: DEFAULT_GAMMA_BPS,
            transfer_fee_bps: DEFAULT_F_MIN_BPS,
            graduation_status: GRADUATION_STATUS_ACTIVE_CURVE,
            authority_revoked: 1,
            vault_bump: 255,
            pool_bump: 254,
            bin_array_bump: 253,
            version: 1,
            transfer_fee_enabled: 1,
            filter_period_seconds: 30,
            base_mint: base_mint.0,
            quote_mint: quote_mint.0,
            base_vault: Pubkey::derived(b"bv", &pool_key.0, &[]).0,
            quote_vault: Pubkey::derived(b"qv", &pool_key.0, &[]).0,
            authority: [0u8; 32],
            active_bin_array: bin_array_key.0,
            creator_fee_vault: Pubkey::derived(b"cf", &pool_key.0, &[]).0,
            padding: [0u8; 56],
        };

        let bin_array = ActiveBinArrayState {
            bin_step_q64,
            pool: pool_key.0,
            active_bin_id,
            floor_bin_id,
            start_bin_id,
            bin_step_bps,
            bump: 253,
            version: 1,
            bins,
        };

        let _ = program_id;
        (pool_key, pool, bin_array)
    }

    #[test]
    fn test_pda_byte_roundtrip_and_amm_update() {
        let (pool_key, pool, bin_array) = build_test_fixture(50, 50_000, 1_000_000_000, 100_000_000_000);
        let pool_bytes = pool.to_bytes();
        let bin_bytes = bin_array.to_bytes();
        assert_eq!(pool_bytes.len(), 480);
        assert_eq!(bin_bytes.len(), 1_024);

        let parsed_pool = RatchetPoolState::from_bytes(&pool_bytes).unwrap();
        let parsed_bins = ActiveBinArrayState::from_bytes(&bin_bytes).unwrap();
        assert_eq!(parsed_pool, pool);
        assert_eq!(parsed_bins, bin_array);

        let keyed = KeyedAccount {
            key: pool_key,
            account: Account {
                lamports: 3_088_640,
                data: pool_bytes.to_vec(),
                owner: DEFAULT_RATCHET_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
            params: None,
        };
        let ctx = AmmContext {
            clock_timestamp: 1_700_000_000,
            slot: 100,
        };
        let mut amm = RatchetJupiterAmm::from_keyed_account(&keyed, &ctx).unwrap();
        let mut map = AccountMap::new();
        map.insert(pool_key, keyed.account);
        map.insert(
            Pubkey(pool.active_bin_array),
            Account {
                lamports: 5_852_160,
                data: bin_bytes.to_vec(),
                owner: DEFAULT_RATCHET_PROGRAM_ID,
                executable: false,
                rent_epoch: 0,
            },
        );
        map.insert(CLOCK_SYSVAR_ID, make_clock_account(105, 1_700_000_020));
        amm.update(&map).unwrap();

        assert_eq!(amm.current_slot, 105);
        assert_eq!(amm.clock_timestamp, 1_700_000_020);
        assert_eq!(amm.bin_array, bin_array);
        assert_eq!(amm.get_reserve_mints().len(), 2);
    }

    #[test]
    fn test_fee_equations_and_u128_overflow_safety() {
        // div_ceil_u128 exactness
        assert_eq!(div_ceil_u128(0, VOLATILITY_DENOMINATOR).unwrap(), 0);
        assert_eq!(div_ceil_u128(1, VOLATILITY_DENOMINATOR).unwrap(), 1);
        assert_eq!(div_ceil_u128(VOLATILITY_DENOMINATOR, VOLATILITY_DENOMINATOR).unwrap(), 1);
        assert_eq!(div_ceil_u128(VOLATILITY_DENOMINATOR + 1, VOLATILITY_DENOMINATOR).unwrap(), 2);

        // Slot-0 buy fee = 80% + ceil(A_buy * (v_a^+ * s)^2 / 10^11)
        let (base, var, total) = compute_f_buy_bps(8_000, 25, 9_000, 0, 50_000, 200, 25).unwrap();
        assert_eq!(base, 8_000);
        assert_eq!(var, 13);
        assert_eq!(total, 8_013);

        // High-water ratchet decay to f_min = 25 bps
        let (base_mature, _, total_mature) =
            compute_f_buy_bps(8_000, 25, 9_000, 120, 50_000, 0, 25).unwrap();
        assert_eq!(base_mature, 25);
        assert_eq!(total_mature, 25);

        // Asymmetric 5x dump-brake capped at 25% (2,500 bps) even for u64::MAX v_a_down
        let (_, var_sell, sell_mild) = compute_f_sell_bps(25, 2_500, 50_000, 200, 25).unwrap();
        assert_eq!(var_sell, 63);
        assert_eq!(sell_mild, 88);

        let (_, _, sell_extreme) =
            compute_f_sell_bps(25, 2_500, 50_000, u64::MAX, 25).unwrap();
        assert_eq!(sell_extreme, 2_500);
    }

    #[test]
    fn test_28_account_swap_metas_and_sell_impact_cap() {
        let (pool_key, pool, bin_array) = build_test_fixture(100, 50_000, 2_000_000_000, 100_000_000_000);
        let amm = RatchetJupiterAmm {
            program_id: DEFAULT_RATCHET_PROGRAM_ID,
            pool_key,
            pool,
            bin_array,
            clock_timestamp: 1_700_000_000,
            current_slot: 1,
        };

        let metas = amm
            .get_swap_and_account_metas(&SwapParams {
                swap_mode: SwapMode::ExactIn,
                in_amount: 1_000_000_000,
                out_amount: 0,
                source_mint: Pubkey(pool.quote_mint),
                destination_mint: Pubkey(pool.base_mint),
                source_token_account: Pubkey([7u8; 32]),
                destination_token_account: Pubkey([8u8; 32]),
                token_transfer_authority: Pubkey([9u8; 32]),
                open_orders_address: None,
                quote_mint_to_referrer: None,
                jupiter_program_id: None,
            })
            .unwrap();

        assert_eq!(metas.account_metas.len(), 28);
        assert_eq!(metas.cpi_depth, 2);
        assert!(metas.cpi_depth <= 4);
        assert!(metas.account_metas.len() <= 255);
        assert!(metas.account_metas[6].is_signer);
        assert_eq!(metas.account_metas[12].pubkey, TOKEN_2022_PROGRAM_ID);

        // Oversized single-leg sell breaching 700 bps price impact cap must fail cleanly (Gate G09)
        let huge_sell = amm.quote(&QuoteParams {
            amount: 1_800_000_000_000, // sweeps > 8 downward bins (~800+ bps)
            input_mint: Pubkey(pool.base_mint),
            output_mint: Pubkey(pool.quote_mint),
            swap_mode: SwapMode::ExactIn,
        });
        assert!(matches!(huge_sell, Err(AdapterError::SellImpactExceedsCap { .. })));
    }

    #[test]
    fn shadow_mainnet_multi_wallet_pump_and_dump_cohort() {
        // Shadow Mainnet Replay Fixture:
        // - bin_step = 100 bps (1% per bin, 7-bin drop ~680 bps <= 700 bps Gate G09 cap)
        // - A_buy = 50_000_000 -> A_sell = 250_000_000 (hits 25% dump-brake after rapid downward crossings)
        // - 5 SOL quote liquidity seeded per lower bin, 50B base atoms (~0.5 SOL) per upper bin
        let (pool_key, pool, bin_array) =
            build_test_fixture(100, 50_000_000, 5_000_000_000, 50_000_000_000);
        let t0 = pool.last_decay_timestamp;
        let base_mint = Pubkey(pool.base_mint);
        let quote_mint = Pubkey(pool.quote_mint);

        let mut amm = RatchetJupiterAmm {
            program_id: DEFAULT_RATCHET_PROGRAM_ID,
            pool_key,
            pool,
            bin_array,
            clock_timestamp: t0,
            current_slot: 0,
        };

        let floor_idx = amm.bin_array.bin_index(amm.pool.floor_bin_id).unwrap();
        let initial_floor_quote = amm.bin_array.bins[floor_idx].reserve_quote_lamports;

        // --- PHASE 1: Multi-wallet Slot 0..2 sniper & pump cohort (3 wallets buying) ---
        let mut pump_quotes = Vec::new();
        for (wallet_idx, sol_in) in [3_000_000_000u64, 6_000_000_000, 8_000_000_000].iter().enumerate() {
            let ts = t0 + (wallet_idx as i64) * 4; // within 30s filter window: accumulators remain hot
            // Re-run slot update via AccountMap round-trip to verify live PDA parsing on every slot
            let mut map = AccountMap::new();
            map.insert(
                pool_key,
                Account {
                    lamports: 3_088_640,
                    data: amm.pool.to_bytes().to_vec(),
                    owner: DEFAULT_RATCHET_PROGRAM_ID,
                    executable: false,
                    rent_epoch: 0,
                },
            );
            map.insert(
                Pubkey(amm.pool.active_bin_array),
                Account {
                    lamports: 5_852_160,
                    data: amm.bin_array.to_bytes().to_vec(),
                    owner: DEFAULT_RATCHET_PROGRAM_ID,
                    executable: false,
                    rent_epoch: 0,
                },
            );
            map.insert(CLOCK_SYSVAR_ID, make_clock_account(wallet_idx as u64, ts));
            amm.update(&map).unwrap();

            let q = amm
                .execute_simulated_swap(
                    &QuoteParams {
                        amount: *sol_in,
                        input_mint: quote_mint,
                        output_mint: base_mint,
                        swap_mode: SwapMode::ExactIn,
                    },
                    ts,
                )
                .unwrap();
            assert!(q.out_amount > 0);
            pump_quotes.push(q);
        }

        // Slot-0 buyer pays 80% base buy fee; upward movement ratchets high-water & lowers base fee
        assert_eq!(pump_quotes[0].base_fee_bps, 8_000);
        assert!(amm.pool.sqrt_p_high_water > amm.pool.sqrt_p_start);
        assert!(amm.pool.v_a_up >= 6);
        assert!(pump_quotes[2].base_fee_bps < pump_quotes[0].base_fee_bps);
        let peak_v_a_up = amm.pool.v_a_up;
        let peak_high_water = amm.pool.sqrt_p_high_water;

        // --- PHASE 2: Real-time accumulator decay rate verification (50% per 30s filter step) ---
        // Still inside filter_period (30s): dt = 20s => 0% decay
        let hot_v_a = decay_accumulator(peak_v_a_up, amm.pool.last_decay_timestamp, amm.pool.last_decay_timestamp + 20, 30, 600);
        assert_eq!(hot_v_a, peak_v_a_up);

        // One filter step elapsed (30s): 50% decay
        let step1_v_a = decay_accumulator(peak_v_a_up, amm.pool.last_decay_timestamp, amm.pool.last_decay_timestamp + 30, 30, 600);
        assert_eq!(step1_v_a, peak_v_a_up / 2);

        // Two filter steps elapsed (60s): 25% remaining
        let step2_v_a = decay_accumulator(peak_v_a_up, amm.pool.last_decay_timestamp, amm.pool.last_decay_timestamp + 60, 30, 600);
        assert_eq!(step2_v_a, peak_v_a_up / 4);

        // Past decay_period (600s): dt = 605s => 100% decayed to 0
        let cooled_v_a = decay_accumulator(peak_v_a_up, amm.pool.last_decay_timestamp, amm.pool.last_decay_timestamp + 605, 30, 600);
        assert_eq!(cooled_v_a, 0);

        // --- PHASE 3: Coordinated multi-wallet dump cohort activating 5x dump-brake up to 25% ---
        let mut dump_sell_fees = Vec::new();
        let mut prev_floor_quote = amm.bin_array.bins[floor_idx].reserve_quote_lamports;
        assert_eq!(prev_floor_quote, initial_floor_quote);

        // 4 wallets dump in rapid succession (< 30s apart) across bins without breaching 700 bps single-leg cap
        for dump_idx in 0..4 {
            let ts = amm.pool.last_decay_timestamp + 3;
            let tokens_to_sell = 150_000_000_000u64;
            let q = amm
                .execute_simulated_swap(
                    &QuoteParams {
                        amount: tokens_to_sell,
                        input_mint: base_mint,
                        output_mint: quote_mint,
                        swap_mode: SwapMode::ExactIn,
                    },
                    ts,
                )
                .unwrap();

            assert!(q.price_impact_bps <= amm.pool.max_sell_impact_bps);
            assert!(q.compounded_sol_lamports > 0);
            assert_eq!(
                q.out_amount + q.compounded_sol_lamports + q.protocol_fee_lamports,
                q.out_amount + q.fee_amount
            );

            // OnlyB auto-compounded floor bin quote reserve grows strictly on every dump
            let cur_floor_quote = amm.bin_array.bins[floor_idx].reserve_quote_lamports;
            assert!(cur_floor_quote > prev_floor_quote);
            prev_floor_quote = cur_floor_quote;

            // High-water mark never regresses during the dump
            assert_eq!(amm.pool.sqrt_p_high_water, peak_high_water);
            dump_sell_fees.push(q.fee_pct_bps);
            let _ = dump_idx;
        }

        // Verify dump-brake escalated and hit the immutable 25% (2,500 bps) sell fee ceiling
        assert!(dump_sell_fees.last().copied().unwrap() > dump_sell_fees[0]);
        assert_eq!(
            *dump_sell_fees.last().unwrap(),
            IMMUTABLE_F_MAX_SELL_BPS,
            "multi-wallet dump cohort must trigger 25% (2,500 bps) dump-brake ceiling; got {:?}",
            dump_sell_fees
        );
        assert!(amm.pool.compounded_sol_total > 0);
        assert_eq!(
            prev_floor_quote - initial_floor_quote,
            amm.pool.compounded_sol_total
        );
        assert!(amm.pool.max_extractable_lamports <= amm.pool.real_vault_lamports);

        // --- PHASE 4: Post-dump cooldown decays v_a_down and releases dump-brake ---
        let cooldown_ts = amm.pool.last_decay_timestamp + 650;
        let post_cooldown_quote = amm
            .execute_simulated_swap(
                &QuoteParams {
                    amount: 10_000_000_000,
                    input_mint: base_mint,
                    output_mint: quote_mint,
                    swap_mode: SwapMode::ExactIn,
                },
                cooldown_ts,
            )
            .unwrap();
        assert!(
            post_cooldown_quote.fee_pct_bps < IMMUTABLE_F_MAX_SELL_BPS,
            "post-decay cooldown must relax sell fee below 25%; got {}",
            post_cooldown_quote.fee_pct_bps
        );
    }

    #[test]
    fn test_pda_validation_rejects_bad_lengths_and_invariants() {
        assert!(matches!(
            RatchetPoolState::from_bytes(&[0u8; 479]),
            Err(AdapterError::InvalidAccountDataLen { expected: 480, actual: 479 })
        ));
        assert!(matches!(
            ActiveBinArrayState::from_bytes(&[0u8; 1023]),
            Err(AdapterError::InvalidAccountDataLen { expected: 1024, actual: 1023 })
        ));

        let (_, mut pool, _) = build_test_fixture(50, 50_000, 1_000_000_000, 100_000_000_000);
        pool.compounding_fee_bps = 7_499;
        assert!(matches!(pool.validate(), Err(AdapterError::InvalidPoolConfig)));

        pool.compounding_fee_bps = DEFAULT_COMPOUNDING_FEE_BPS;
        pool.f_max_sell_bps = 2_501;
        assert!(matches!(pool.validate(), Err(AdapterError::InvalidPoolConfig)));

        pool.f_max_sell_bps = IMMUTABLE_F_MAX_SELL_BPS;
        pool.a_sell = pool.a_buy * 4;
        assert!(matches!(pool.validate(), Err(AdapterError::InvalidPoolConfig)));

        pool.a_sell = pool.a_buy * 5;
        pool.max_extractable_lamports = pool.real_vault_lamports + 1;
        assert!(matches!(pool.validate(), Err(AdapterError::InsolventPool { .. })));
    }

    #[test]
    fn test_wire_packers_sha256_and_real_pda_derivation() {
        // FIPS 180-4 SHA-256("abc") known test vector
        assert_eq!(
            sha256(b"abc"),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
                0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
                0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
                0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
            ]
        );

        let (pool_key, mut pool, bin_array) =
            build_test_fixture(50, 50_000, 1_000_000_000, 100_000_000_000);
        pool.padding[0] = 252;
        pool.padding[1] = 1;
        let expected_auth = Pubkey::create_program_address(
            &[POOL_AUTHORITY_SEED, &pool_key.0, &[252]],
            &DEFAULT_RATCHET_PROGRAM_ID,
        );

        let amm = RatchetJupiterAmm {
            program_id: DEFAULT_RATCHET_PROGRAM_ID,
            pool_key,
            pool,
            bin_array,
            clock_timestamp: 1_700_000_000,
            current_slot: 1,
        };
        let metas = amm
            .get_swap_and_account_metas(&SwapParams {
                swap_mode: SwapMode::ExactIn,
                in_amount: 500_000_000,
                out_amount: 100,
                source_mint: Pubkey(pool.quote_mint),
                destination_mint: Pubkey(pool.base_mint),
                source_token_account: Pubkey([7u8; 32]),
                destination_token_account: Pubkey([8u8; 32]),
                token_transfer_authority: Pubkey([9u8; 32]),
                open_orders_address: None,
                quote_mint_to_referrer: None,
                jupiter_program_id: None,
            })
            .unwrap();
        assert_eq!(metas.account_metas[21].pubkey, expected_auth);
        assert_eq!(metas.instruction_data, pack_swap_ix_data(true, 500_000_000, 100));
        assert_eq!(metas.instruction_data[0], IX_SWAP);
        assert_eq!(metas.instruction_data[1], 1);

        let init_pool = pack_initialize_pool_ix_data(
            Q64_ONE,
            Q64_ONE * 2,
            50_000,
            25,
            0,
            -8,
            -8,
            7_500,
            9_000,
            700,
            0,
            0,
        );
        assert_eq!(init_pool.len(), 64);
        assert_eq!(init_pool[0], IX_INITIALIZE_POOL);

        let init_bins = pack_initialize_bin_array_ix_data(-8, 0, -8, 25, Q64_ONE);
        assert_eq!(init_bins.len(), 31);
        assert_eq!(init_bins[0], IX_INITIALIZE_BIN_ARRAY);

        let seed_ix = pack_seed_initial_bins_ix_data(1_000, 2_000, &[(0, 500, 1_000), (1, 500, 1_000)]);
        assert_eq!(seed_ix.len(), 18 + 2 * 20);
        assert_eq!(seed_ix[0], IX_SEED_INITIAL_BINS);
        assert_eq!(seed_ix[1], 2);

        assert_eq!(pack_revoke_and_open_slot0_ix_data(), [IX_REVOKE_AND_OPEN_SLOT0]);
        assert_eq!(pack_graduate_in_place_ix_data(), [IX_GRADUATE_IN_PLACE]);
        assert_eq!(
            pack_harvest_fallback_transfer_fees_ix_data(0),
            [IX_HARVEST_FALLBACK_TRANSFER_FEES, 0]
        );
    }
}

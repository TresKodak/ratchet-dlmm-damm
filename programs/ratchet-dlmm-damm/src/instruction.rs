use solana_program::{
    instruction::{AccountMeta, Instruction},
    program_error::ProgramError,
    pubkey::Pubkey,
    system_program,
};

use crate::cpi::{SPL_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, WSOL_MINT_ID};

/// Canonical PDA seed for `RatchetPoolPda` (`[b"ratchet_pool", base_mint]`).
pub const POOL_SEED: &[u8] = b"ratchet_pool";
/// Canonical PDA seed for `ActiveBinArrayPda` (`[b"bin_array", pool_pda, &start_bin_id.to_le_bytes()]`).
pub const BIN_ARRAY_SEED: &[u8] = b"bin_array";
/// Canonical PDA seed for `PoolAuthorityPda` (`[b"pool_authority", pool_pda]`).
pub const POOL_AUTHORITY_SEED: &[u8] = b"pool_authority";
/// Canonical PDA seed for `BaseVault` (`[b"base_vault", pool_pda]`).
pub const BASE_VAULT_SEED: &[u8] = b"base_vault";
/// Canonical PDA seed for `QuoteVault` (`[b"quote_vault", pool_pda]`).
pub const QUOTE_VAULT_SEED: &[u8] = b"quote_vault";

pub const IX_INITIALIZE_POOL: u8 = 0x01;
pub const IX_INITIALIZE_BIN_ARRAY: u8 = 0x02;
pub const IX_SEED_INITIAL_BINS: u8 = 0x03;
pub const IX_REVOKE_AND_OPEN_SLOT0: u8 = 0x04;
pub const IX_SWAP: u8 = 0x05;
pub const IX_GRADUATE_IN_PLACE: u8 = 0x06;
pub const IX_HARVEST_FALLBACK_TRANSFER_FEES: u8 = 0x07;

pub const HARVEST_MODE_TO_MINT: u8 = 0;
pub const HARVEST_MODE_WITHDRAW_FROM_MINT: u8 = 1;

/// Derives `RatchetPoolPda` address and bump.
pub fn derive_pool_pda(base_mint: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[POOL_SEED, base_mint.as_ref()], program_id)
}

/// Derives `ActiveBinArrayPda` address and bump.
pub fn derive_bin_array_pda(
    pool_pda: &Pubkey,
    start_bin_id: i32,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[BIN_ARRAY_SEED, pool_pda.as_ref(), &start_bin_id.to_le_bytes()],
        program_id,
    )
}

/// Derives `PoolAuthorityPda` address and bump.
pub fn derive_pool_authority_pda(pool_pda: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[POOL_AUTHORITY_SEED, pool_pda.as_ref()], program_id)
}

/// Derives canonical `BaseVault` PDA address and bump.
pub fn derive_base_vault_pda(pool_pda: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[BASE_VAULT_SEED, pool_pda.as_ref()], program_id)
}

/// Derives canonical `QuoteVault` PDA address and bump.
pub fn derive_quote_vault_pda(pool_pda: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[QUOTE_VAULT_SEED, pool_pda.as_ref()], program_id)
}

/// Arguments for `0x01 InitializePool` (63 bytes after discriminator = 64B total).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InitializePoolArgs {
    pub sqrt_p_start: u128,
    pub sqrt_p_graduation: u128,
    pub a_buy: u64,
    pub bin_step_bps: u16,
    pub active_bin_id: i32,
    pub floor_bin_id: i32,
    pub bin_array_start_id: i32,
    pub compounding_fee_bps: u16,
    pub gamma_bps: u16,
    pub max_sell_impact_bps: u16,
    pub transfer_fee_bps: u16,
    pub flags: u8,
}

/// Arguments for `0x02 InitializeBinArray` (30 bytes after discriminator = 31B total).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InitializeBinArrayArgs {
    pub start_bin_id: i32,
    pub active_bin_id: i32,
    pub floor_bin_id: i32,
    pub bin_step_bps: u16,
    pub start_sqrt_price_q64: u128,
}

/// Single bin liquidity seed entry (20 bytes).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BinSeedEntry {
    pub bin_id: i32,
    pub reserve_quote_lamports: u64,
    pub reserve_base_atoms: u64,
}

/// Arguments for `0x03 SeedInitialBins` (18B header + `20 * entries.len()`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedInitialBinsArgs {
    pub quote_deposit_lamports: u64,
    pub base_deposit_atoms: u64,
    pub entries: Vec<BinSeedEntry>,
}

/// Arguments for `0x05 Swap` (18 bytes total).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SwapArgs {
    pub is_buy: u8,
    pub amount_in: u64,
    pub min_amount_out: u64,
}

/// Parsed binary instruction variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RatchetInstruction {
    InitializePool(InitializePoolArgs),
    InitializeBinArray(InitializeBinArrayArgs),
    SeedInitialBins(SeedInitialBinsArgs),
    RevokeAndOpenSlot0,
    Swap(SwapArgs),
    GraduateInPlace,
    HarvestFallbackTransferFees { mode: u8 },
}

#[inline]
fn read_u16_le(buf: &[u8], off: usize) -> Result<u16, ProgramError> {
    let bytes: [u8; 2] = buf
        .get(off..off + 2)
        .and_then(|s| s.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)?;
    Ok(u16::from_le_bytes(bytes))
}

#[inline]
fn read_i32_le(buf: &[u8], off: usize) -> Result<i32, ProgramError> {
    let bytes: [u8; 4] = buf
        .get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)?;
    Ok(i32::from_le_bytes(bytes))
}

#[inline]
fn read_u64_le(buf: &[u8], off: usize) -> Result<u64, ProgramError> {
    let bytes: [u8; 8] = buf
        .get(off..off + 8)
        .and_then(|s| s.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)?;
    Ok(u64::from_le_bytes(bytes))
}

#[inline]
fn read_u128_le(buf: &[u8], off: usize) -> Result<u128, ProgramError> {
    let bytes: [u8; 16] = buf
        .get(off..off + 16)
        .and_then(|s| s.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)?;
    Ok(u128::from_le_bytes(bytes))
}

impl InitializePoolArgs {
    pub fn pack(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.push(IX_INITIALIZE_POOL);
        out.extend_from_slice(&self.sqrt_p_start.to_le_bytes());
        out.extend_from_slice(&self.sqrt_p_graduation.to_le_bytes());
        out.extend_from_slice(&self.a_buy.to_le_bytes());
        out.extend_from_slice(&self.bin_step_bps.to_le_bytes());
        out.extend_from_slice(&self.active_bin_id.to_le_bytes());
        out.extend_from_slice(&self.floor_bin_id.to_le_bytes());
        out.extend_from_slice(&self.bin_array_start_id.to_le_bytes());
        out.extend_from_slice(&self.compounding_fee_bps.to_le_bytes());
        out.extend_from_slice(&self.gamma_bps.to_le_bytes());
        out.extend_from_slice(&self.max_sell_impact_bps.to_le_bytes());
        out.extend_from_slice(&self.transfer_fee_bps.to_le_bytes());
        out.push(self.flags);
        out
    }

    pub fn unpack(rest: &[u8]) -> Result<Self, ProgramError> {
        if rest.len() < 63 {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            sqrt_p_start: read_u128_le(rest, 0)?,
            sqrt_p_graduation: read_u128_le(rest, 16)?,
            a_buy: read_u64_le(rest, 32)?,
            bin_step_bps: read_u16_le(rest, 40)?,
            active_bin_id: read_i32_le(rest, 42)?,
            floor_bin_id: read_i32_le(rest, 46)?,
            bin_array_start_id: read_i32_le(rest, 50)?,
            compounding_fee_bps: read_u16_le(rest, 54)?,
            gamma_bps: read_u16_le(rest, 56)?,
            max_sell_impact_bps: read_u16_le(rest, 58)?,
            transfer_fee_bps: read_u16_le(rest, 60)?,
            flags: rest[62],
        })
    }
}

impl InitializeBinArrayArgs {
    pub fn pack(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(31);
        out.push(IX_INITIALIZE_BIN_ARRAY);
        out.extend_from_slice(&self.start_bin_id.to_le_bytes());
        out.extend_from_slice(&self.active_bin_id.to_le_bytes());
        out.extend_from_slice(&self.floor_bin_id.to_le_bytes());
        out.extend_from_slice(&self.bin_step_bps.to_le_bytes());
        out.extend_from_slice(&self.start_sqrt_price_q64.to_le_bytes());
        out
    }

    pub fn unpack(rest: &[u8]) -> Result<Self, ProgramError> {
        if rest.len() < 30 {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            start_bin_id: read_i32_le(rest, 0)?,
            active_bin_id: read_i32_le(rest, 4)?,
            floor_bin_id: read_i32_le(rest, 8)?,
            bin_step_bps: read_u16_le(rest, 12)?,
            start_sqrt_price_q64: read_u128_le(rest, 14)?,
        })
    }
}

impl SeedInitialBinsArgs {
    pub fn pack(&self) -> Vec<u8> {
        let count = self.entries.len().min(u8::MAX as usize) as u8;
        let mut out = Vec::with_capacity(18 + (count as usize) * 20);
        out.push(IX_SEED_INITIAL_BINS);
        out.push(count);
        out.extend_from_slice(&self.quote_deposit_lamports.to_le_bytes());
        out.extend_from_slice(&self.base_deposit_atoms.to_le_bytes());
        for entry in self.entries.iter().take(count as usize) {
            out.extend_from_slice(&entry.bin_id.to_le_bytes());
            out.extend_from_slice(&entry.reserve_quote_lamports.to_le_bytes());
            out.extend_from_slice(&entry.reserve_base_atoms.to_le_bytes());
        }
        out
    }

    pub fn unpack(rest: &[u8]) -> Result<Self, ProgramError> {
        if rest.len() < 17 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let count = rest[0] as usize;
        let expected_len = 17usize
            .checked_add(
                count
                    .checked_mul(20)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )
            .ok_or(ProgramError::InvalidInstructionData)?;
        if count == 0 || rest.len() < expected_len {
            return Err(ProgramError::InvalidInstructionData);
        }
        let quote_deposit_lamports = read_u64_le(rest, 1)?;
        let base_deposit_atoms = read_u64_le(rest, 9)?;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let off = 17 + i * 20;
            entries.push(BinSeedEntry {
                bin_id: read_i32_le(rest, off)?,
                reserve_quote_lamports: read_u64_le(rest, off + 4)?,
                reserve_base_atoms: read_u64_le(rest, off + 12)?,
            });
        }
        Ok(Self {
            quote_deposit_lamports,
            base_deposit_atoms,
            entries,
        })
    }
}

impl SwapArgs {
    pub fn pack(&self) -> [u8; 18] {
        let mut out = [0u8; 18];
        out[0] = IX_SWAP;
        out[1] = self.is_buy;
        out[2..10].copy_from_slice(&self.amount_in.to_le_bytes());
        out[10..18].copy_from_slice(&self.min_amount_out.to_le_bytes());
        out
    }

    #[inline]
    pub fn unpack_fast(rest: &[u8]) -> Result<Self, ProgramError> {
        if rest.len() < 17 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let is_buy = rest[0];
        if is_buy > 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        Ok(Self {
            is_buy,
            amount_in: read_u64_le(rest, 1)?,
            min_amount_out: read_u64_le(rest, 9)?,
        })
    }
}

impl RatchetInstruction {
    pub fn unpack(data: &[u8]) -> Result<Self, ProgramError> {
        let (&tag, rest) = data
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        match tag {
            IX_INITIALIZE_POOL => Ok(Self::InitializePool(InitializePoolArgs::unpack(rest)?)),
            IX_INITIALIZE_BIN_ARRAY => {
                Ok(Self::InitializeBinArray(InitializeBinArrayArgs::unpack(rest)?))
            }
            IX_SEED_INITIAL_BINS => Ok(Self::SeedInitialBins(SeedInitialBinsArgs::unpack(rest)?)),
            IX_REVOKE_AND_OPEN_SLOT0 => Ok(Self::RevokeAndOpenSlot0),
            IX_SWAP => Ok(Self::Swap(SwapArgs::unpack_fast(rest)?)),
            IX_GRADUATE_IN_PLACE => Ok(Self::GraduateInPlace),
            IX_HARVEST_FALLBACK_TRANSFER_FEES => {
                let &mode = rest.first().ok_or(ProgramError::InvalidInstructionData)?;
                Ok(Self::HarvestFallbackTransferFees { mode })
            }
            _ => Err(ProgramError::InvalidInstructionData),
        }
    }
}

/// Builds `0x01 InitializePool` instruction.
#[allow(clippy::too_many_arguments)]
pub fn initialize_pool_ix(
    program_id: &Pubkey,
    payer: &Pubkey,
    base_mint: &Pubkey,
    creator_fee_vault: &Pubkey,
    base_vault_override: Option<Pubkey>,
    quote_vault_override: Option<Pubkey>,
    args: InitializePoolArgs,
) -> Instruction {
    let (pool_pda, _) = derive_pool_pda(base_mint, program_id);
    let (bin_array_pda, _) =
        derive_bin_array_pda(&pool_pda, args.bin_array_start_id, program_id);
    let (pool_authority_pda, _) = derive_pool_authority_pda(&pool_pda, program_id);
    let base_vault = base_vault_override
        .unwrap_or_else(|| derive_base_vault_pda(&pool_pda, program_id).0);
    let quote_vault = quote_vault_override
        .unwrap_or_else(|| derive_quote_vault_pda(&pool_pda, program_id).0);

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(pool_pda, false),
            AccountMeta::new_readonly(bin_array_pda, false),
            AccountMeta::new_readonly(pool_authority_pda, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new_readonly(WSOL_MINT_ID, false),
            AccountMeta::new(base_vault, false),
            AccountMeta::new(quote_vault, false),
            AccountMeta::new_readonly(*creator_fee_vault, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ],
        data: args.pack(),
    }
}

/// Builds `0x02 InitializeBinArray` instruction.
pub fn initialize_bin_array_ix(
    program_id: &Pubkey,
    payer: &Pubkey,
    pool_pda: &Pubkey,
    args: InitializeBinArrayArgs,
) -> Instruction {
    let (bin_array_pda, _) = derive_bin_array_pda(pool_pda, args.start_bin_id, program_id);
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*pool_pda, false),
            AccountMeta::new(bin_array_pda, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data: args.pack(),
    }
}

/// Builds `0x03 SeedInitialBins` instruction.
#[allow(clippy::too_many_arguments)]
pub fn seed_initial_bins_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    pool_pda: &Pubkey,
    bin_array_pda: &Pubkey,
    base_mint: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
    depositor_base_account: &Pubkey,
    depositor_quote_account: &Pubkey,
    args: SeedInitialBinsArgs,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new(*bin_array_pda, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new(*base_vault, false),
            AccountMeta::new(*quote_vault, false),
            AccountMeta::new(*depositor_base_account, false),
            AccountMeta::new(*depositor_quote_account, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ],
        data: args.pack(),
    }
}

/// Builds `0x04 RevokeAndOpenSlot0` instruction.
pub fn revoke_and_open_slot0_ix(
    program_id: &Pubkey,
    authority: &Pubkey,
    pool_pda: &Pubkey,
    bin_array_pda: &Pubkey,
    base_mint: &Pubkey,
    quote_vault: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new_readonly(*bin_array_pda, false),
            AccountMeta::new(*base_mint, false),
            AccountMeta::new_readonly(*quote_vault, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        ],
        data: vec![IX_REVOKE_AND_OPEN_SLOT0],
    }
}

/// Builds compact 12-account `0x05 Swap` instruction.
#[allow(clippy::too_many_arguments)]
pub fn swap_ix(
    program_id: &Pubkey,
    pool_pda: &Pubkey,
    bin_array_pda: &Pubkey,
    base_mint: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
    user_transfer_authority: &Pubkey,
    user_base_account: &Pubkey,
    user_quote_account: &Pubkey,
    args: SwapArgs,
) -> Instruction {
    let (pool_authority_pda, _) = derive_pool_authority_pda(pool_pda, program_id);
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new(*bin_array_pda, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new_readonly(WSOL_MINT_ID, false),
            AccountMeta::new(*base_vault, false),
            AccountMeta::new(*quote_vault, false),
            AccountMeta::new_readonly(*user_transfer_authority, true),
            AccountMeta::new(*user_base_account, false),
            AccountMeta::new(*user_quote_account, false),
            AccountMeta::new_readonly(pool_authority_pda, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ],
        data: args.pack().to_vec(),
    }
}

/// Builds `0x06 GraduateInPlace` instruction.
pub fn graduate_in_place_ix(
    program_id: &Pubkey,
    pool_pda: &Pubkey,
    bin_array_pda: &Pubkey,
    quote_vault: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new_readonly(*bin_array_pda, false),
            AccountMeta::new_readonly(*quote_vault, false),
        ],
        data: vec![IX_GRADUATE_IN_PLACE],
    }
}

/// Builds `0x07 HarvestFallbackTransferFees` mode 0 (write-lock-free harvest to mint).
pub fn harvest_withheld_to_mint_ix(
    program_id: &Pubkey,
    base_mint: &Pubkey,
    source_accounts: &[Pubkey],
) -> Instruction {
    let mut accounts = Vec::with_capacity(2 + source_accounts.len());
    accounts.push(AccountMeta::new(*base_mint, false));
    accounts.push(AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false));
    for src in source_accounts {
        accounts.push(AccountMeta::new(*src, false));
    }
    Instruction {
        program_id: *program_id,
        accounts,
        data: vec![IX_HARVEST_FALLBACK_TRANSFER_FEES, HARVEST_MODE_TO_MINT],
    }
}

/// Builds `0x07 HarvestFallbackTransferFees` mode 1 (batched withdraw withheld from mint to destination).
pub fn withdraw_withheld_from_mint_ix(
    program_id: &Pubkey,
    pool_pda: &Pubkey,
    base_mint: &Pubkey,
    destination: &Pubkey,
    withdraw_withheld_authority: &Pubkey,
    authority_is_signer: bool,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new(*base_mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*withdraw_withheld_authority, authority_is_signer),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
        ],
        data: vec![
            IX_HARVEST_FALLBACK_TRANSFER_FEES,
            HARVEST_MODE_WITHDRAW_FROM_MINT,
        ],
    }
}

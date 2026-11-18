use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    system_instruction, system_program,
};

/// Canonical SPL Token program ID (`TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// Canonical SPL Token-2022 program ID (`TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`).
pub const TOKEN_2022_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

/// Canonical Wrapped SOL mint (`So11111111111111111111111111111111111111112`).
pub const WSOL_MINT_ID: Pubkey =
    solana_program::pubkey!("So11111111111111111111111111111111111111112");

/// Standard SPL Token account length (165 bytes).
pub const SPL_TOKEN_ACCOUNT_LEN: usize = 165;

/// Token-2022 token account length with `TransferFeeAmount` extension (165 + 1 + 4 + 8 = 178 bytes).
pub const TOKEN_2022_TRANSFER_FEE_ACCOUNT_LEN: usize = 178;

/// Token-2022 `AuthorityType::MintTokens`.
pub const AUTHORITY_TYPE_MINT_TOKENS: u8 = 0;
/// Token-2022 `AuthorityType::FreezeAccount`.
pub const AUTHORITY_TYPE_FREEZE_ACCOUNT: u8 = 1;
/// Token-2022 `AuthorityType::TransferFeeConfig`.
pub const AUTHORITY_TYPE_TRANSFER_FEE_CONFIG: u8 = 4;

/// Reads a token account's mint (`0..32`).
#[inline]
pub fn read_token_account_mint(data: &[u8]) -> Result<[u8; 32], ProgramError> {
    if data.len() < SPL_TOKEN_ACCOUNT_LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&data[0..32]);
    Ok(out)
}

/// Reads a token account's owner (`32..64`).
#[inline]
pub fn read_token_account_owner(data: &[u8]) -> Result<[u8; 32], ProgramError> {
    if data.len() < SPL_TOKEN_ACCOUNT_LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&data[32..64]);
    Ok(out)
}

/// Reads a token account's `amount: u64` (`64..72`) directly without full deserialization.
#[inline]
pub fn read_token_account_amount(data: &[u8]) -> Result<u64, ProgramError> {
    let bytes: [u8; 8] = data
        .get(64..72)
        .and_then(|s| s.try_into().ok())
        .ok_or(ProgramError::InvalidAccountData)?;
    Ok(u64::from_le_bytes(bytes))
}

/// Reads a token account's initialization state byte (`108`: `1` = Initialized, `2` = Frozen).
#[inline]
pub fn read_token_account_state(data: &[u8]) -> Result<u8, ProgramError> {
    data.get(108)
        .copied()
        .ok_or(ProgramError::InvalidAccountData)
}

/// Reads a Mint's `decimals` byte (`44`) after confirming `is_initialized == 1` (`45`).
#[inline]
pub fn read_mint_decimals(data: &[u8]) -> Result<u8, ProgramError> {
    if data.len() < 82 || data[45] != 1 {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(data[44])
}

/// Reads a Mint's optional `mint_authority` (`0..36`).
pub fn read_mint_authority(data: &[u8]) -> Result<Option<[u8; 32]>, ProgramError> {
    if data.len() < 82 || data[45] != 1 {
        return Err(ProgramError::InvalidAccountData);
    }
    read_coption_pubkey(&data[0..36])
}

/// Reads a Mint's optional `freeze_authority` (`46..82`).
pub fn read_freeze_authority(data: &[u8]) -> Result<Option<[u8; 32]>, ProgramError> {
    if data.len() < 82 || data[45] != 1 {
        return Err(ProgramError::InvalidAccountData);
    }
    read_coption_pubkey(&data[46..82])
}

fn read_coption_pubkey(slice: &[u8]) -> Result<Option<[u8; 32]>, ProgramError> {
    let tag = u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
    match tag {
        0 => Ok(None),
        1 => {
            let mut key = [0u8; 32];
            key.copy_from_slice(&slice[4..36]);
            Ok(Some(key))
        }
        _ => Err(ProgramError::InvalidAccountData),
    }
}

/// Parsed fields from a Token-2022 Mint's `TransferFeeConfig` TLV extension (`ExtensionType = 1`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MintTransferFeeTlv {
    pub transfer_fee_config_authority: [u8; 32],
    pub withdraw_withheld_authority: [u8; 32],
    pub withheld_amount: u64,
}

/// Scans a Token-2022 Mint's TLV buffer (starting at byte `166`) for `TransferFeeConfig`.
pub fn find_transfer_fee_config_in_mint(data: &[u8]) -> Option<MintTransferFeeTlv> {
    if data.len() <= 166 || data[165] != 1 {
        return None;
    }
    let mut pos = 166usize;
    while pos + 4 <= data.len() {
        let ext_type = u16::from_le_bytes([data[pos], data[pos + 1]]);
        if ext_type == 0 {
            break;
        }
        let ext_len = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
        let val_start = pos + 4;
        let val_end = val_start.checked_add(ext_len)?;
        if val_end > data.len() {
            return None;
        }
        if ext_type == 1 && ext_len >= 72 {
            let mut cfg_auth = [0u8; 32];
            let mut wdr_auth = [0u8; 32];
            cfg_auth.copy_from_slice(&data[val_start..val_start + 32]);
            wdr_auth.copy_from_slice(&data[val_start + 32..val_start + 64]);
            let withheld_amount = u64::from_le_bytes(
                data[val_start + 64..val_start + 72].try_into().ok()?,
            );
            return Some(MintTransferFeeTlv {
                transfer_fee_config_authority: cfg_auth,
                withdraw_withheld_authority: wdr_auth,
                withheld_amount,
            });
        }
        pos = val_end;
    }
    None
}

/// Creates and assigns a PDA account, supporting both zero-lamport and pre-funded PDAs.
#[inline(never)]
pub fn create_pda_account<'a>(
    payer: &AccountInfo<'a>,
    pda: &AccountInfo<'a>,
    system_prog: &AccountInfo<'a>,
    owner_program_id: &Pubkey,
    required_lamports: u64,
    space: u64,
    signer_seeds: &[&[u8]],
) -> ProgramResult {
    let existing = pda.lamports();
    if existing == 0 {
        let ix = system_instruction::create_account(
            payer.key,
            pda.key,
            required_lamports,
            space,
            owner_program_id,
        );
        invoke_signed(
            &ix,
            &[payer.clone(), pda.clone(), system_prog.clone()],
            &[signer_seeds],
        )
    } else {
        if existing < required_lamports {
            let top_up = required_lamports - existing;
            let ix = system_instruction::transfer(payer.key, pda.key, top_up);
            invoke(&ix, &[payer.clone(), pda.clone(), system_prog.clone()])?;
        }
        let alloc_ix = system_instruction::allocate(pda.key, space);
        invoke_signed(
            &alloc_ix,
            &[pda.clone(), system_prog.clone()],
            &[signer_seeds],
        )?;
        let assign_ix = system_instruction::assign(pda.key, owner_program_id);
        invoke_signed(
            &assign_ix,
            &[pda.clone(), system_prog.clone()],
            &[signer_seeds],
        )
    }
}

/// Creates a PDA token account (if empty) and initializes it via `InitializeAccount3` (tag `18`).
#[inline(never)]
#[allow(clippy::too_many_arguments)]
pub fn create_and_init_token_account_pda<'a>(
    payer: &AccountInfo<'a>,
    vault_pda: &AccountInfo<'a>,
    mint: &AccountInfo<'a>,
    owner_authority: &Pubkey,
    token_program: &AccountInfo<'a>,
    system_prog: &AccountInfo<'a>,
    required_lamports: u64,
    space: u64,
    vault_seeds: &[&[u8]],
) -> ProgramResult {
    if vault_pda.owner != &system_program::ID {
        return Err(ProgramError::AccountAlreadyInitialized);
    }
    create_pda_account(
        payer,
        vault_pda,
        system_prog,
        token_program.key,
        required_lamports,
        space,
        vault_seeds,
    )?;

    let mut data = Vec::with_capacity(33);
    data.push(18);
    data.extend_from_slice(owner_authority.as_ref());
    let ix = Instruction {
        program_id: *token_program.key,
        accounts: vec![
            AccountMeta::new(*vault_pda.key, false),
            AccountMeta::new_readonly(*mint.key, false),
        ],
        data,
    };
    invoke(&ix, &[vault_pda.clone(), mint.clone(), token_program.clone()])
}

/// Minimal raw SPL Token / Token-2022 `Transfer` CPI (tag `3`, 3 accounts).
#[inline(never)]
pub fn spl_token_transfer<'a>(
    token_program: &AccountInfo<'a>,
    source: &AccountInfo<'a>,
    destination: &AccountInfo<'a>,
    authority: &AccountInfo<'a>,
    amount: u64,
    signer_seeds: Option<&[&[u8]]>,
) -> ProgramResult {
    let mut data = Vec::with_capacity(9);
    data.push(3);
    data.extend_from_slice(&amount.to_le_bytes());

    let ix = Instruction {
        program_id: *token_program.key,
        accounts: vec![
            AccountMeta::new(*source.key, false),
            AccountMeta::new(*destination.key, false),
            AccountMeta::new_readonly(*authority.key, true),
        ],
        data,
    };
    let infos = [
        source.clone(),
        destination.clone(),
        authority.clone(),
        token_program.clone(),
    ];
    match signer_seeds {
        Some(seeds) => invoke_signed(&ix, &infos, &[seeds]),
        None => invoke(&ix, &infos),
    }
}

/// Minimal raw Token-2022 `TransferChecked` CPI (tag `12`, 4 accounts).
#[inline(never)]
#[allow(clippy::too_many_arguments)]
pub fn token2022_transfer_checked<'a>(
    token_2022_program: &AccountInfo<'a>,
    source: &AccountInfo<'a>,
    mint: &AccountInfo<'a>,
    destination: &AccountInfo<'a>,
    authority: &AccountInfo<'a>,
    amount: u64,
    decimals: u8,
    signer_seeds: Option<&[&[u8]]>,
) -> ProgramResult {
    let mut data = Vec::with_capacity(10);
    data.push(12);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    let ix = Instruction {
        program_id: *token_2022_program.key,
        accounts: vec![
            AccountMeta::new(*source.key, false),
            AccountMeta::new_readonly(*mint.key, false),
            AccountMeta::new(*destination.key, false),
            AccountMeta::new_readonly(*authority.key, true),
        ],
        data,
    };
    let infos = [
        source.clone(),
        mint.clone(),
        destination.clone(),
        authority.clone(),
        token_2022_program.clone(),
    ];
    match signer_seeds {
        Some(seeds) => invoke_signed(&ix, &infos, &[seeds]),
        None => invoke(&ix, &infos),
    }
}

/// Revokes a Token-2022 authority (`SetAuthority` tag `6`, `new_authority = None`).
#[inline(never)]
pub fn token2022_revoke_authority<'a>(
    token_2022_program: &AccountInfo<'a>,
    mint_or_account: &AccountInfo<'a>,
    current_authority: &AccountInfo<'a>,
    authority_type: u8,
) -> ProgramResult {
    let ix = Instruction {
        program_id: *token_2022_program.key,
        accounts: vec![
            AccountMeta::new(*mint_or_account.key, false),
            AccountMeta::new_readonly(*current_authority.key, true),
        ],
        data: vec![6, authority_type, 0],
    };
    invoke(
        &ix,
        &[
            mint_or_account.clone(),
            current_authority.clone(),
            token_2022_program.clone(),
        ],
    )
}

/// Invokes Token-2022 `TransferFeeInstruction::HarvestWithheldTokensToMint` (`[26, 4]`).
#[inline(never)]
pub fn token2022_harvest_withheld_to_mint<'a>(
    token_2022_program: &AccountInfo<'a>,
    mint: &AccountInfo<'a>,
    sources: &[AccountInfo<'a>],
) -> ProgramResult {
    let mut metas = Vec::with_capacity(1 + sources.len());
    metas.push(AccountMeta::new(*mint.key, false));
    let mut infos = Vec::with_capacity(2 + sources.len());
    infos.push(mint.clone());
    for src in sources {
        metas.push(AccountMeta::new(*src.key, false));
        infos.push(src.clone());
    }
    infos.push(token_2022_program.clone());

    let ix = Instruction {
        program_id: *token_2022_program.key,
        accounts: metas,
        data: vec![26, 4],
    };
    invoke(&ix, &infos)
}

/// Invokes Token-2022 `TransferFeeInstruction::WithdrawWithheldTokensFromMint` (`[26, 2]`).
#[inline(never)]
pub fn token2022_withdraw_withheld_from_mint<'a>(
    token_2022_program: &AccountInfo<'a>,
    mint: &AccountInfo<'a>,
    destination: &AccountInfo<'a>,
    withdraw_withheld_authority: &AccountInfo<'a>,
    signer_seeds: Option<&[&[u8]]>,
) -> ProgramResult {
    let ix = Instruction {
        program_id: *token_2022_program.key,
        accounts: vec![
            AccountMeta::new(*mint.key, false),
            AccountMeta::new(*destination.key, false),
            AccountMeta::new_readonly(*withdraw_withheld_authority.key, true),
        ],
        data: vec![26, 2],
    };
    let infos = [
        mint.clone(),
        destination.clone(),
        withdraw_withheld_authority.clone(),
        token_2022_program.clone(),
    ];
    match signer_seeds {
        Some(seeds) => invoke_signed(&ix, &infos, &[seeds]),
        None => invoke(&ix, &infos),
    }
}

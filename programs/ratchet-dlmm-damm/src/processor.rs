use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};

use crate::cpi::{
    create_and_init_token_account_pda, create_pda_account, find_transfer_fee_config_in_mint,
    read_freeze_authority, read_mint_authority, read_mint_decimals, read_token_account_amount,
    read_token_account_mint, read_token_account_owner, read_token_account_state,
    spl_token_transfer, token2022_harvest_withheld_to_mint, token2022_revoke_authority,
    token2022_transfer_checked, token2022_withdraw_withheld_from_mint,
    AUTHORITY_TYPE_FREEZE_ACCOUNT, AUTHORITY_TYPE_MINT_TOKENS, AUTHORITY_TYPE_TRANSFER_FEE_CONFIG,
    SPL_TOKEN_ACCOUNT_LEN, SPL_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
    TOKEN_2022_TRANSFER_FEE_ACCOUNT_LEN, WSOL_MINT_ID,
};
use crate::error::RatchetError;
use crate::graduation::graduate_in_place;
use crate::instruction::{
    derive_base_vault_pda, derive_bin_array_pda, derive_pool_authority_pda, derive_pool_pda,
    derive_quote_vault_pda, BinSeedEntry, InitializeBinArrayArgs, InitializePoolArgs,
    SeedInitialBinsArgs, SwapArgs, BASE_VAULT_SEED, BIN_ARRAY_SEED, HARVEST_MODE_TO_MINT,
    HARVEST_MODE_WITHDRAW_FROM_MINT, IX_GRADUATE_IN_PLACE, IX_HARVEST_FALLBACK_TRANSFER_FEES,
    IX_INITIALIZE_BIN_ARRAY, IX_INITIALIZE_POOL, IX_REVOKE_AND_OPEN_SLOT0,
    IX_SEED_INITIAL_BINS, IX_SWAP, POOL_AUTHORITY_SEED, POOL_SEED, QUOTE_VAULT_SEED,
};
use crate::math::{execute_buy_swap, execute_sell_swap, sqrt_bin_step_q64};
use crate::state::{
    ActiveBinArrayPda, RatchetPoolPda, ACTIVE_BIN_ARRAY_PDA_DATA_BYTES, DEFAULT_COMPOUNDING_FEE_BPS,
    DEFAULT_F_0_BPS, DEFAULT_F_MAX_SELL_BPS, DEFAULT_F_MIN_BPS, HARD_SELL_IMPACT_CAP_BPS,
    RATCHET_POOL_PDA_DATA_BYTES,
};

/// Program entrypoint dispatcher.
///
/// Uses a fast-path check for `0x05 Swap` before decoding heavier setup instructions.
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let (&tag, rest) = instruction_data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;

    match tag {
        IX_SWAP => {
            let args = SwapArgs::unpack_fast(rest)?;
            process_swap(program_id, accounts, args)
        }
        IX_INITIALIZE_POOL => {
            let args = InitializePoolArgs::unpack(rest)?;
            process_initialize_pool(program_id, accounts, args)
        }
        IX_INITIALIZE_BIN_ARRAY => {
            let args = InitializeBinArrayArgs::unpack(rest)?;
            process_initialize_bin_array(program_id, accounts, args)
        }
        IX_SEED_INITIAL_BINS => {
            let args = SeedInitialBinsArgs::unpack(rest)?;
            process_seed_initial_bins(program_id, accounts, args)
        }
        IX_REVOKE_AND_OPEN_SLOT0 => process_revoke_and_open_slot0(program_id, accounts),
        IX_GRADUATE_IN_PLACE => process_graduate_in_place(program_id, accounts),
        IX_HARVEST_FALLBACK_TRANSFER_FEES => {
            let &mode = rest.first().ok_or(ProgramError::InvalidInstructionData)?;
            process_harvest_fallback_transfer_fees(program_id, accounts, mode)
        }
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

fn process_initialize_pool(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: InitializePoolArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let pool_pda = next_account_info(iter)?;
    let bin_array_pda = next_account_info(iter)?;
    let pool_authority_pda = next_account_info(iter)?;
    let base_mint = next_account_info(iter)?;
    let quote_mint = next_account_info(iter)?;
    let base_vault = next_account_info(iter)?;
    let quote_vault = next_account_info(iter)?;
    let creator_fee_vault = next_account_info(iter)?;
    let system_prog = next_account_info(iter)?;
    let token_2022_prog = next_account_info(iter)?;
    let spl_token_prog = next_account_info(iter)?;

    if !payer.is_signer || !pool_pda.is_writable || !base_vault.is_writable || !quote_vault.is_writable
    {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *system_prog.key != solana_program::system_program::ID
        || *token_2022_prog.key != TOKEN_2022_PROGRAM_ID
        || *spl_token_prog.key != SPL_TOKEN_PROGRAM_ID
    {
        return Err(ProgramError::IncorrectProgramId);
    }
    if *base_mint.owner != TOKEN_2022_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if *quote_mint.key != WSOL_MINT_ID || *quote_mint.owner != SPL_TOKEN_PROGRAM_ID {
        return Err(ProgramError::InvalidAccountData);
    }
    if !(DEFAULT_COMPOUNDING_FEE_BPS..=10_000).contains(&args.compounding_fee_bps) {
        return Err(RatchetError::InvalidFeeConfig.into());
    }
    if args.max_sell_impact_bps > HARD_SELL_IMPACT_CAP_BPS || args.transfer_fee_bps > DEFAULT_F_MAX_SELL_BPS {
        return Err(RatchetError::InvalidFeeConfig.into());
    }

    let (expected_pool, pool_bump) = derive_pool_pda(base_mint.key, program_id);
    if expected_pool != *pool_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }
    let (expected_bin_array, bin_array_bump) =
        derive_bin_array_pda(pool_pda.key, args.bin_array_start_id, program_id);
    if expected_bin_array != *bin_array_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }
    let (expected_authority, authority_bump) = derive_pool_authority_pda(pool_pda.key, program_id);
    if expected_authority != *pool_authority_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }

    let (has_transfer_fee_ext, _decimals) = {
        let mint_data = base_mint.try_borrow_data()?;
        let dec = read_mint_decimals(&mint_data)?;
        let has_tf = find_transfer_fee_config_in_mint(&mint_data).is_some();
        (has_tf, dec)
    };

    let rent = Rent::get()?;
    let pool_rent = rent.minimum_balance(RATCHET_POOL_PDA_DATA_BYTES);
    let pool_seeds: &[&[u8]] = &[POOL_SEED, base_mint.key.as_ref(), &[pool_bump]];
    create_pda_account(
        payer,
        pool_pda,
        system_prog,
        program_id,
        pool_rent,
        RATCHET_POOL_PDA_DATA_BYTES as u64,
        pool_seeds,
    )?;

    let mut vault_bump = 0u8;
    if base_vault.data_is_empty() {
        let (expected_base_vault, b_bump) = derive_base_vault_pda(pool_pda.key, program_id);
        if expected_base_vault != *base_vault.key {
            return Err(ProgramError::InvalidSeeds);
        }
        vault_bump = b_bump;
        let space = if has_transfer_fee_ext {
            TOKEN_2022_TRANSFER_FEE_ACCOUNT_LEN
        } else {
            SPL_TOKEN_ACCOUNT_LEN
        };
        let vault_rent = rent.minimum_balance(space);
        let seeds: &[&[u8]] = &[BASE_VAULT_SEED, pool_pda.key.as_ref(), &[b_bump]];
        create_and_init_token_account_pda(
            payer,
            base_vault,
            base_mint,
            pool_authority_pda.key,
            token_2022_prog,
            system_prog,
            vault_rent,
            space as u64,
            seeds,
        )?;
    } else {
        if *base_vault.owner != TOKEN_2022_PROGRAM_ID {
            return Err(ProgramError::IncorrectProgramId);
        }
        let data = base_vault.try_borrow_data()?;
        if read_token_account_mint(&data)? != base_mint.key.to_bytes()
            || read_token_account_owner(&data)? != pool_authority_pda.key.to_bytes()
            || read_token_account_state(&data)? != 1
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    if quote_vault.data_is_empty() {
        let (expected_quote_vault, q_bump) = derive_quote_vault_pda(pool_pda.key, program_id);
        if expected_quote_vault != *quote_vault.key {
            return Err(ProgramError::InvalidSeeds);
        }
        if vault_bump == 0 {
            vault_bump = q_bump;
        }
        let vault_rent = rent.minimum_balance(SPL_TOKEN_ACCOUNT_LEN);
        let seeds: &[&[u8]] = &[QUOTE_VAULT_SEED, pool_pda.key.as_ref(), &[q_bump]];
        create_and_init_token_account_pda(
            payer,
            quote_vault,
            quote_mint,
            pool_authority_pda.key,
            spl_token_prog,
            system_prog,
            vault_rent,
            SPL_TOKEN_ACCOUNT_LEN as u64,
            seeds,
        )?;
    } else {
        if *quote_vault.owner != SPL_TOKEN_PROGRAM_ID {
            return Err(ProgramError::IncorrectProgramId);
        }
        let data = quote_vault.try_borrow_data()?;
        if read_token_account_mint(&data)? != quote_mint.key.to_bytes()
            || read_token_account_owner(&data)? != pool_authority_pda.key.to_bytes()
            || read_token_account_state(&data)? != 1
        {
            return Err(ProgramError::InvalidAccountData);
        }
    }

    let now = Clock::get()?.unix_timestamp;
    let mut pool = RatchetPoolPda::new(
        base_mint.key.to_bytes(),
        quote_mint.key.to_bytes(),
        base_vault.key.to_bytes(),
        quote_vault.key.to_bytes(),
        payer.key.to_bytes(),
        bin_array_pda.key.to_bytes(),
        creator_fee_vault.key.to_bytes(),
        args.sqrt_p_start,
        args.sqrt_p_graduation,
        args.a_buy,
        args.bin_step_bps,
        args.active_bin_id,
        args.floor_bin_id,
        args.bin_array_start_id,
        pool_bump,
        vault_bump,
        bin_array_bump,
        now,
    )?;

    // Start vault ledger at zero prior to `SeedInitialBins` deposits.
    pool.real_vault_lamports = 0;
    pool.max_extractable_lamports = 0;
    pool.protocol_fee_lamports = 0;
    pool.compounding_fee_bps = args.compounding_fee_bps;
    pool.gamma_bps = args.gamma_bps;
    pool.max_sell_impact_bps = args.max_sell_impact_bps;
    pool.transfer_fee_bps = args.transfer_fee_bps;
    pool.transfer_fee_enabled = u8::from(has_transfer_fee_ext);
    pool.set_pool_authority_bump(authority_bump);
    pool.validate()?;

    let mut dst = pool_pda.try_borrow_mut_data()?;
    if dst.len() != RATCHET_POOL_PDA_DATA_BYTES {
        return Err(ProgramError::InvalidAccountData);
    }
    dst.copy_from_slice(pool.as_bytes());
    Ok(())
}

fn process_initialize_bin_array(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: InitializeBinArrayArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let payer = next_account_info(iter)?;
    let pool_pda = next_account_info(iter)?;
    let bin_array_pda = next_account_info(iter)?;
    let system_prog = next_account_info(iter)?;

    if !payer.is_signer || !bin_array_pda.is_writable {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *pool_pda.owner != *program_id || *system_prog.key != solana_program::system_program::ID {
        return Err(ProgramError::IncorrectProgramId);
    }

    let pool = {
        let data = pool_pda.try_borrow_data()?;
        RatchetPoolPda::from_bytes_unaligned(&data)?
    };
    if pool.authority_revoked != 0 || pool.authority != payer.key.to_bytes() {
        return Err(RatchetError::AuthorityRevoked.into());
    }
    if pool.active_bin_array != bin_array_pda.key.to_bytes()
        || pool.bin_array_start_id != args.start_bin_id
        || pool.bin_step_bps != args.bin_step_bps
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let (expected_bin_array, bin_array_bump) =
        derive_bin_array_pda(pool_pda.key, args.start_bin_id, program_id);
    if expected_bin_array != *bin_array_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }

    let bin_step_q64 = sqrt_bin_step_q64(args.bin_step_bps)?;
    let bin_array = ActiveBinArrayPda::new(
        pool_pda.key.to_bytes(),
        args.start_bin_id,
        args.active_bin_id,
        args.floor_bin_id,
        args.bin_step_bps,
        bin_step_q64,
        args.start_sqrt_price_q64,
        bin_array_bump,
    )?;

    let rent = Rent::get()?;
    let bin_rent = rent.minimum_balance(ACTIVE_BIN_ARRAY_PDA_DATA_BYTES);
    let start_bytes = args.start_bin_id.to_le_bytes();
    let seeds: &[&[u8]] = &[
        BIN_ARRAY_SEED,
        pool_pda.key.as_ref(),
        &start_bytes,
        &[bin_array_bump],
    ];
    create_pda_account(
        payer,
        bin_array_pda,
        system_prog,
        program_id,
        bin_rent,
        ACTIVE_BIN_ARRAY_PDA_DATA_BYTES as u64,
        seeds,
    )?;

    let mut dst = bin_array_pda.try_borrow_mut_data()?;
    if dst.len() != ACTIVE_BIN_ARRAY_PDA_DATA_BYTES {
        return Err(ProgramError::InvalidAccountData);
    }
    dst.copy_from_slice(bin_array.as_bytes());
    Ok(())
}

fn process_seed_initial_bins(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    args: SeedInitialBinsArgs,
) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let pool_pda = next_account_info(iter)?;
    let bin_array_pda = next_account_info(iter)?;
    let base_mint = next_account_info(iter)?;
    let base_vault = next_account_info(iter)?;
    let quote_vault = next_account_info(iter)?;
    let depositor_base = next_account_info(iter)?;
    let depositor_quote = next_account_info(iter)?;
    let token_2022_prog = next_account_info(iter)?;
    let spl_token_prog = next_account_info(iter)?;

    if !authority.is_signer
        || !pool_pda.is_writable
        || !bin_array_pda.is_writable
        || !base_vault.is_writable
        || !quote_vault.is_writable
        || !depositor_base.is_writable
        || !depositor_quote.is_writable
    {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *pool_pda.owner != *program_id
        || *bin_array_pda.owner != *program_id
        || *token_2022_prog.key != TOKEN_2022_PROGRAM_ID
        || *spl_token_prog.key != SPL_TOKEN_PROGRAM_ID
    {
        return Err(ProgramError::IncorrectProgramId);
    }

    let mut pool = {
        let data = pool_pda.try_borrow_data()?;
        RatchetPoolPda::from_bytes_unaligned(&data)?
    };
    let mut bin_array = {
        let data = bin_array_pda.try_borrow_data()?;
        ActiveBinArrayPda::from_bytes_unaligned(&data)?
    };

    if pool.authority_revoked != 0 || pool.authority != authority.key.to_bytes() {
        return Err(RatchetError::AuthorityRevoked.into());
    }
    if pool.active_bin_array != bin_array_pda.key.to_bytes()
        || bin_array.pool != pool_pda.key.to_bytes()
        || pool.base_mint != base_mint.key.to_bytes()
        || pool.base_vault != base_vault.key.to_bytes()
        || pool.quote_vault != quote_vault.key.to_bytes()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut batch_quote_sum: u64 = 0;
    let mut batch_base_sum: u64 = 0;
    for BinSeedEntry {
        bin_id,
        reserve_quote_lamports,
        reserve_base_atoms,
    } in &args.entries
    {
        let bin = bin_array.bin_mut(*bin_id)?;
        bin.reserve_quote_lamports = bin
            .reserve_quote_lamports
            .checked_add(*reserve_quote_lamports)
            .ok_or(RatchetError::MathOverflow)?;
        bin.reserve_base_atoms = bin
            .reserve_base_atoms
            .checked_add(*reserve_base_atoms)
            .ok_or(RatchetError::MathOverflow)?;
        batch_quote_sum = batch_quote_sum
            .checked_add(*reserve_quote_lamports)
            .ok_or(RatchetError::MathOverflow)?;
        batch_base_sum = batch_base_sum
            .checked_add(*reserve_base_atoms)
            .ok_or(RatchetError::MathOverflow)?;
    }

    if args.quote_deposit_lamports < batch_quote_sum || args.base_deposit_atoms < batch_base_sum {
        return Err(RatchetError::InvalidParameter.into());
    }

    let decimals = {
        let mint_data = base_mint.try_borrow_data()?;
        read_mint_decimals(&mint_data)?
    };

    if args.base_deposit_atoms > 0 {
        token2022_transfer_checked(
            token_2022_prog,
            depositor_base,
            base_mint,
            base_vault,
            authority,
            args.base_deposit_atoms,
            decimals,
            None,
        )?;
    }
    if args.quote_deposit_lamports > 0 {
        spl_token_transfer(
            spl_token_prog,
            depositor_quote,
            quote_vault,
            authority,
            args.quote_deposit_lamports,
            None,
        )?;
    }

    let live_quote_vault = {
        let data = quote_vault.try_borrow_data()?;
        read_token_account_amount(&data)?
    };
    let live_base_vault = {
        let data = base_vault.try_borrow_data()?;
        read_token_account_amount(&data)?
    };

    let surplus_cushion = args.quote_deposit_lamports - batch_quote_sum;
    pool.real_vault_lamports = pool
        .real_vault_lamports
        .checked_add(args.quote_deposit_lamports)
        .ok_or(RatchetError::MathOverflow)?;
    pool.max_extractable_lamports = pool
        .max_extractable_lamports
        .checked_add(batch_quote_sum)
        .ok_or(RatchetError::MathOverflow)?;
    pool.protocol_fee_lamports = pool
        .protocol_fee_lamports
        .checked_add(surplus_cushion)
        .ok_or(RatchetError::MathOverflow)?;

    if live_quote_vault < pool.real_vault_lamports
        || pool.max_extractable_lamports > pool.real_vault_lamports
    {
        return Err(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: live_quote_vault.min(pool.real_vault_lamports),
        }
        .into());
    }
    if live_base_vault < bin_array.total_base_atoms()? {
        return Err(RatchetError::InsufficientBinLiquidity.into());
    }

    pool.validate()?;

    {
        let mut dst = pool_pda.try_borrow_mut_data()?;
        dst.copy_from_slice(pool.as_bytes());
    }
    {
        let mut dst = bin_array_pda.try_borrow_mut_data()?;
        dst.copy_from_slice(bin_array.as_bytes());
    }
    Ok(())
}

fn process_revoke_and_open_slot0(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let authority = next_account_info(iter)?;
    let pool_pda = next_account_info(iter)?;
    let bin_array_pda = next_account_info(iter)?;
    let base_mint = next_account_info(iter)?;
    let quote_vault = next_account_info(iter)?;
    let token_2022_prog = next_account_info(iter)?;

    if !authority.is_signer || !pool_pda.is_writable || !base_mint.is_writable {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *pool_pda.owner != *program_id
        || *bin_array_pda.owner != *program_id
        || *base_mint.owner != TOKEN_2022_PROGRAM_ID
        || *token_2022_prog.key != TOKEN_2022_PROGRAM_ID
    {
        return Err(ProgramError::IncorrectProgramId);
    }

    let mut pool = {
        let data = pool_pda.try_borrow_data()?;
        RatchetPoolPda::from_bytes_unaligned(&data)?
    };
    let bin_array = {
        let data = bin_array_pda.try_borrow_data()?;
        ActiveBinArrayPda::from_bytes_unaligned(&data)?
    };

    if pool.authority_revoked != 0 {
        return Err(RatchetError::AuthorityRevoked.into());
    }
    if pool.authority != authority.key.to_bytes() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if pool.active_bin_array != bin_array_pda.key.to_bytes()
        || pool.base_mint != base_mint.key.to_bytes()
        || pool.quote_vault != quote_vault.key.to_bytes()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    if pool.f_max_sell_bps != DEFAULT_F_MAX_SELL_BPS
        || pool.compounding_fee_bps < DEFAULT_COMPOUNDING_FEE_BPS
        || pool.f_0_bps != DEFAULT_F_0_BPS
        || pool.f_min_bps != DEFAULT_F_MIN_BPS
        || pool.max_sell_impact_bps > HARD_SELL_IMPACT_CAP_BPS
    {
        return Err(RatchetError::InvalidFeeConfig.into());
    }

    let live_quote_vault = {
        let data = quote_vault.try_borrow_data()?;
        read_token_account_amount(&data)?
    };
    if live_quote_vault < pool.real_vault_lamports
        || pool.max_extractable_lamports > pool.real_vault_lamports
        || bin_array.total_quote_lamports()? > pool.real_vault_lamports
    {
        return Err(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: live_quote_vault.min(pool.real_vault_lamports),
        }
        .into());
    }

    let (mint_auth, freeze_auth, tf_cfg_auth) = {
        let mint_data = base_mint.try_borrow_data()?;
        let m_auth = read_mint_authority(&mint_data)?;
        let f_auth = read_freeze_authority(&mint_data)?;
        let tf_auth = find_transfer_fee_config_in_mint(&mint_data)
            .map(|tlv| tlv.transfer_fee_config_authority)
            .filter(|key| *key != [0u8; 32]);
        (m_auth, f_auth, tf_auth)
    };

    if let Some(m_auth) = mint_auth {
        if m_auth != authority.key.to_bytes() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        token2022_revoke_authority(token_2022_prog, base_mint, authority, AUTHORITY_TYPE_MINT_TOKENS)?;
    }
    if let Some(f_auth) = freeze_auth {
        if f_auth != authority.key.to_bytes() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        token2022_revoke_authority(
            token_2022_prog,
            base_mint,
            authority,
            AUTHORITY_TYPE_FREEZE_ACCOUNT,
        )?;
    }
    if let Some(tf_auth) = tf_cfg_auth {
        if tf_auth != authority.key.to_bytes() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        token2022_revoke_authority(
            token_2022_prog,
            base_mint,
            authority,
            AUTHORITY_TYPE_TRANSFER_FEE_CONFIG,
        )?;
    }

    pool.revoke_authority()?;
    pool.sqrt_p_high_water = pool.sqrt_p_current;
    pool.last_decay_timestamp = Clock::get()?.unix_timestamp;
    pool.validate()?;

    let mut dst = pool_pda.try_borrow_mut_data()?;
    dst.copy_from_slice(pool.as_bytes());
    Ok(())
}

#[inline(never)]
fn process_swap(program_id: &Pubkey, accounts: &[AccountInfo], args: SwapArgs) -> ProgramResult {
    // Support both the 28-account Jupiter/TS router layout and the compact 12-account layout.
    let (pool_pda, bin_array_pda, base_mint, quote_mint, base_vault, quote_vault, user_authority, user_base, user_quote, pool_authority_pda, token_2022_prog, spl_token_prog) =
        if accounts.len() >= 22 {
            (
                &accounts[0],
                &accounts[1],
                &accounts[2],
                &accounts[3],
                &accounts[4],
                &accounts[5],
                &accounts[6],
                &accounts[7],
                &accounts[8],
                &accounts[21],
                &accounts[12],
                &accounts[13],
            )
        } else if accounts.len() >= 12 {
            (
                &accounts[0],
                &accounts[1],
                &accounts[2],
                &accounts[3],
                &accounts[4],
                &accounts[5],
                &accounts[6],
                &accounts[7],
                &accounts[8],
                &accounts[9],
                &accounts[10],
                &accounts[11],
            )
        } else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

    if !user_authority.is_signer
        || !pool_pda.is_writable
        || !bin_array_pda.is_writable
        || !base_vault.is_writable
        || !quote_vault.is_writable
        || !user_base.is_writable
        || !user_quote.is_writable
    {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *pool_pda.owner != *program_id || *bin_array_pda.owner != *program_id {
        return Err(ProgramError::IncorrectProgramId);
    }
    if *token_2022_prog.key != TOKEN_2022_PROGRAM_ID || *spl_token_prog.key != SPL_TOKEN_PROGRAM_ID
    {
        return Err(ProgramError::IncorrectProgramId);
    }
    if user_base.key == base_vault.key || user_quote.key == quote_vault.key {
        return Err(ProgramError::InvalidAccountData);
    }

    let mut pool = {
        let data = pool_pda.try_borrow_data()?;
        RatchetPoolPda::from_bytes_unaligned(&data)?
    };
    let mut bin_array = {
        let data = bin_array_pda.try_borrow_data()?;
        ActiveBinArrayPda::from_bytes_unaligned(&data)?
    };

    if pool.active_bin_array != bin_array_pda.key.to_bytes()
        || bin_array.pool != pool_pda.key.to_bytes()
        || pool.base_mint != base_mint.key.to_bytes()
        || pool.quote_mint != quote_mint.key.to_bytes()
        || pool.base_vault != base_vault.key.to_bytes()
        || pool.quote_vault != quote_vault.key.to_bytes()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let authority_bump = match pool.pool_authority_bump() {
        Some(bump) => {
            let expected = Pubkey::create_program_address(
                &[POOL_AUTHORITY_SEED, pool_pda.key.as_ref(), &[bump]],
                program_id,
            )?;
            if expected != *pool_authority_pda.key {
                return Err(ProgramError::InvalidSeeds);
            }
            bump
        }
        None => {
            let (expected, bump) = derive_pool_authority_pda(pool_pda.key, program_id);
            if expected != *pool_authority_pda.key {
                return Err(ProgramError::InvalidSeeds);
            }
            bump
        }
    };

    let decimals = {
        let mint_data = base_mint.try_borrow_data()?;
        read_mint_decimals(&mint_data)?
    };
    let now = Clock::get()?.unix_timestamp;
    let authority_seeds: &[&[u8]] = &[POOL_AUTHORITY_SEED, pool_pda.key.as_ref(), &[authority_bump]];

    if args.is_buy == 1 {
        let quote = execute_buy_swap(
            &mut pool,
            &mut bin_array,
            args.amount_in,
            args.min_amount_out,
            now,
        )?;
        spl_token_transfer(
            spl_token_prog,
            user_quote,
            quote_vault,
            user_authority,
            quote.quote_in_lamports,
            None,
        )?;
        if quote.base_out_atoms > 0 {
            token2022_transfer_checked(
                token_2022_prog,
                base_vault,
                base_mint,
                user_base,
                pool_authority_pda,
                quote.base_out_atoms,
                decimals,
                Some(authority_seeds),
            )?;
        }
    } else {
        let quote = execute_sell_swap(
            &mut pool,
            &mut bin_array,
            args.amount_in,
            args.min_amount_out,
            now,
        )?;
        token2022_transfer_checked(
            token_2022_prog,
            user_base,
            base_mint,
            base_vault,
            user_authority,
            quote.base_in_atoms,
            decimals,
            None,
        )?;
        if quote.net_user_sol_out_lamports > 0 {
            spl_token_transfer(
                spl_token_prog,
                quote_vault,
                user_quote,
                pool_authority_pda,
                quote.net_user_sol_out_lamports,
                Some(authority_seeds),
            )?;
        }
    }

    {
        let mut dst = pool_pda.try_borrow_mut_data()?;
        dst.copy_from_slice(pool.as_bytes());
    }
    {
        let mut dst = bin_array_pda.try_borrow_mut_data()?;
        dst.copy_from_slice(bin_array.as_bytes());
    }
    Ok(())
}

fn process_graduate_in_place(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let iter = &mut accounts.iter();
    let pool_pda = next_account_info(iter)?;
    let bin_array_pda = next_account_info(iter)?;
    let quote_vault = next_account_info(iter)?;

    if !pool_pda.is_writable {
        return Err(ProgramError::InvalidAccountData);
    }
    if *pool_pda.owner != *program_id || *bin_array_pda.owner != *program_id {
        return Err(ProgramError::IncorrectProgramId);
    }

    let mut pool = {
        let data = pool_pda.try_borrow_data()?;
        RatchetPoolPda::from_bytes_unaligned(&data)?
    };
    let bin_array = {
        let data = bin_array_pda.try_borrow_data()?;
        ActiveBinArrayPda::from_bytes_unaligned(&data)?
    };

    if pool.active_bin_array != bin_array_pda.key.to_bytes()
        || bin_array.pool != pool_pda.key.to_bytes()
        || pool.quote_vault != quote_vault.key.to_bytes()
    {
        return Err(ProgramError::InvalidAccountData);
    }

    let live_quote_vault = {
        let data = quote_vault.try_borrow_data()?;
        read_token_account_amount(&data)?
    };
    if live_quote_vault < pool.real_vault_lamports {
        return Err(RatchetError::InsolventPool {
            max_extractable: pool.max_extractable_lamports,
            real_vault: live_quote_vault,
        }
        .into());
    }

    let slot = Clock::get()?.slot;
    let _receipt = graduate_in_place(&mut pool, &bin_array, slot)?;

    let mut dst = pool_pda.try_borrow_mut_data()?;
    dst.copy_from_slice(pool.as_bytes());
    Ok(())
}

fn process_harvest_fallback_transfer_fees(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    mode: u8,
) -> ProgramResult {
    match mode {
        HARVEST_MODE_TO_MINT => {
            // Write-lock-free harvest: does NOT touch `RatchetPoolPda`.
            let iter = &mut accounts.iter();
            let base_mint = next_account_info(iter)?;
            let token_2022_prog = next_account_info(iter)?;
            if *base_mint.owner != TOKEN_2022_PROGRAM_ID
                || *token_2022_prog.key != TOKEN_2022_PROGRAM_ID
            {
                return Err(ProgramError::IncorrectProgramId);
            }
            let sources = iter.as_slice();
            if sources.is_empty() {
                return Err(ProgramError::NotEnoughAccountKeys);
            }
            token2022_harvest_withheld_to_mint(token_2022_prog, base_mint, sources)
        }
        HARVEST_MODE_WITHDRAW_FROM_MINT => {
            let iter = &mut accounts.iter();
            let pool_pda = next_account_info(iter)?;
            let base_mint = next_account_info(iter)?;
            let destination = next_account_info(iter)?;
            let withdraw_auth = next_account_info(iter)?;
            let token_2022_prog = next_account_info(iter)?;

            if !pool_pda.is_writable || !base_mint.is_writable || !destination.is_writable {
                return Err(ProgramError::InvalidAccountData);
            }
            if *pool_pda.owner != *program_id
                || *base_mint.owner != TOKEN_2022_PROGRAM_ID
                || *token_2022_prog.key != TOKEN_2022_PROGRAM_ID
            {
                return Err(ProgramError::IncorrectProgramId);
            }

            let mut pool = {
                let data = pool_pda.try_borrow_data()?;
                RatchetPoolPda::from_bytes_unaligned(&data)?
            };
            if pool.base_mint != base_mint.key.to_bytes() {
                return Err(ProgramError::InvalidAccountData);
            }

            let withheld_before = {
                let mint_data = base_mint.try_borrow_data()?;
                find_transfer_fee_config_in_mint(&mint_data)
                    .map(|tlv| tlv.withheld_amount)
                    .ok_or(ProgramError::InvalidAccountData)?
            };

            if withdraw_auth.is_signer {
                token2022_withdraw_withheld_from_mint(
                    token_2022_prog,
                    base_mint,
                    destination,
                    withdraw_auth,
                    None,
                )?;
            } else {
                let (expected_auth, auth_bump) = derive_pool_authority_pda(pool_pda.key, program_id);
                if expected_auth != *withdraw_auth.key {
                    return Err(ProgramError::MissingRequiredSignature);
                }
                let seeds: &[&[u8]] =
                    &[POOL_AUTHORITY_SEED, pool_pda.key.as_ref(), &[auth_bump]];
                token2022_withdraw_withheld_from_mint(
                    token_2022_prog,
                    base_mint,
                    destination,
                    withdraw_auth,
                    Some(seeds),
                )?;
            }

            pool.harvested_transfer_fee_atoms = pool
                .harvested_transfer_fee_atoms
                .checked_add(withheld_before)
                .ok_or(RatchetError::MathOverflow)?;

            let mut dst = pool_pda.try_borrow_mut_data()?;
            dst.copy_from_slice(pool.as_bytes());
            Ok(())
        }
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

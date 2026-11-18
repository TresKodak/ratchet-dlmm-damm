//! End-to-end real Solana SVM (LiteSVM) integration tests for `ratchet_dlmm_damm.so`.
//!
//! Exercises the compiled sBPF ELF binary alongside LiteSVM's real SPL Token-2022,
//! SPL Token, and Associated Token Account BPF programs across all 8 lifecycle stages
//! and audit gates G01, G02, G04, G05, G06, G09, and G10.

use std::path::PathBuf;

use litesvm::LiteSVM;
use ratchet_dlmm_damm::{
    compounding::compute_sell_compounding_split,
    instruction::{
        derive_base_vault_pda, derive_bin_array_pda, derive_pool_authority_pda, derive_pool_pda,
        derive_quote_vault_pda, graduate_in_place_ix, harvest_withheld_to_mint_ix,
        initialize_bin_array_ix, initialize_pool_ix, revoke_and_open_slot0_ix,
        seed_initial_bins_ix, swap_ix, withdraw_withheld_from_mint_ix, BinSeedEntry,
        InitializeBinArrayArgs, InitializePoolArgs, SeedInitialBinsArgs, SwapArgs,
    },
    math::{compute_buy_fee_bps, pow_q64, q64_mul_round, sqrt_bin_step_q64, steps_above_high_water},
    state::{
        step2_rent_schedule, step5_rent_schedule, ActiveBinArrayPda, RatchetPoolPda,
        ACTIVE_BIN_ARRAY_LEN, ACTIVE_BIN_ARRAY_PDA_DATA_BYTES, DEFAULT_COMPOUNDING_FEE_BPS,
        DEFAULT_F_0_BPS, DEFAULT_F_MAX_SELL_BPS, DEFAULT_F_MIN_BPS, DEFAULT_GAMMA_BPS,
        DEFAULT_MAX_EXTRACTABLE_LAMPORTS, DEFAULT_MAX_SELL_IMPACT_BPS, DEFAULT_REAL_VAULT_LAMPORTS,
        GRADUATION_STATUS_ACTIVE_CURVE, GRADUATION_STATUS_GRADUATED_IN_PLACE, Q64_ONE,
        RATCHET_POOL_PDA_DATA_BYTES,
    },
};
use ratchet_litesvm_e2e::token_builders::{
    create_associated_token_account_idempotent, decode_mint_base, decode_token_metadata,
    decode_transfer_fee_amount_withheld, decode_transfer_fee_config, get_associated_token_address,
    initialize_account3, initialize_metadata_pointer, initialize_mint2, initialize_token_metadata,
    initialize_transfer_fee_config, mint_len_with_transfer_fee_and_metadata, mint_to,
    read_token_amount, sync_native, transfer_checked, ASSOCIATED_TOKEN_PROGRAM_ID,
    MINT_WITH_TRANSFER_FEE_AND_METADATA_POINTER_LEN, SPL_TOKEN_PROGRAM_ID,
    TOKEN_2022_ACCOUNT_WITH_TRANSFER_FEE_LEN, TOKEN_2022_PROGRAM_ID, WSOL_MINT,
};
use solana_sdk::{
    account::Account,
    clock::Clock,
    instruction::{AccountMeta, Instruction, InstructionError},
    message::Message,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction, system_program, sysvar,
    transaction::{Transaction, TransactionError},
};

const SWAP_CU_LIMIT_G02: u64 = 43_000;

fn program_id() -> Pubkey {
    ratchet_dlmm_damm::ID
}

fn program_so_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so")
}

fn setup_svm() -> (LiteSVM, Keypair) {
    let so_path = program_so_path();
    assert!(
        so_path.exists(),
        "Compiled program ELF missing at {} — run cargo-build-sbf first",
        so_path.display()
    );
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(program_id(), &so_path)
        .expect("load compiled ratchet_dlmm_damm.so into LiteSVM");

    // Preload canonical SPL-Token Native Mint (WSOL) as present on Solana clusters.
    let mut wsol_mint_data = vec![0u8; 82];
    wsol_mint_data[44] = 9; // decimals = 9
    wsol_mint_data[45] = 1; // is_initialized = true
    svm.set_account(
        WSOL_MINT,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(82),
            data: wsol_mint_data,
            owner: SPL_TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .expect("seed WSOL native mint");

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000_000)
        .expect("airdrop SOL to payer");
    (svm, payer)
}

fn send_tx(
    svm: &mut LiteSVM,
    instructions: &[Instruction],
    payer: &Keypair,
    signers: &[&Keypair],
) -> litesvm::types::TransactionMetadata {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new(instructions, Some(&payer.pubkey()));
    let tx = Transaction::new(signers, msg, blockhash);
    svm.send_transaction(tx)
        .unwrap_or_else(|e| panic!("transaction failed: {:?}\nlogs:\n{}", e.err, e.meta.logs.join("\n")))
}

fn send_tx_expect_err(
    svm: &mut LiteSVM,
    instructions: &[Instruction],
    payer: &Keypair,
    signers: &[&Keypair],
) -> litesvm::types::FailedTransactionMetadata {
    let blockhash = svm.latest_blockhash();
    let msg = Message::new(instructions, Some(&payer.pubkey()));
    let tx = Transaction::new(signers, msg, blockhash);
    let err = svm
        .send_transaction(tx)
        .expect_err("expected transaction to fail, but it succeeded");
    svm.expire_blockhash();
    err
}

#[allow(clippy::too_many_arguments)]
fn create_token2022_mint_with_extensions(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint: &Keypair,
    decimals: u8,
    transfer_fee_bps: u16,
    max_transfer_fee: u64,
    withdraw_withheld_authority: &Pubkey,
    name: &str,
    symbol: &str,
    uri: &str,
) {
    let final_len = mint_len_with_transfer_fee_and_metadata(name, symbol, uri);
    let rent_lamports = svm.minimum_balance_for_rent_exemption(final_len);

    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &mint.pubkey(),
            rent_lamports,
            MINT_WITH_TRANSFER_FEE_AND_METADATA_POINTER_LEN as u64,
            &TOKEN_2022_PROGRAM_ID,
        ),
        initialize_transfer_fee_config(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            Some(&payer.pubkey()),
            Some(withdraw_withheld_authority),
            transfer_fee_bps,
            max_transfer_fee,
        ),
        initialize_metadata_pointer(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            Some(&payer.pubkey()),
            Some(&mint.pubkey()),
        ),
        initialize_mint2(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            decimals,
            &payer.pubkey(),
            Some(&payer.pubkey()),
        ),
        initialize_token_metadata(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &payer.pubkey(),
            &mint.pubkey(),
            &payer.pubkey(),
            name,
            symbol,
            uri,
        ),
    ];

    send_tx(svm, &ixs, payer, &[payer, mint]);
}

fn create_token2022_account(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Pubkey {
    let account = Keypair::new();
    let rent = svm.minimum_balance_for_rent_exemption(TOKEN_2022_ACCOUNT_WITH_TRANSFER_FEE_LEN);
    let ixs = [
        system_instruction::create_account(
            &payer.pubkey(),
            &account.pubkey(),
            rent,
            TOKEN_2022_ACCOUNT_WITH_TRANSFER_FEE_LEN as u64,
            &TOKEN_2022_PROGRAM_ID,
        ),
        initialize_account3(&TOKEN_2022_PROGRAM_ID, &account.pubkey(), mint, owner),
    ];
    send_tx(svm, &ixs, payer, &[payer, &account]);
    account.pubkey()
}

fn wrap_sol_to_wsol_ata(svm: &mut LiteSVM, payer: &Keypair, owner: &Pubkey, lamports: u64) -> Pubkey {
    let wsol_ata = get_associated_token_address(owner, &WSOL_MINT, &SPL_TOKEN_PROGRAM_ID);
    let ixs = [
        create_associated_token_account_idempotent(
            &payer.pubkey(),
            owner,
            &WSOL_MINT,
            &SPL_TOKEN_PROGRAM_ID,
        ),
        system_instruction::transfer(&payer.pubkey(), &wsol_ata, lamports),
        sync_native(&SPL_TOKEN_PROGRAM_ID, &wsol_ata),
    ];
    send_tx(svm, &ixs, payer, &[payer]);
    wsol_ata
}

fn read_pool(svm: &LiteSVM, pool_pda: &Pubkey) -> RatchetPoolPda {
    let acct = svm.get_account(pool_pda).expect("pool account exists");
    assert_eq!(acct.owner, program_id());
    assert_eq!(acct.data.len(), RATCHET_POOL_PDA_DATA_BYTES);
    RatchetPoolPda::from_bytes_unaligned(&acct.data).expect("decode RatchetPoolPda")
}

fn read_bin_array(svm: &LiteSVM, bin_array_pda: &Pubkey) -> ActiveBinArrayPda {
    let acct = svm.get_account(bin_array_pda).expect("bin array account exists");
    assert_eq!(acct.owner, program_id());
    assert_eq!(acct.data.len(), ACTIVE_BIN_ARRAY_PDA_DATA_BYTES);
    ActiveBinArrayPda::from_bytes_unaligned(&acct.data).expect("decode ActiveBinArrayPda")
}

/// Builds the full 28-account `0x05 Swap` layout matching `sdk/ts-client/src/accounts.ts`
/// and `programs/ratchet-dlmm-damm/src/processor.rs` (`accounts.len() >= 22`).
#[allow(clippy::too_many_arguments)]
fn build_swap_28_ix(
    prog_id: &Pubkey,
    pool_pda: &Pubkey,
    bin_array_pda: &Pubkey,
    base_mint: &Pubkey,
    base_vault: &Pubkey,
    quote_vault: &Pubkey,
    user_authority: &Pubkey,
    user_base: &Pubkey,
    user_quote: &Pubkey,
    creator_fee_vault: &Pubkey,
    args: SwapArgs,
) -> Instruction {
    let (pool_authority_pda, _) = derive_pool_authority_pda(pool_pda, prog_id);
    let dummy_oracle = Pubkey::new_unique();
    let dummy_ext1 = Pubkey::new_unique();
    let dummy_ext2 = Pubkey::new_unique();
    let dummy_tf = Pubkey::new_unique();
    let dummy_comp = Pubkey::new_unique();
    let dummy_event = Pubkey::new_unique();
    let jup_id = Pubkey::new_unique();
    let memo_id = solana_sdk::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

    Instruction {
        program_id: *prog_id,
        accounts: vec![
            AccountMeta::new(*pool_pda, false),
            AccountMeta::new(*bin_array_pda, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new_readonly(WSOL_MINT, false),
            AccountMeta::new(*base_vault, false),
            AccountMeta::new(*quote_vault, false),
            AccountMeta::new_readonly(*user_authority, true),
            AccountMeta::new(*user_base, false),
            AccountMeta::new(*user_quote, false),
            AccountMeta::new(*creator_fee_vault, false),
            AccountMeta::new(dummy_tf, false),
            AccountMeta::new(dummy_comp, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(sysvar::rent::id(), false),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
            AccountMeta::new_readonly(sysvar::instructions::id(), false),
            AccountMeta::new_readonly(dummy_event, false),
            AccountMeta::new_readonly(*prog_id, false),
            AccountMeta::new_readonly(pool_authority_pda, false),
            AccountMeta::new(dummy_oracle, false),
            AccountMeta::new(dummy_ext1, false),
            AccountMeta::new(dummy_ext2, false),
            AccountMeta::new_readonly(jup_id, false),
            AccountMeta::new(*creator_fee_vault, false),
            AccountMeta::new_readonly(memo_id, false),
        ],
        data: args.pack().to_vec(),
    }
}

#[test]
fn test_litesvm_full_ratchet_lifecycle_and_gates() {
    let (mut svm, payer) = setup_svm();
    let prog_id = program_id();

    // Verify SIMD-0437 Step 2 & Step 5 rent schedule constants match spec.
    let step2 = step2_rent_schedule().unwrap();
    assert_eq!(step2.total_lamports, 8_940_800);
    let step5 = step5_rent_schedule().unwrap();
    assert_eq!(step5.total_lamports, 1_224_960);

    // -------------------------------------------------------------------------
    // 1. Create Token-2022 Mint (TransferFeeConfig + MetadataPointer + TokenMetadata)
    //    Mint supply + Wrap SOL into WSOL
    // -------------------------------------------------------------------------
    let mint = Keypair::new();
    let decimals = 6u8;
    let total_supply: u64 = 1_000_000_000_000_000; // 1B tokens @ 6 decimals
    let token_name = "Ratchet Protocol";
    let token_symbol = "RATCH";
    let token_uri = "https://example.invalid/ratchet.json";

    let (pool_pda, _) = derive_pool_pda(&mint.pubkey(), &prog_id);
    let (pool_authority_pda, _) = derive_pool_authority_pda(&pool_pda, &prog_id);
    let (base_vault, _) = derive_base_vault_pda(&pool_pda, &prog_id);
    let (quote_vault, _) = derive_quote_vault_pda(&pool_pda, &prog_id);

    create_token2022_mint_with_extensions(
        &mut svm,
        &payer,
        &mint,
        decimals,
        0, // Primary pool uses 0 bps fallback transfer fee so AMM quotes match 1:1
        0,
        &pool_authority_pda,
        token_name,
        token_symbol,
        token_uri,
    );

    let mint_acct = svm.get_account(&mint.pubkey()).unwrap();
    assert_eq!(mint_acct.owner, TOKEN_2022_PROGRAM_ID);
    let decoded_base = decode_mint_base(&mint_acct.data);
    assert_eq!(decoded_base.decimals, decimals);
    assert!(decoded_base.is_initialized);
    assert_eq!(decoded_base.mint_authority, Some(payer.pubkey()));
    assert_eq!(decoded_base.freeze_authority, Some(payer.pubkey()));

    let decoded_tf = decode_transfer_fee_config(&mint_acct.data).expect("TransferFeeConfig TLV");
    assert_eq!(decoded_tf.transfer_fee_config_authority, payer.pubkey());
    assert_eq!(decoded_tf.withdraw_withheld_authority, pool_authority_pda);

    let decoded_meta = decode_token_metadata(&mint_acct.data).expect("TokenMetadata TLV");
    assert_eq!(decoded_meta.name, token_name);
    assert_eq!(decoded_meta.symbol, token_symbol);
    assert_eq!(decoded_meta.uri, token_uri);

    // Mint total supply to payer's Token-2022 account and wrap 250 SOL to payer's WSOL ATA.
    let payer_base_ata = create_token2022_account(&mut svm, &payer, &mint.pubkey(), &payer.pubkey());
    send_tx(
        &mut svm,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &payer_base_ata,
            &payer.pubkey(),
            total_supply,
        )],
        &payer,
        &[&payer],
    );
    assert_eq!(
        read_token_amount(&svm.get_account(&payer_base_ata).unwrap().data),
        total_supply
    );

    let payer_wsol_ata = wrap_sol_to_wsol_ata(&mut svm, &payer, &payer.pubkey(), 250_000_000_000);
    assert_eq!(
        read_token_amount(&svm.get_account(&payer_wsol_ata).unwrap().data),
        250_000_000_000
    );

    // -------------------------------------------------------------------------
    // 2. Execute InitializePool (480 B) & InitializeBinArray (1,024 B)
    // -------------------------------------------------------------------------
    let bin_step_bps: u16 = 100; // 1% price step per bin so 700 bps Gate G09 triggers at >7 bins
    let bin_step_q64 = sqrt_bin_step_q64(bin_step_bps).unwrap();
    let start_bin_id: i32 = 0;
    let floor_bin_id: i32 = 0;
    let active_bin_id: i32 = 10;
    let bin_0_sqrt_p = Q64_ONE;
    let active_sqrt_p = q64_mul_round(bin_0_sqrt_p, pow_q64(bin_step_q64, 10).unwrap()).unwrap();
    // Graduation threshold = 4 bin steps above Slot-0 active price.
    let grad_sqrt_p = q64_mul_round(active_sqrt_p, pow_q64(bin_step_q64, 4).unwrap()).unwrap();
    let a_buy: u64 = 5_000_000; // Makes 5x dump-brake visibly responsive within 2500 bps cap

    let (bin_array_pda, _) = derive_bin_array_pda(&pool_pda, start_bin_id, &prog_id);
    let creator_fee_vault = Keypair::new().pubkey();

    let init_pool_ix = initialize_pool_ix(
        &prog_id,
        &payer.pubkey(),
        &mint.pubkey(),
        &creator_fee_vault,
        None,
        None,
        InitializePoolArgs {
            sqrt_p_start: active_sqrt_p,
            sqrt_p_graduation: grad_sqrt_p,
            a_buy,
            bin_step_bps,
            active_bin_id,
            floor_bin_id,
            bin_array_start_id: start_bin_id,
            compounding_fee_bps: DEFAULT_COMPOUNDING_FEE_BPS,
            gamma_bps: DEFAULT_GAMMA_BPS,
            max_sell_impact_bps: DEFAULT_MAX_SELL_IMPACT_BPS,
            transfer_fee_bps: 0,
            flags: 0,
        },
    );
    let init_bins_ix = initialize_bin_array_ix(
        &prog_id,
        &payer.pubkey(),
        &pool_pda,
        InitializeBinArrayArgs {
            start_bin_id,
            active_bin_id,
            floor_bin_id,
            bin_step_bps,
            start_sqrt_price_q64: bin_0_sqrt_p,
        },
    );
    let init_meta = send_tx(&mut svm, &[init_pool_ix, init_bins_ix], &payer, &[&payer]);
    println!(
        "[LiteSVM Setup] InitializePool + InitializeBinArray CU = {}",
        init_meta.compute_units_consumed
    );

    let pool = read_pool(&svm, &pool_pda);
    assert_eq!(pool.f_0_bps, DEFAULT_F_0_BPS);
    assert_eq!(pool.f_min_bps, DEFAULT_F_MIN_BPS);
    assert_eq!(pool.f_max_sell_bps, DEFAULT_F_MAX_SELL_BPS);
    assert_eq!(pool.compounding_fee_bps, DEFAULT_COMPOUNDING_FEE_BPS);
    assert_eq!(pool.a_sell, 5 * a_buy);
    assert_eq!(pool.authority_revoked, 0);
    assert_eq!(pool.graduation_status, GRADUATION_STATUS_ACTIVE_CURVE);

    let bin_array = read_bin_array(&svm, &bin_array_pda);
    assert_eq!(bin_array.active_bin_id, active_bin_id);
    assert_eq!(bin_array.floor_bin_id, floor_bin_id);
    assert_eq!(bin_array.bins[active_bin_id as usize].sqrt_price_q64, active_sqrt_p);

    // -------------------------------------------------------------------------
    // 3. Execute SeedInitialBins depositing real Token-2022 + WSOL + Gate G06 solvency
    // -------------------------------------------------------------------------
    // Negative solvency check: quote_deposit < bin quote total must fail cleanly.
    let bad_seed_ix = seed_initial_bins_ix(
        &prog_id,
        &payer.pubkey(),
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &payer_base_ata,
        &payer_wsol_ata,
        SeedInitialBinsArgs {
            quote_deposit_lamports: 1_000,
            base_deposit_atoms: 1_000_000,
            entries: vec![BinSeedEntry {
                bin_id: floor_bin_id,
                reserve_quote_lamports: 2_000,
                reserve_base_atoms: 0,
            }],
        },
    );
    send_tx_expect_err(&mut svm, &[bad_seed_ix], &payer, &[&payer]);

    // Canonical 86.5 SOL real vault vs 85.0 SOL max extractable across bins 0..=10 (+1.5 SOL cushion).
    let mut entries = Vec::new();
    let num_quote_bins = (active_bin_id - floor_bin_id + 1) as u64;
    let per_bin_quote = DEFAULT_MAX_EXTRACTABLE_LAMPORTS / num_quote_bins;
    let quote_rem = DEFAULT_MAX_EXTRACTABLE_LAMPORTS % num_quote_bins;
    let per_bin_base: u64 = 5_000_000_000; // 5,000 tokens per upper bin (~5.5 SOL net quote/bin)
    let mut total_base_seeded = 0u64;

    for bin_id in floor_bin_id..(ACTIVE_BIN_ARRAY_LEN as i32) {
        let q = if bin_id <= active_bin_id {
            per_bin_quote + if bin_id == floor_bin_id { quote_rem } else { 0 }
        } else {
            0
        };
        let b = if bin_id >= active_bin_id {
            total_base_seeded += per_bin_base;
            per_bin_base
        } else {
            0
        };
        entries.push(BinSeedEntry {
            bin_id,
            reserve_quote_lamports: q,
            reserve_base_atoms: b,
        });
    }

    let seed_ix = seed_initial_bins_ix(
        &prog_id,
        &payer.pubkey(),
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &payer_base_ata,
        &payer_wsol_ata,
        SeedInitialBinsArgs {
            quote_deposit_lamports: DEFAULT_REAL_VAULT_LAMPORTS,
            base_deposit_atoms: total_base_seeded,
            entries,
        },
    );
    let seed_meta = send_tx(&mut svm, &[seed_ix], &payer, &[&payer]);
    println!(
        "[LiteSVM Setup] SeedInitialBins CU = {}",
        seed_meta.compute_units_consumed
    );

    let pool_seeded = read_pool(&svm, &pool_pda);
    assert_eq!(pool_seeded.real_vault_lamports, DEFAULT_REAL_VAULT_LAMPORTS);
    assert_eq!(pool_seeded.max_extractable_lamports, DEFAULT_MAX_EXTRACTABLE_LAMPORTS);
    assert_eq!(
        read_token_amount(&svm.get_account(&quote_vault).unwrap().data),
        DEFAULT_REAL_VAULT_LAMPORTS
    );
    assert_eq!(
        read_token_amount(&svm.get_account(&base_vault).unwrap().data),
        total_base_seeded
    );

    // -------------------------------------------------------------------------
    // 4. Execute RevokeAndOpenSlot0
    // -------------------------------------------------------------------------
    let revoke_ix = revoke_and_open_slot0_ix(
        &prog_id,
        &payer.pubkey(),
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &quote_vault,
    );
    let revoke_meta = send_tx(&mut svm, &[revoke_ix], &payer, &[&payer]);
    println!(
        "[LiteSVM Setup] RevokeAndOpenSlot0 CU = {}",
        revoke_meta.compute_units_consumed
    );

    let pool_open = read_pool(&svm, &pool_pda);
    assert_eq!(pool_open.authority_revoked, 1);
    assert_eq!(pool_open.authority, [0u8; 32]);
    assert_eq!(pool_open.f_0_bps, 8000);
    assert_eq!(pool_open.f_max_sell_bps, 2500);
    assert!(pool_open.compounding_fee_bps >= 7500);

    let mint_after_revoke = svm.get_account(&mint.pubkey()).unwrap();
    let decoded_revoked_base = decode_mint_base(&mint_after_revoke.data);
    assert_eq!(decoded_revoked_base.mint_authority, None);
    assert_eq!(decoded_revoked_base.freeze_authority, None);
    let decoded_revoked_tf = decode_transfer_fee_config(&mint_after_revoke.data).unwrap();
    assert_eq!(decoded_revoked_tf.transfer_fee_config_authority, Pubkey::default());
    // Withdraw authority remains intact for out-of-band fee harvesting.
    assert_eq!(decoded_revoked_tf.withdraw_withheld_authority, pool_authority_pda);

    // Minting after authority revocation must fail.
    send_tx_expect_err(
        &mut svm,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &payer_base_ata,
            &payer.pubkey(),
            1,
        )],
        &payer,
        &[&payer],
    );

    // Re-calling RevokeAndOpenSlot0 must fail with RatchetError::AuthorityRevoked (6012).
    svm.expire_blockhash();
    let revoke_err = send_tx_expect_err(
        &mut svm,
        &[revoke_and_open_slot0_ix(
            &prog_id,
            &payer.pubkey(),
            &pool_pda,
            &bin_array_pda,
            &mint.pubkey(),
            &quote_vault,
        )],
        &payer,
        &[&payer],
    );
    assert_eq!(
        revoke_err.err,
        TransactionError::InstructionError(0, InstructionError::Custom(6012))
    );

    // -------------------------------------------------------------------------
    // 5. Execute real Buy Swap(s), verify v_a_up, sqrt_p_high_water, transfers, and Gate G02 CU
    // -------------------------------------------------------------------------
    let trader = Keypair::new();
    svm.airdrop(&trader.pubkey(), 200_000_000_000).unwrap();
    let trader_wsol_ata = wrap_sol_to_wsol_ata(&mut svm, &trader, &trader.pubkey(), 150_000_000_000);
    let trader_base_ata =
        create_token2022_account(&mut svm, &trader, &mint.pubkey(), &trader.pubkey());

    // Buy 1 (Slot-0 80% base fee): 35 SOL in -> ~7 SOL net quote, crosses >= 1 bin upward.
    let buy1_in: u64 = 35_000_000_000;
    let pool_before_b1 = read_pool(&svm, &pool_pda);
    let steps_b1 =
        steps_above_high_water(pool_before_b1.sqrt_p_high_water, pool_before_b1.sqrt_p_start, bin_step_q64)
            .unwrap();
    assert_eq!(steps_b1, 0);
    let fee_b1 = compute_buy_fee_bps(
        pool_before_b1.f_0_bps,
        pool_before_b1.f_min_bps,
        pool_before_b1.gamma_bps,
        steps_b1,
        pool_before_b1.a_buy,
        pool_before_b1.v_a_up,
        pool_before_b1.bin_step_bps,
    )
    .unwrap();
    assert_eq!(fee_b1.base_fee_bps, 8000);

    let buy1_ix = swap_ix(
        &prog_id,
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &trader.pubkey(),
        &trader_base_ata,
        &trader_wsol_ata,
        SwapArgs {
            is_buy: 1,
            amount_in: buy1_in,
            min_amount_out: 1,
        },
    );
    let buy1_meta = send_tx(&mut svm, &[buy1_ix], &trader, &[&trader]);
    let buy1_cu = buy1_meta.compute_units_consumed;
    println!("[LiteSVM Gate G02] Buy Swap 1 (12-acct) CU = {}", buy1_cu);
    assert!(
        buy1_cu <= SWAP_CU_LIMIT_G02,
        "Buy Swap 1 exceeded Gate G02 limit: {} > {}",
        buy1_cu,
        SWAP_CU_LIMIT_G02
    );

    let pool_after_b1 = read_pool(&svm, &pool_pda);
    let trader_base_after_b1 = read_token_amount(&svm.get_account(&trader_base_ata).unwrap().data);
    assert!(trader_base_after_b1 > 0);
    assert!(pool_after_b1.v_a_up >= 1, "expected upward crossings, got v_a_up={}", pool_after_b1.v_a_up);
    assert!(pool_after_b1.sqrt_p_high_water > pool_before_b1.sqrt_p_high_water);
    assert_eq!(pool_after_b1.sqrt_p_high_water, pool_after_b1.sqrt_p_current);
    assert_eq!(
        read_token_amount(&svm.get_account(&quote_vault).unwrap().data),
        pool_after_b1.real_vault_lamports
    );
    assert_eq!(
        pool_after_b1.real_vault_lamports,
        pool_before_b1.real_vault_lamports + buy1_in
    );

    // Buy 2 using the 28-account layout: verify high-water ratchet decayed base buy fee < 8000 bps and CU <= 43,000.
    let steps_b2 =
        steps_above_high_water(pool_after_b1.sqrt_p_high_water, pool_after_b1.sqrt_p_start, bin_step_q64)
            .unwrap();
    assert!(steps_b2 >= 1);
    let fee_b2 = compute_buy_fee_bps(
        pool_after_b1.f_0_bps,
        pool_after_b1.f_min_bps,
        pool_after_b1.gamma_bps,
        steps_b2,
        pool_after_b1.a_buy,
        pool_after_b1.v_a_up,
        pool_after_b1.bin_step_bps,
    )
    .unwrap();
    assert!(
        fee_b2.base_fee_bps < 8000,
        "High-water ratchet must decay base buy fee below 8000 bps: {}",
        fee_b2.base_fee_bps
    );

    let buy2_in: u64 = 25_000_000_000;
    let buy2_ix = build_swap_28_ix(
        &prog_id,
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &trader.pubkey(),
        &trader_base_ata,
        &trader_wsol_ata,
        &creator_fee_vault,
        SwapArgs {
            is_buy: 1,
            amount_in: buy2_in,
            min_amount_out: 1,
        },
    );
    let buy2_meta = send_tx(&mut svm, &[buy2_ix], &trader, &[&trader]);
    let buy2_cu = buy2_meta.compute_units_consumed;
    println!("[LiteSVM Gate G02] Buy Swap 2 (28-acct) CU = {}", buy2_cu);
    assert!(
        buy2_cu <= SWAP_CU_LIMIT_G02,
        "Buy Swap 2 (28-acct) exceeded Gate G02 limit: {} > {}",
        buy2_cu,
        SWAP_CU_LIMIT_G02
    );
    let pool_after_b2 = read_pool(&svm, &pool_pda);
    let trader_base_after_b2 = read_token_amount(&svm.get_account(&trader_base_ata).unwrap().data);
    assert!(trader_base_after_b2 > trader_base_after_b1);
    assert!(pool_after_b2.sqrt_p_high_water >= pool_after_b1.sqrt_p_high_water);

    // -------------------------------------------------------------------------
    // 6. Execute Sell Swap(s), verify Gate G09 (700 bps impact cap), 5x dump-brake,
    //    75% OnlyB quote auto-compounding into floor bin, and 0-lamport drift (Gate G01)
    // -------------------------------------------------------------------------
    // Mint a large token balance to a whale seller (pre-minted from payer's supply) to test Gate G09 rejection.
    let whale = Keypair::new();
    svm.airdrop(&whale.pubkey(), 10_000_000_000).unwrap();
    let whale_base_ata =
        create_token2022_account(&mut svm, &whale, &mint.pubkey(), &whale.pubkey());
    let whale_wsol_ata = wrap_sol_to_wsol_ata(&mut svm, &whale, &whale.pubkey(), 10_000_000);
    send_tx(
        &mut svm,
        &[transfer_checked(
            &TOKEN_2022_PROGRAM_ID,
            &payer_base_ata,
            &mint.pubkey(),
            &whale_base_ata,
            &payer.pubkey(),
            100_000_000_000,
            decimals,
        )],
        &payer,
        &[&payer],
    );

    // Oversized dump sweeping >7 bins downward must fail with RatchetError::SellImpactExceedsCap (6007).
    let oversized_sell_ix = swap_ix(
        &prog_id,
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &whale.pubkey(),
        &whale_base_ata,
        &whale_wsol_ata,
        SwapArgs {
            is_buy: 0,
            amount_in: 65_000_000_000,
            min_amount_out: 1,
        },
    );
    let g09_err = send_tx_expect_err(&mut svm, &[oversized_sell_ix], &whale, &[&whale]);
    assert_eq!(
        g09_err.err,
        TransactionError::InstructionError(0, InstructionError::Custom(6007)),
        "Expected Gate G09 SellImpactExceedsCap (6007), got {:?}",
        g09_err.err
    );

    // Valid multi-bin Sell Swap (crossing downward <= 700 bps impact):
    // Verify 5x dump-brake, 75% OnlyB floor bin quote auto-compounding, and 0-lamport conservation.
    let pool_before_s1 = read_pool(&svm, &pool_pda);
    let bins_before_s1 = read_bin_array(&svm, &bin_array_pda);
    let floor_quote_before = bins_before_s1.bins[floor_bin_id as usize].reserve_quote_lamports;
    let wsol_before_s1 = read_token_amount(&svm.get_account(&whale_wsol_ata).unwrap().data);
    let qvault_before_s1 = read_token_amount(&svm.get_account(&quote_vault).unwrap().data);

    let sell_in: u64 = 12_000_000_000;
    let expected_quote_s1 =
        ratchet_dlmm_damm::quote_sell(&pool_before_s1, &bins_before_s1, sell_in).unwrap();
    assert!(expected_quote_s1.downward_crossings >= 1);
    assert!(expected_quote_s1.price_impact_bps <= 700);
    assert!(
        expected_quote_s1.fee_quote.total_fee_bps > DEFAULT_F_MIN_BPS
            && expected_quote_s1.fee_quote.total_fee_bps <= DEFAULT_F_MAX_SELL_BPS,
        "Expected 5x asymmetric dump-brake f_sell in (25, 2500], got {}",
        expected_quote_s1.fee_quote.total_fee_bps
    );

    let split_check = compute_sell_compounding_split(
        expected_quote_s1.gross_sol_out_lamports,
        expected_quote_s1.fee_quote.total_fee_bps,
        pool_before_s1.compounding_fee_bps,
    )
    .unwrap();
    assert_eq!(
        split_check.compounded_sol_lamports as u128
            + split_check.protocol_fee_lamports as u128
            + split_check.net_user_sol_out_lamports as u128,
        split_check.gross_sol_out_lamports as u128
    );

    let sell1_ix = build_swap_28_ix(
        &prog_id,
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &whale.pubkey(),
        &whale_base_ata,
        &whale_wsol_ata,
        &creator_fee_vault,
        SwapArgs {
            is_buy: 0,
            amount_in: sell_in,
            min_amount_out: expected_quote_s1.net_user_sol_out_lamports,
        },
    );
    let sell1_meta = send_tx(&mut svm, &[sell1_ix], &whale, &[&whale]);
    let sell1_cu = sell1_meta.compute_units_consumed;
    println!("[LiteSVM Gate G02] Sell Swap 1 (28-acct) CU = {}", sell1_cu);
    assert!(
        sell1_cu <= SWAP_CU_LIMIT_G02,
        "Sell Swap 1 exceeded Gate G02 limit: {} > {}",
        sell1_cu,
        SWAP_CU_LIMIT_G02
    );

    let pool_after_s1 = read_pool(&svm, &pool_pda);
    let bins_after_s1 = read_bin_array(&svm, &bin_array_pda);
    let floor_quote_after = bins_after_s1.bins[floor_bin_id as usize].reserve_quote_lamports;
    let wsol_after_s1 = read_token_amount(&svm.get_account(&whale_wsol_ata).unwrap().data);
    let qvault_after_s1 = read_token_amount(&svm.get_account(&quote_vault).unwrap().data);

    // High-water mark never decreases on sells; v_a_down tracks downward crossings.
    assert_eq!(pool_after_s1.sqrt_p_high_water, pool_before_s1.sqrt_p_high_water);
    assert!(pool_after_s1.sqrt_p_current < pool_before_s1.sqrt_p_current);
    assert_eq!(
        pool_after_s1.v_a_down,
        pool_before_s1.v_a_down + expected_quote_s1.downward_crossings
    );

    // Gate G01 0-lamport conservation + OnlyB 75% floor bin auto-compounding.
    assert_eq!(
        floor_quote_after - floor_quote_before,
        expected_quote_s1.compounded_sol_lamports
    );
    assert_eq!(
        wsol_after_s1 - wsol_before_s1,
        expected_quote_s1.net_user_sol_out_lamports
    );
    assert_eq!(
        qvault_before_s1 - qvault_after_s1,
        expected_quote_s1.net_user_sol_out_lamports
    );
    assert_eq!(pool_after_s1.real_vault_lamports, qvault_after_s1);
    assert_eq!(
        pool_before_s1.real_vault_lamports as u128,
        pool_after_s1.real_vault_lamports as u128
            + expected_quote_s1.net_user_sol_out_lamports as u128
    );
    assert!(pool_after_s1.max_extractable_lamports <= pool_after_s1.real_vault_lamports);

    // -------------------------------------------------------------------------
    // 7. Push price >= sqrt_p_graduation -> GraduateInPlace -> Immediate same/next slot swaps (Gate G10)
    // -------------------------------------------------------------------------
    // Premature graduation must fail with GraduationThresholdNotReached (6011).
    let pre_grad_err = send_tx_expect_err(
        &mut svm,
        &[graduate_in_place_ix(
            &prog_id,
            &pool_pda,
            &bin_array_pda,
            &quote_vault,
        )],
        &payer,
        &[&payer],
    );
    assert_eq!(
        pre_grad_err.err,
        TransactionError::InstructionError(0, InstructionError::Custom(6011))
    );

    // Execute buy swap pushing sqrt_p_current >= sqrt_p_graduation.
    let grad_buyer_wsol = wrap_sol_to_wsol_ata(&mut svm, &payer, &payer.pubkey(), 100_000_000_000);
    let buy_to_grad_ix = swap_ix(
        &prog_id,
        &pool_pda,
        &bin_array_pda,
        &mint.pubkey(),
        &base_vault,
        &quote_vault,
        &payer.pubkey(),
        &payer_base_ata,
        &grad_buyer_wsol,
        SwapArgs {
            is_buy: 1,
            amount_in: 80_000_000_000,
            min_amount_out: 1,
        },
    );
    let buy_grad_meta = send_tx(&mut svm, &[buy_to_grad_ix], &payer, &[&payer]);
    println!(
        "[LiteSVM Gate G02] Buy-to-Graduation (12-acct) CU = {}",
        buy_grad_meta.compute_units_consumed
    );
    assert!(buy_grad_meta.compute_units_consumed <= SWAP_CU_LIMIT_G02);

    let pool_pre_grad = read_pool(&svm, &pool_pda);
    assert!(
        pool_pre_grad.sqrt_p_current >= pool_pre_grad.sqrt_p_graduation,
        "expected sqrt_p_current ({}) >= sqrt_p_graduation ({})",
        pool_pre_grad.sqrt_p_current,
        pool_pre_grad.sqrt_p_graduation
    );

    // Warp to slot 100 and execute GraduateInPlace, then swap immediately in slot 100 (0 dead-zone slots!).
    svm.warp_to_slot(100);
    svm.expire_blockhash();
    let grad_meta = send_tx(
        &mut svm,
        &[graduate_in_place_ix(
            &prog_id,
            &pool_pda,
            &bin_array_pda,
            &quote_vault,
        )],
        &payer,
        &[&payer],
    );
    println!(
        "[LiteSVM Gate G10] GraduateInPlace CU = {}",
        grad_meta.compute_units_consumed
    );

    let pool_graduated = read_pool(&svm, &pool_pda);
    assert_eq!(pool_graduated.graduation_status, GRADUATION_STATUS_GRADUATED_IN_PLACE);
    assert_eq!(pool_graduated.graduation_slot, 100);
    assert!(pool_graduated.max_extractable_lamports <= pool_graduated.real_vault_lamports);

    // Same-slot (slot 100) post-graduation Buy Swap -> 0 dead-zone slots!
    let post_grad_buy_meta = send_tx(
        &mut svm,
        &[swap_ix(
            &prog_id,
            &pool_pda,
            &bin_array_pda,
            &mint.pubkey(),
            &base_vault,
            &quote_vault,
            &trader.pubkey(),
            &trader_base_ata,
            &trader_wsol_ata,
            SwapArgs {
                is_buy: 1,
                amount_in: 1_000_000_000,
                min_amount_out: 1,
            },
        )],
        &trader,
        &[&trader],
    );
    println!(
        "[LiteSVM Gate G02] Post-Grad Same-Slot Buy CU = {}",
        post_grad_buy_meta.compute_units_consumed
    );
    assert!(post_grad_buy_meta.compute_units_consumed <= SWAP_CU_LIMIT_G02);
    assert_eq!(svm.get_sysvar::<Clock>().slot, 100);

    // Next-slot (slot 101) post-graduation Sell Swap -> continuous DAMM execution.
    svm.warp_to_slot(101);
    svm.expire_blockhash();
    let post_grad_sell_meta = send_tx(
        &mut svm,
        &[swap_ix(
            &prog_id,
            &pool_pda,
            &bin_array_pda,
            &mint.pubkey(),
            &base_vault,
            &quote_vault,
            &whale.pubkey(),
            &whale_base_ata,
            &whale_wsol_ata,
            SwapArgs {
                is_buy: 0,
                amount_in: 500_000_000,
                min_amount_out: 1,
            },
        )],
        &whale,
        &[&whale],
    );
    println!(
        "[LiteSVM Gate G02] Post-Grad Next-Slot Sell CU = {}",
        post_grad_sell_meta.compute_units_consumed
    );
    assert!(post_grad_sell_meta.compute_units_consumed <= SWAP_CU_LIMIT_G02);
}

#[test]
fn test_litesvm_harvest_fallback_transfer_fees_out_of_band() {
    let (mut svm, payer) = setup_svm();
    let prog_id = program_id();

    // -------------------------------------------------------------------------
    // 8. Dedicated Token-2022 Mint with active 100 bps (1%) TransferFeeConfig
    //    verifying write-lock-free HarvestWithheldTokensToMint (mode 0)
    //    and PDA-authorized WithdrawWithheldTokensFromMint (mode 1)
    // -------------------------------------------------------------------------
    let mint = Keypair::new();
    let decimals = 6u8;
    let tf_bps = 100u16; // 1.00% fallback fee
    let max_tf = 10_000_000_000u64;

    let (pool_pda, _) = derive_pool_pda(&mint.pubkey(), &prog_id);
    let (pool_authority_pda, _) = derive_pool_authority_pda(&pool_pda, &prog_id);
    let (base_vault, _) = derive_base_vault_pda(&pool_pda, &prog_id);
    let (quote_vault, _) = derive_quote_vault_pda(&pool_pda, &prog_id);

    create_token2022_mint_with_extensions(
        &mut svm,
        &payer,
        &mint,
        decimals,
        tf_bps,
        max_tf,
        &pool_authority_pda,
        "Ratchet Fallback",
        "RFALL",
        "https://example.invalid/fallback.json",
    );

    let payer_base_ata = create_token2022_account(&mut svm, &payer, &mint.pubkey(), &payer.pubkey());
    let recipient = Keypair::new();
    svm.airdrop(&recipient.pubkey(), 1_000_000_000).unwrap();
    let recipient_base_ata =
        create_token2022_account(&mut svm, &payer, &mint.pubkey(), &recipient.pubkey());
    let treasury = Keypair::new();
    let treasury_base_ata =
        create_token2022_account(&mut svm, &payer, &mint.pubkey(), &treasury.pubkey());

    send_tx(
        &mut svm,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &payer_base_ata,
            &payer.pubkey(),
            100_000_000_000,
        )],
        &payer,
        &[&payer],
    );

    let payer_wsol_ata = wrap_sol_to_wsol_ata(&mut svm, &payer, &payer.pubkey(), 20_000_000_000);
    let bin_step_bps = 100u16;
    let bin_step_q64 = sqrt_bin_step_q64(bin_step_bps).unwrap();
    let start_sqrt_p = Q64_ONE;
    let grad_sqrt_p = q64_mul_round(start_sqrt_p, pow_q64(bin_step_q64, 5).unwrap()).unwrap();
    let (bin_array_pda, _) = derive_bin_array_pda(&pool_pda, 0, &prog_id);

    send_tx(
        &mut svm,
        &[
            initialize_pool_ix(
                &prog_id,
                &payer.pubkey(),
                &mint.pubkey(),
                &treasury.pubkey(),
                None,
                None,
                InitializePoolArgs {
                    sqrt_p_start: start_sqrt_p,
                    sqrt_p_graduation: grad_sqrt_p,
                    a_buy: 10_000,
                    bin_step_bps,
                    active_bin_id: 0,
                    floor_bin_id: 0,
                    bin_array_start_id: 0,
                    compounding_fee_bps: DEFAULT_COMPOUNDING_FEE_BPS,
                    gamma_bps: DEFAULT_GAMMA_BPS,
                    max_sell_impact_bps: DEFAULT_MAX_SELL_IMPACT_BPS,
                    transfer_fee_bps: tf_bps,
                    flags: 0,
                },
            ),
            initialize_bin_array_ix(
                &prog_id,
                &payer.pubkey(),
                &pool_pda,
                InitializeBinArrayArgs {
                    start_bin_id: 0,
                    active_bin_id: 0,
                    floor_bin_id: 0,
                    bin_step_bps,
                    start_sqrt_price_q64: start_sqrt_p,
                },
            ),
        ],
        &payer,
        &[&payer],
    );

    // Deposit enough base tokens so post-1%-fee vault balance >= bin reserve (1_000_000_000 atoms).
    let bin_base_reserve = 1_000_000_000u64;
    let base_deposit_with_fee_cover = 1_020_000_000u64;
    send_tx(
        &mut svm,
        &[
            seed_initial_bins_ix(
                &prog_id,
                &payer.pubkey(),
                &pool_pda,
                &bin_array_pda,
                &mint.pubkey(),
                &base_vault,
                &quote_vault,
                &payer_base_ata,
                &payer_wsol_ata,
                SeedInitialBinsArgs {
                    quote_deposit_lamports: 10_000_000_000,
                    base_deposit_atoms: base_deposit_with_fee_cover,
                    entries: vec![BinSeedEntry {
                        bin_id: 0,
                        reserve_quote_lamports: 9_000_000_000,
                        reserve_base_atoms: bin_base_reserve,
                    }],
                },
            ),
            revoke_and_open_slot0_ix(
                &prog_id,
                &payer.pubkey(),
                &pool_pda,
                &bin_array_pda,
                &mint.pubkey(),
                &quote_vault,
            ),
        ],
        &payer,
        &[&payer],
    );

    // External Token-2022 transfer accrues 1% withheld fee on recipient's Token-2022 account.
    let ext_transfer_amount = 5_000_000_000u64; // 1% = 50_000_000 atoms withheld
    send_tx(
        &mut svm,
        &[transfer_checked(
            &TOKEN_2022_PROGRAM_ID,
            &payer_base_ata,
            &mint.pubkey(),
            &recipient_base_ata,
            &payer.pubkey(),
            ext_transfer_amount,
            decimals,
        )],
        &payer,
        &[&payer],
    );

    let recipient_withheld =
        decode_transfer_fee_amount_withheld(&svm.get_account(&recipient_base_ata).unwrap().data)
            .expect("TransferFeeAmount extension on recipient ATA");
    assert_eq!(recipient_withheld, 50_000_000);
    let vault_withheld =
        decode_transfer_fee_amount_withheld(&svm.get_account(&base_vault).unwrap().data)
            .expect("TransferFeeAmount extension on base_vault");
    assert_eq!(vault_withheld, 10_200_000); // 1% of 1_020_000_000

    // Mode 0: Write-lock-free HarvestWithheldTokensToMint (RatchetPoolPda is NOT in accounts!).
    let harvest_ix =
        harvest_withheld_to_mint_ix(&prog_id, &mint.pubkey(), &[recipient_base_ata, base_vault]);
    assert!(!harvest_ix.accounts.iter().any(|m| m.pubkey == pool_pda));
    let harvest_meta = send_tx(&mut svm, &[harvest_ix], &payer, &[&payer]);
    println!(
        "[LiteSVM Fallback] HarvestWithheldToMint (mode 0) CU = {}",
        harvest_meta.compute_units_consumed
    );

    assert_eq!(
        decode_transfer_fee_amount_withheld(&svm.get_account(&recipient_base_ata).unwrap().data)
            .unwrap(),
        0
    );
    let mint_tf_after_harvest =
        decode_transfer_fee_config(&svm.get_account(&mint.pubkey()).unwrap().data).unwrap();
    assert_eq!(mint_tf_after_harvest.withheld_amount, 60_200_000);

    // Mode 1: WithdrawWithheldTokensFromMint signed by PoolAuthorityPda (`authority_is_signer = false`).
    let withdraw_meta = send_tx(
        &mut svm,
        &[withdraw_withheld_from_mint_ix(
            &prog_id,
            &pool_pda,
            &mint.pubkey(),
            &treasury_base_ata,
            &pool_authority_pda,
            false,
        )],
        &payer,
        &[&payer],
    );
    println!(
        "[LiteSVM Fallback] WithdrawWithheldFromMint (mode 1) CU = {}",
        withdraw_meta.compute_units_consumed
    );

    let mint_tf_after_withdraw =
        decode_transfer_fee_config(&svm.get_account(&mint.pubkey()).unwrap().data).unwrap();
    assert_eq!(mint_tf_after_withdraw.withheld_amount, 0);
    assert_eq!(
        read_token_amount(&svm.get_account(&treasury_base_ata).unwrap().data),
        60_200_000
    );
    let pool_after_harvest = read_pool(&svm, &pool_pda);
    assert_eq!(pool_after_harvest.harvested_transfer_fee_atoms, 60_200_000);
}

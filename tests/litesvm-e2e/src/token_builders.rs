//! Raw SPL Token, SPL Token-2022, TokenMetadata, and Associated Token Account instruction builders
//! plus TLV state decoders for Token-2022 mint and token account extensions.

use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey,
    pubkey::Pubkey,
    system_program,
};

pub const SPL_TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_PROGRAM_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");

/// Base SPL Token / Token-2022 account length (`165` bytes).
pub const TOKEN_ACCOUNT_BASE_LEN: usize = 165;
/// Base Token-2022 Mint length with `TransferFeeConfig` (108B+4B) + `MetadataPointer` (64B+4B) + AccountType (1B).
pub const MINT_WITH_TRANSFER_FEE_AND_METADATA_POINTER_LEN: usize = 346;
/// Token-2022 token account length when mint carries `TransferFeeConfig` (`165 + 1 + 4 + 8 = 178` bytes).
pub const TOKEN_2022_ACCOUNT_WITH_TRANSFER_FEE_LEN: usize = 178;

/// Token-2022 `AuthorityType` enum values for `SetAuthority`.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AuthorityType {
    MintTokens = 0,
    FreezeAccount = 1,
    AccountOwner = 2,
    CloseAccount = 3,
    TransferFeeConfig = 4,
    WithheldWithdraw = 5,
    MetadataPointer = 12,
}

/// Computes the exact TLV byte overhead (`4 + Borsh(TokenMetadata)`) added by `TokenMetadata::Initialize`.
pub fn token_metadata_tlv_len(name: &str, symbol: &str, uri: &str) -> usize {
    // 4 (TLV header: u16 type + u16 len)
    // + 32 (update_authority) + 32 (mint)
    // + 4 + name.len() + 4 + symbol.len() + 4 + uri.len()
    // + 4 (additional_metadata Vec len = 0)
    4 + 32 + 32 + (4 + name.len()) + (4 + symbol.len()) + (4 + uri.len()) + 4
}

/// Computes the total post-realloc Token-2022 Mint length for `TransferFeeConfig + MetadataPointer + TokenMetadata`.
pub fn mint_len_with_transfer_fee_and_metadata(name: &str, symbol: &str, uri: &str) -> usize {
    let total =
        MINT_WITH_TRANSFER_FEE_AND_METADATA_POINTER_LEN + token_metadata_tlv_len(name, symbol, uri);
    // Token-2022 avoids colliding with Multisig::LEN (355 bytes) by padding 2 bytes if needed.
    if total == 355 {
        total + 2
    } else {
        total
    }
}

/// Derives the canonical Associated Token Account address for `(owner, mint, token_program_id)`.
pub fn get_associated_token_address(
    owner: &Pubkey,
    mint: &Pubkey,
    token_program_id: &Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program_id.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

fn push_coption_pubkey(buf: &mut Vec<u8>, key: Option<&Pubkey>) {
    match key {
        Some(pk) => {
            buf.push(1);
            buf.extend_from_slice(pk.as_ref());
        }
        None => buf.push(0),
    }
}

fn push_borsh_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// Token-2022 `TransferFeeExtension::InitializeTransferFeeConfig` (`[26, 0]`).
pub fn initialize_transfer_fee_config(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    transfer_fee_config_authority: Option<&Pubkey>,
    withdraw_withheld_authority: Option<&Pubkey>,
    transfer_fee_basis_points: u16,
    maximum_fee: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(78);
    data.push(26);
    data.push(0);
    push_coption_pubkey(&mut data, transfer_fee_config_authority);
    push_coption_pubkey(&mut data, withdraw_withheld_authority);
    data.extend_from_slice(&transfer_fee_basis_points.to_le_bytes());
    data.extend_from_slice(&maximum_fee.to_le_bytes());

    Instruction {
        program_id: *token_program_id,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// Token-2022 `MetadataPointerInstruction::Initialize` (`[39, 0]`).
pub fn initialize_metadata_pointer(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    authority: Option<&Pubkey>,
    metadata_address: Option<&Pubkey>,
) -> Instruction {
    let mut data = Vec::with_capacity(66);
    data.push(39);
    data.push(0);
    data.extend_from_slice(authority.map(|p| p.to_bytes()).unwrap_or([0u8; 32]).as_ref());
    data.extend_from_slice(
        metadata_address
            .map(|p| p.to_bytes())
            .unwrap_or([0u8; 32])
            .as_ref(),
    );

    Instruction {
        program_id: *token_program_id,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// SPL Token / Token-2022 `InitializeMint2` (`[20]`).
pub fn initialize_mint2(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    decimals: u8,
    mint_authority: &Pubkey,
    freeze_authority: Option<&Pubkey>,
) -> Instruction {
    let mut data = Vec::with_capacity(67);
    data.push(20);
    data.push(decimals);
    data.extend_from_slice(mint_authority.as_ref());
    push_coption_pubkey(&mut data, freeze_authority);

    Instruction {
        program_id: *token_program_id,
        accounts: vec![AccountMeta::new(*mint, false)],
        data,
    }
}

/// Token-2022 `TokenMetadataInstruction::Initialize` (`spl_token_metadata_interface:initialize_account`).
#[allow(clippy::too_many_arguments)]
pub fn initialize_token_metadata(
    token_program_id: &Pubkey,
    metadata: &Pubkey,
    update_authority: &Pubkey,
    mint: &Pubkey,
    mint_authority: &Pubkey,
    name: &str,
    symbol: &str,
    uri: &str,
) -> Instruction {
    let mut data = Vec::with_capacity(20 + name.len() + symbol.len() + uri.len());
    // sha256("spl_token_metadata_interface:initialize_account")[..8]
    data.extend_from_slice(&[210, 225, 30, 162, 88, 184, 77, 141]);
    push_borsh_string(&mut data, name);
    push_borsh_string(&mut data, symbol);
    push_borsh_string(&mut data, uri);

    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*metadata, false),
            AccountMeta::new_readonly(*update_authority, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(*mint_authority, true),
        ],
        data,
    }
}

/// SPL Token / Token-2022 `InitializeAccount3` (`[18]`).
pub fn initialize_account3(
    token_program_id: &Pubkey,
    account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
) -> Instruction {
    let mut data = Vec::with_capacity(33);
    data.push(18);
    data.extend_from_slice(owner.as_ref());

    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*account, false),
            AccountMeta::new_readonly(*mint, false),
        ],
        data,
    }
}

/// Associated Token Account `CreateIdempotent` (`[1]`).
pub fn create_associated_token_account_idempotent(
    payer: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program_id: &Pubkey,
) -> Instruction {
    let ata = get_associated_token_address(owner, mint, token_program_id);
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new_readonly(*token_program_id, false),
        ],
        data: vec![1],
    }
}

/// SPL Token / Token-2022 `MintTo` (`[7]`).
pub fn mint_to(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(7);
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data,
    }
}

/// SPL Token / Token-2022 `TransferChecked` (`[12]`).
pub fn transfer_checked(
    token_program_id: &Pubkey,
    source: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    owner: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = Vec::with_capacity(10);
    data.push(12);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);

    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*source, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*owner, true),
        ],
        data,
    }
}

/// SPL Token `SyncNative` (`[17]`).
pub fn sync_native(token_program_id: &Pubkey, account: &Pubkey) -> Instruction {
    Instruction {
        program_id: *token_program_id,
        accounts: vec![AccountMeta::new(*account, false)],
        data: vec![17],
    }
}

/// SPL Token / Token-2022 `SetAuthority` (`[6]`).
pub fn set_authority(
    token_program_id: &Pubkey,
    account_or_mint: &Pubkey,
    current_authority: &Pubkey,
    authority_type: AuthorityType,
    new_authority: Option<&Pubkey>,
) -> Instruction {
    let mut data = Vec::with_capacity(35);
    data.push(6);
    data.push(authority_type as u8);
    push_coption_pubkey(&mut data, new_authority);

    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*account_or_mint, false),
            AccountMeta::new_readonly(*current_authority, true),
        ],
        data,
    }
}

/// Token-2022 `TransferFeeExtension::HarvestWithheldTokensToMint` (`[26, 4]`).
pub fn harvest_withheld_tokens_to_mint(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    sources: &[Pubkey],
) -> Instruction {
    let mut accounts = Vec::with_capacity(1 + sources.len());
    accounts.push(AccountMeta::new(*mint, false));
    for src in sources {
        accounts.push(AccountMeta::new(*src, false));
    }
    Instruction {
        program_id: *token_program_id,
        accounts,
        data: vec![26, 4],
    }
}

/// Token-2022 `TransferFeeExtension::WithdrawWithheldTokensFromMint` (`[26, 2]`).
pub fn withdraw_withheld_tokens_from_mint(
    token_program_id: &Pubkey,
    mint: &Pubkey,
    destination: &Pubkey,
    authority: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *token_program_id,
        accounts: vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*destination, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data: vec![26, 2],
    }
}

/// Reads the SPL Token / Token-2022 token account `amount` at bytes `64..72`.
pub fn read_token_amount(data: &[u8]) -> u64 {
    assert!(data.len() >= 72, "token account data too short: {}", data.len());
    u64::from_le_bytes(data[64..72].try_into().unwrap())
}

/// Decoded base Mint fields (`0..82`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedMintBase {
    pub mint_authority: Option<Pubkey>,
    pub supply: u64,
    pub decimals: u8,
    pub is_initialized: bool,
    pub freeze_authority: Option<Pubkey>,
}

/// Decodes the base 82-byte Mint header.
pub fn decode_mint_base(data: &[u8]) -> DecodedMintBase {
    assert!(data.len() >= 82, "mint data too short: {}", data.len());
    let mint_auth_opt = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let mint_authority = if mint_auth_opt == 1 {
        Some(Pubkey::new_from_array(data[4..36].try_into().unwrap()))
    } else {
        None
    };
    let supply = u64::from_le_bytes(data[36..44].try_into().unwrap());
    let decimals = data[44];
    let is_initialized = data[45] != 0;
    let freeze_auth_opt = u32::from_le_bytes(data[46..50].try_into().unwrap());
    let freeze_authority = if freeze_auth_opt == 1 {
        Some(Pubkey::new_from_array(data[50..82].try_into().unwrap()))
    } else {
        None
    };
    DecodedMintBase {
        mint_authority,
        supply,
        decimals,
        is_initialized,
        freeze_authority,
    }
}

/// Finds a Token-2022 TLV extension slice inside an extended Mint or Token Account (`offset 166..`).
pub fn find_tlv_extension(data: &[u8], expected_type: u16) -> Option<&[u8]> {
    if data.len() <= 166 {
        return None;
    }
    let mut cursor = 166;
    while cursor + 4 <= data.len() {
        let ext_type = u16::from_le_bytes([data[cursor], data[cursor + 1]]);
        let ext_len = u16::from_le_bytes([data[cursor + 2], data[cursor + 3]]) as usize;
        if ext_type == 0 {
            break;
        }
        let start = cursor + 4;
        let end = start.checked_add(ext_len)?;
        if end > data.len() {
            return None;
        }
        if ext_type == expected_type {
            return Some(&data[start..end]);
        }
        cursor = end;
    }
    None
}

/// Decoded `TransferFeeConfig` TLV extension (`ExtensionType = 1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTransferFeeConfig {
    pub transfer_fee_config_authority: Pubkey,
    pub withdraw_withheld_authority: Pubkey,
    pub withheld_amount: u64,
    pub transfer_fee_bps: u16,
    pub maximum_fee: u64,
}

pub fn decode_transfer_fee_config(data: &[u8]) -> Option<DecodedTransferFeeConfig> {
    let tlv = find_tlv_extension(data, 1)?;
    if tlv.len() < 108 {
        return None;
    }
    let transfer_fee_config_authority = Pubkey::new_from_array(tlv[0..32].try_into().ok()?);
    let withdraw_withheld_authority = Pubkey::new_from_array(tlv[32..64].try_into().ok()?);
    let withheld_amount = u64::from_le_bytes(tlv[64..72].try_into().ok()?);
    let maximum_fee = u64::from_le_bytes(tlv[98..106].try_into().ok()?);
    let transfer_fee_bps = u16::from_le_bytes(tlv[106..108].try_into().ok()?);
    Some(DecodedTransferFeeConfig {
        transfer_fee_config_authority,
        withdraw_withheld_authority,
        withheld_amount,
        transfer_fee_bps,
        maximum_fee,
    })
}

/// Decodes `TransferFeeAmount.withheld_amount` (`ExtensionType = 2`) from a Token-2022 account.
pub fn decode_transfer_fee_amount_withheld(data: &[u8]) -> Option<u64> {
    let tlv = find_tlv_extension(data, 2)?;
    if tlv.len() < 8 {
        return None;
    }
    Some(u64::from_le_bytes(tlv[0..8].try_into().ok()?))
}

/// Decoded `TokenMetadata` TLV extension (`ExtensionType = 19`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedTokenMetadata {
    pub update_authority: Pubkey,
    pub mint: Pubkey,
    pub name: String,
    pub symbol: String,
    pub uri: String,
}

pub fn decode_token_metadata(data: &[u8]) -> Option<DecodedTokenMetadata> {
    let tlv = find_tlv_extension(data, 19)?;
    if tlv.len() < 64 {
        return None;
    }
    let update_authority = Pubkey::new_from_array(tlv[0..32].try_into().ok()?);
    let mint = Pubkey::new_from_array(tlv[32..64].try_into().ok()?);
    let mut off = 64;
    let read_str = |buf: &[u8], pos: &mut usize| -> Option<String> {
        if *pos + 4 > buf.len() {
            return None;
        }
        let len = u32::from_le_bytes(buf[*pos..*pos + 4].try_into().ok()?) as usize;
        *pos += 4;
        if *pos + len > buf.len() {
            return None;
        }
        let s = std::str::from_utf8(&buf[*pos..*pos + len]).ok()?.to_string();
        *pos += len;
        Some(s)
    };
    let name = read_str(tlv, &mut off)?;
    let symbol = read_str(tlv, &mut off)?;
    let uri = read_str(tlv, &mut off)?;
    Some(DecodedTokenMetadata {
        update_authority,
        mint,
        name,
        symbol,
        uri,
    })
}

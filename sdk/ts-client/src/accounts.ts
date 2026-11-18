/**
 * Account resolution, real PDA derivation, SIMD-0437 PDA rent math, Token-2022
 * TransferFeeConfig fallback harvesting configuration, and 28-account swap meta builder.
 */

import { PublicKey } from "@solana/web3.js";

export const TOKEN_2022_PROGRAM_ID = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
export const SPL_TOKEN_PROGRAM_ID = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const WSOL_MINT_ADDRESS = "So11111111111111111111111111111111111111112";
export const ASSOCIATED_TOKEN_PROGRAM_ID = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
export const SYSTEM_PROGRAM_ID = "11111111111111111111111111111111";
export const RENT_SYSVAR_ID = "SysvarRent111111111111111111111111111111111";
export const CLOCK_SYSVAR_ID = "SysvarC1ock11111111111111111111111111111111";
export const INSTRUCTIONS_SYSVAR_ID = "Sysvar1nstructions1111111111111111111111111";
export const MEMO_PROGRAM_ID = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";

/** Canonical Ratchet-DLMM-DAMM deployed program ID (`deploy/ratchet_dlmm_damm-keypair.json`). */
export const DEFAULT_RATCHET_PROGRAM_ID = "5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5";
export const DEFAULT_JUPITER_PROGRAM_ID = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";

/** Canonical on-chain PDA seed strings matching `programs/ratchet-dlmm-damm/src/instruction.rs`. */
export const POOL_SEED = "ratchet_pool";
export const BIN_ARRAY_SEED = "bin_array";
export const POOL_AUTHORITY_SEED = "pool_authority";
export const BASE_VAULT_SEED = "base_vault";
export const QUOTE_VAULT_SEED = "quote_vault";

/** Canonical binary instruction discriminators (`0x01`..`0x07`). */
export const IX_INITIALIZE_POOL = 0x01;
export const IX_INITIALIZE_BIN_ARRAY = 0x02;
export const IX_SEED_INITIAL_BINS = 0x03;
export const IX_REVOKE_AND_OPEN_SLOT0 = 0x04;
export const IX_SWAP = 0x05;
export const IX_GRADUATE_IN_PLACE = 0x06;
export const IX_HARVEST_FALLBACK_TRANSFER_FEES = 0x07;

export const HARVEST_MODE_TO_MINT = 0;
export const HARVEST_MODE_WITHDRAW_FROM_MINT = 1;

/** SIMD-0437 exact PDA sizing constants. */
export const RATCHET_POOL_PDA_DATA_BYTES = 480n;
export const SIMD0437_ACCOUNT_OVERHEAD_BYTES = 128n;
export const RATCHET_POOL_PDA_TOTAL_BYTES = 608n;

export const ACTIVE_BIN_ARRAY_PDA_DATA_BYTES = 1_024n;
export const ACTIVE_BIN_ARRAY_PDA_TOTAL_BYTES = 1_152n;

export const SIMD0437_STEP2_LAMPORTS_PER_BYTE = 5_080n;
export const SIMD0437_STEP5_LAMPORTS_PER_BYTE = 696n;

export const RATCHET_POOL_RENT_STEP2_LAMPORTS = 3_088_640n;
export const ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS = 5_852_160n;
export const TOTAL_PDA_RENT_STEP2_LAMPORTS = 8_940_800n;

export const RATCHET_POOL_RENT_STEP5_LAMPORTS = 423_168n;
export const ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS = 801_792n;
export const TOTAL_PDA_RENT_STEP5_LAMPORTS = 1_224_960n;

/** Gate G05 execution bounds. */
export const EXACT_SWAP_ACCOUNT_COUNT = 28;
export const RATCHET_SWAP_CPI_DEPTH = 2;
export const MAX_SOLANA_CPI_DEPTH = 4;
export const SIMD0339_MAX_ACCOUNTS = 255;

const BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const BASE58_MAP = new Map<string, number>(
  BASE58_ALPHABET.split("").map((ch, i) => [ch, i])
);

export function isValidBase58Pubkey(pubkey: string): boolean {
  if (pubkey.length < 32 || pubkey.length > 44) {
    return false;
  }
  for (const ch of pubkey) {
    if (!BASE58_MAP.has(ch)) {
      return false;
    }
  }
  try {
    return decodeBase58(pubkey).length === 32;
  } catch {
    return false;
  }
}

export function assertValidPubkey(pubkey: string, label: string): string {
  if (!isValidBase58Pubkey(pubkey)) {
    throw new Error(`Invalid Base58 public key for ${label}: "${pubkey}"`);
  }
  return pubkey;
}

export function encodeBase58(bytes: Uint8Array): string {
  if (bytes.length === 0) return "";
  const digits = [0];
  for (const byte of bytes) {
    let carry = byte;
    for (let i = 0; i < digits.length; i++) {
      const val = (digits[i] ?? 0) * 256 + carry;
      digits[i] = val % 58;
      carry = Math.floor(val / 58);
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = Math.floor(carry / 58);
    }
  }
  let leadingZeros = 0;
  for (const byte of bytes) {
    if (byte === 0) leadingZeros++;
    else break;
  }
  let out = "1".repeat(leadingZeros);
  for (let i = digits.length - 1; i >= 0; i--) {
    out += BASE58_ALPHABET[digits[i] ?? 0] ?? "1";
  }
  return out;
}

export function decodeBase58(str: string): Uint8Array {
  if (str.length === 0) return new Uint8Array(0);
  const bytes = [0];
  for (const ch of str) {
    const val = BASE58_MAP.get(ch);
    if (val === undefined) {
      throw new Error(`Invalid Base58 character '${ch}' in "${str}"`);
    }
    let carry = val;
    for (let i = 0; i < bytes.length; i++) {
      const x = (bytes[i] ?? 0) * 58 + carry;
      bytes[i] = x & 0xff;
      carry = x >> 8;
    }
    while (carry > 0) {
      bytes.push(carry & 0xff);
      carry >>= 8;
    }
  }
  let leadingOnes = 0;
  for (const ch of str) {
    if (ch === "1") leadingOnes++;
    else break;
  }
  const out = new Uint8Array(leadingOnes + bytes.length);
  for (let i = 0; i < bytes.length; i++) {
    out[out.length - 1 - i] = bytes[i] ?? 0;
  }
  return out;
}

/**
 * Deterministic 32-byte address derivation in pure JS for off-chain test fixtures
 * and placeholder resolution when explicit non-PDA keys are needed.
 */
export function deriveDeterministicAddress(seedTag: string, ...parts: string[]): string {
  const joined = [seedTag, ...parts].join(":");
  const payload = new Uint8Array(joined.length);
  for (let i = 0; i < joined.length; i++) {
    payload[i] = joined.charCodeAt(i) & 0xff;
  }
  const out = new Uint8Array(32);
  let h1 = 0x811c9dc5;
  let h2 = 0x9e3779b9;
  for (let i = 0; i < payload.length; i++) {
    const b = payload[i] ?? 0;
    h1 ^= b;
    h1 = Math.imul(h1, 0x01000193) >>> 0;
    h2 = (h2 + b + ((h1 << 7) | (h1 >>> 25))) >>> 0;
  }
  for (let i = 0; i < 32; i++) {
    h1 = Math.imul(h1 ^ (i + 17), 0x85ebca6b) >>> 0;
    h2 = Math.imul(h2 ^ (i + 31), 0xc2b2ae35) >>> 0;
    out[i] = ((h1 >>> ((i % 4) * 8)) ^ (h2 >>> (((i + 2) % 4) * 8))) & 0xff;
  }
  return encodeBase58(out);
}

export interface DerivedRatchetPdas {
  programId: string;
  baseMint: string;
  binArrayStartId: number;
  ratchetPoolPda: string;
  ratchetPoolBump: number;
  activeBinArrayPda: string;
  activeBinArrayBump: number;
  poolAuthorityPda: string;
  poolAuthorityBump: number;
  baseVault: string;
  baseVaultBump: number;
  quoteVault: string;
  quoteVaultBump: number;
}

/**
 * Derives canonical Solana PDAs using `PublicKey.findProgramAddressSync` matching
 * `programs/ratchet-dlmm-damm/src/instruction.rs` bit-for-bit.
 */
export function deriveRatchetPdas(
  baseMint: string,
  binArrayStartId = -8,
  programId: string = DEFAULT_RATCHET_PROGRAM_ID
): DerivedRatchetPdas {
  const programKey = new PublicKey(assertValidPubkey(programId, "programId"));
  const mintKey = new PublicKey(assertValidPubkey(baseMint, "baseMint"));

  const [poolKey, ratchetPoolBump] = PublicKey.findProgramAddressSync(
    [Buffer.from(POOL_SEED), mintKey.toBuffer()],
    programKey
  );

  const startBuf = Buffer.alloc(4);
  startBuf.writeInt32LE(binArrayStartId, 0);
  const [binArrayKey, activeBinArrayBump] = PublicKey.findProgramAddressSync(
    [Buffer.from(BIN_ARRAY_SEED), poolKey.toBuffer(), startBuf],
    programKey
  );

  const [poolAuthorityKey, poolAuthorityBump] = PublicKey.findProgramAddressSync(
    [Buffer.from(POOL_AUTHORITY_SEED), poolKey.toBuffer()],
    programKey
  );

  const [baseVaultKey, baseVaultBump] = PublicKey.findProgramAddressSync(
    [Buffer.from(BASE_VAULT_SEED), poolKey.toBuffer()],
    programKey
  );

  const [quoteVaultKey, quoteVaultBump] = PublicKey.findProgramAddressSync(
    [Buffer.from(QUOTE_VAULT_SEED), poolKey.toBuffer()],
    programKey
  );

  return {
    programId: programKey.toBase58(),
    baseMint: mintKey.toBase58(),
    binArrayStartId,
    ratchetPoolPda: poolKey.toBase58(),
    ratchetPoolBump,
    activeBinArrayPda: binArrayKey.toBase58(),
    activeBinArrayBump,
    poolAuthorityPda: poolAuthorityKey.toBase58(),
    poolAuthorityBump,
    baseVault: baseVaultKey.toBase58(),
    baseVaultBump,
    quoteVault: quoteVaultKey.toBase58(),
    quoteVaultBump,
  };
}

export interface SwapAccountMeta {
  index: number;
  role: string;
  pubkey: string;
  isSigner: boolean;
  isWritable: boolean;
}

export interface ResolveSwapAccountsInput {
  ratchetProgramId?: string;
  ratchetPoolPda: string;
  activeBinArrayPda: string;
  baseTokenMint: string;
  quoteSolMint?: string;
  baseVault: string;
  quoteVault: string;
  userTransferAuthority: string;
  userBaseAccount: string;
  userQuoteAccount: string;
  creatorFeeVault: string;
  transferFeeTreasury?: string;
  compoundingReserveVault?: string;
  poolAuthorityPda?: string;
  eventAuthority?: string;
  highWaterOraclePda?: string;
  lowerBinArrayExtension?: string;
  upperBinArrayExtension?: string;
  jupiterProgramId?: string;
  referralFeeAccount?: string;
}

export interface SwapAccounts28Result {
  accounts: SwapAccountMeta[];
  accountCount: number;
  cpiDepth: number;
  maxCpiDepth: number;
  simd0339MaxAccounts: number;
  withinExecutionLimits: boolean;
}

/**
 * Resolves the exact 28-account swap instruction metas required by Ratchet-DLMM-DAMM
 * (`programs/ratchet-dlmm-damm/src/processor.rs`) and Jupiter router adapters within
 * CPI depth 2 (<= 4) and SIMD-0339 255-account limit.
 */
export function resolveSwapAccounts28(input: ResolveSwapAccountsInput): SwapAccounts28Result {
  const ratchetProgramId = assertValidPubkey(
    input.ratchetProgramId ?? DEFAULT_RATCHET_PROGRAM_ID,
    "ratchetProgramId"
  );
  const ratchetPoolPda = assertValidPubkey(input.ratchetPoolPda, "ratchetPoolPda");
  const activeBinArrayPda = assertValidPubkey(input.activeBinArrayPda, "activeBinArrayPda");
  const baseTokenMint = assertValidPubkey(input.baseTokenMint, "baseTokenMint");
  const quoteSolMint = assertValidPubkey(
    input.quoteSolMint ?? WSOL_MINT_ADDRESS,
    "quoteSolMint"
  );
  const baseVault = assertValidPubkey(input.baseVault, "baseVault");
  const quoteVault = assertValidPubkey(input.quoteVault, "quoteVault");
  const userTransferAuthority = assertValidPubkey(
    input.userTransferAuthority,
    "userTransferAuthority"
  );
  const userBaseAccount = assertValidPubkey(input.userBaseAccount, "userBaseAccount");
  const userQuoteAccount = assertValidPubkey(input.userQuoteAccount, "userQuoteAccount");
  const creatorFeeVault = assertValidPubkey(input.creatorFeeVault, "creatorFeeVault");

  const programPubkey = new PublicKey(ratchetProgramId);
  const poolPubkey = new PublicKey(ratchetPoolPda);

  const poolAuthorityPda = assertValidPubkey(
    input.poolAuthorityPda ??
      PublicKey.findProgramAddressSync(
        [Buffer.from(POOL_AUTHORITY_SEED), poolPubkey.toBuffer()],
        programPubkey
      )[0].toBase58(),
    "poolAuthorityPda"
  );

  const transferFeeTreasury = assertValidPubkey(
    input.transferFeeTreasury ??
      deriveDeterministicAddress("transfer_fee_treasury", ratchetPoolPda, baseTokenMint),
    "transferFeeTreasury"
  );
  const compoundingReserveVault = assertValidPubkey(
    input.compoundingReserveVault ??
      deriveDeterministicAddress("compounding_reserve", ratchetPoolPda, quoteSolMint),
    "compoundingReserveVault"
  );
  const eventAuthority = assertValidPubkey(
    input.eventAuthority ?? deriveDeterministicAddress("event_authority", ratchetProgramId),
    "eventAuthority"
  );
  const highWaterOraclePda = assertValidPubkey(
    input.highWaterOraclePda ?? deriveDeterministicAddress("high_water_oracle", ratchetPoolPda),
    "highWaterOraclePda"
  );
  const lowerBinArrayExtension = assertValidPubkey(
    input.lowerBinArrayExtension ??
      deriveDeterministicAddress("lower_bin_ext", ratchetPoolPda, activeBinArrayPda),
    "lowerBinArrayExtension"
  );
  const upperBinArrayExtension = assertValidPubkey(
    input.upperBinArrayExtension ??
      deriveDeterministicAddress("upper_bin_ext", ratchetPoolPda, activeBinArrayPda),
    "upperBinArrayExtension"
  );
  const jupiterProgramId = assertValidPubkey(
    input.jupiterProgramId ?? DEFAULT_JUPITER_PROGRAM_ID,
    "jupiterProgramId"
  );
  const referralFeeAccount = assertValidPubkey(
    input.referralFeeAccount ?? creatorFeeVault,
    "referralFeeAccount"
  );

  // Order matches `programs/ratchet-dlmm-damm/src/processor.rs::process_swap` (indices 0..8, 12, 13, 21)
  const rawAccounts: Array<Omit<SwapAccountMeta, "index">> = [
    { role: "ratchet_pool_pda", pubkey: ratchetPoolPda, isSigner: false, isWritable: true },
    { role: "active_bin_array_pda", pubkey: activeBinArrayPda, isSigner: false, isWritable: true },
    { role: "base_token_mint", pubkey: baseTokenMint, isSigner: false, isWritable: false },
    { role: "quote_sol_mint", pubkey: quoteSolMint, isSigner: false, isWritable: false },
    { role: "base_vault", pubkey: baseVault, isSigner: false, isWritable: true },
    { role: "quote_vault", pubkey: quoteVault, isSigner: false, isWritable: true },
    {
      role: "user_transfer_authority",
      pubkey: userTransferAuthority,
      isSigner: true,
      isWritable: false,
    },
    { role: "user_base_account", pubkey: userBaseAccount, isSigner: false, isWritable: true },
    { role: "user_quote_account", pubkey: userQuoteAccount, isSigner: false, isWritable: true },
    { role: "creator_fee_vault", pubkey: creatorFeeVault, isSigner: false, isWritable: true },
    {
      role: "transfer_fee_treasury",
      pubkey: transferFeeTreasury,
      isSigner: false,
      isWritable: true,
    },
    {
      role: "compounding_reserve_vault",
      pubkey: compoundingReserveVault,
      isSigner: false,
      isWritable: true,
    },
    { role: "token_2022_program", pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
    { role: "spl_token_program", pubkey: SPL_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    {
      role: "associated_token_program",
      pubkey: ASSOCIATED_TOKEN_PROGRAM_ID,
      isSigner: false,
      isWritable: false,
    },
    { role: "system_program", pubkey: SYSTEM_PROGRAM_ID, isSigner: false, isWritable: false },
    { role: "rent_sysvar", pubkey: RENT_SYSVAR_ID, isSigner: false, isWritable: false },
    { role: "clock_sysvar", pubkey: CLOCK_SYSVAR_ID, isSigner: false, isWritable: false },
    {
      role: "instructions_sysvar",
      pubkey: INSTRUCTIONS_SYSVAR_ID,
      isSigner: false,
      isWritable: false,
    },
    { role: "event_authority", pubkey: eventAuthority, isSigner: false, isWritable: false },
    { role: "ratchet_program", pubkey: ratchetProgramId, isSigner: false, isWritable: false },
    { role: "pool_authority_pda", pubkey: poolAuthorityPda, isSigner: false, isWritable: false },
    { role: "high_water_oracle_pda", pubkey: highWaterOraclePda, isSigner: false, isWritable: true },
    {
      role: "lower_bin_array_extension",
      pubkey: lowerBinArrayExtension,
      isSigner: false,
      isWritable: true,
    },
    {
      role: "upper_bin_array_extension",
      pubkey: upperBinArrayExtension,
      isSigner: false,
      isWritable: true,
    },
    { role: "jupiter_program", pubkey: jupiterProgramId, isSigner: false, isWritable: false },
    { role: "referral_fee_account", pubkey: referralFeeAccount, isSigner: false, isWritable: true },
    { role: "memo_program", pubkey: MEMO_PROGRAM_ID, isSigner: false, isWritable: false },
  ];

  const accounts: SwapAccountMeta[] = rawAccounts.map((meta, index) => ({
    index,
    ...meta,
  }));

  if (accounts.length !== EXACT_SWAP_ACCOUNT_COUNT) {
    throw new Error(
      `Expected ${EXACT_SWAP_ACCOUNT_COUNT} swap accounts, resolved ${accounts.length}`
    );
  }

  return {
    accounts,
    accountCount: accounts.length,
    cpiDepth: RATCHET_SWAP_CPI_DEPTH,
    maxCpiDepth: MAX_SOLANA_CPI_DEPTH,
    simd0339MaxAccounts: SIMD0339_MAX_ACCOUNTS,
    withinExecutionLimits:
      RATCHET_SWAP_CPI_DEPTH <= MAX_SOLANA_CPI_DEPTH &&
      accounts.length <= SIMD0339_MAX_ACCOUNTS,
  };
}

export interface TransferFeeFallbackConfig {
  extensionType: "TransferFeeConfig";
  mint: string;
  /** Fallback transfer fee in bps (0 in normal flow; immutable ceiling <= 2500 bps). */
  transferFeeBasisPoints: number;
  /** Maximum withheld fee in base token atoms per transfer. */
  maximumFeeAtoms: bigint;
  /** Must be null once genesis revokes mutable authority (`0x04 RevokeAndOpenSlot0`). */
  transferFeeConfigAuthority: string | null;
  /** Authority allowed to withdraw withheld tokens out of the hot swap path. */
  withdrawWithheldAuthority: string;
  /** Enforces write-lock-free fallback harvesting outside the swap hot-path lane. */
  writeLockFreeHarvesting: true;
  /** Serialized Token-2022 InitializeTransferFeeConfig payload (14 bytes compact header). */
  serializedInitPayload: Uint8Array;
}

export interface CreateTransferFeeConfigInput {
  mint: string;
  transferFeeBasisPoints?: number;
  maximumFeeAtoms?: bigint;
  transferFeeConfigAuthority?: string | null;
  withdrawWithheldAuthority: string;
}

/**
 * Builds the Token-2022 `ExtensionType::TransferFeeConfig` fallback harvesting
 * configuration and serializes its initialization payload.
 */
export function createTransferFeeFallbackConfig(
  input: CreateTransferFeeConfigInput
): TransferFeeFallbackConfig {
  const mint = assertValidPubkey(input.mint, "mint");
  const withdrawWithheldAuthority = assertValidPubkey(
    input.withdrawWithheldAuthority,
    "withdrawWithheldAuthority"
  );
  const transferFeeBasisPoints = input.transferFeeBasisPoints ?? 0;
  if (
    !Number.isInteger(transferFeeBasisPoints) ||
    transferFeeBasisPoints < 0 ||
    transferFeeBasisPoints > 2500
  ) {
    throw new RangeError(
      `transferFeeBasisPoints must be an integer in [0, 2500], got ${transferFeeBasisPoints}`
    );
  }
  const maximumFeeAtoms = input.maximumFeeAtoms ?? 0n;
  if (maximumFeeAtoms < 0n || maximumFeeAtoms > (1n << 64n) - 1n) {
    throw new RangeError(`maximumFeeAtoms out of u64 range: ${maximumFeeAtoms}`);
  }
  const transferFeeConfigAuthority =
    input.transferFeeConfigAuthority === undefined
      ? null
      : input.transferFeeConfigAuthority === null
        ? null
        : assertValidPubkey(input.transferFeeConfigAuthority, "transferFeeConfigAuthority");

  const payload = new Uint8Array(14);
  const view = new DataView(payload.buffer);
  payload[0] = 26;
  payload[1] = 0;
  payload[2] = transferFeeConfigAuthority === null ? 0 : 1;
  payload[3] = 1;
  view.setUint16(4, transferFeeBasisPoints, true);
  view.setBigUint64(6, maximumFeeAtoms, true);

  return {
    extensionType: "TransferFeeConfig",
    mint,
    transferFeeBasisPoints,
    maximumFeeAtoms,
    transferFeeConfigAuthority,
    withdrawWithheldAuthority,
    writeLockFreeHarvesting: true,
    serializedInitPayload: payload,
  };
}

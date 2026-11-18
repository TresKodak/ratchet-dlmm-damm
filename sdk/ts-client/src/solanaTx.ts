/**
 * Production Solana Mainnet Transaction Builder, PDA Decoders & Config Loader
 * using `@solana/web3.js`, `@solana/spl-token`, and `@solana/spl-token-metadata`.
 *
 * Splits genesis deployment into deterministic <= 1,232-byte transaction packets
 * matching `programs/ratchet-dlmm-damm/src/processor.rs` (`0x01`..`0x07`).
 */

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  ExtensionType,
  LENGTH_SIZE,
  NATIVE_MINT,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  TYPE_SIZE,
  createAssociatedTokenAccountIdempotentInstruction,
  createInitializeMetadataPointerInstruction,
  createInitializeMintInstruction,
  createInitializeTransferFeeConfigInstruction,
  createMintToInstruction,
  createSyncNativeInstruction,
  getAssociatedTokenAddressSync,
  getMintLen,
} from "@solana/spl-token";
import {
  createInitializeInstruction as createInitializeTokenMetadataInstruction,
  createUpdateAuthorityInstruction as createUpdateTokenMetadataAuthorityInstruction,
  pack as packTokenMetadata,
} from "@solana/spl-token-metadata";
import {
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";
import {
  ACTIVE_BIN_ARRAY_PDA_DATA_BYTES,
  DEFAULT_RATCHET_PROGRAM_ID,
  RATCHET_POOL_PDA_DATA_BYTES,
  deriveRatchetPdas,
  type DerivedRatchetPdas,
} from "./accounts.js";
import {
  BUFFERED_SELL_IMPACT_CAP_BPS,
  DEFAULT_F0_BPS,
  DEFAULT_F_MIN_BPS,
  DEFAULT_GAMMA_BPS,
  HARD_SELL_IMPACT_CAP_BPS,
  IMMUTABLE_F_MAX_SELL_BPS,
  IMMUTABLE_MIN_COMPOUNDING_FEE_BPS,
  Q64_ONE,
  q64MulRound,
} from "./feePreview.js";
import {
  type RentStep,
  packInitializeBinArrayIxData,
  packInitializePoolIxData,
  packRevokeAndOpenSlot0IxData,
  packSeedInitialBinsIxData,
} from "./genesisBundle.js";

/** Maximum Solana UDP transaction packet size in bytes. */
export const MAX_SOLANA_PACKET_BYTES = 1_232;

export const PLACEHOLDER_PATTERNS = [
  "REPLACE_WITH_TOKEN_NAME",
  "REPLACE_WITH_SYMBOL",
  "REPLACE_WITH_METADATA_URI",
  "REPLACE_WITH_PROGRAM_ID",
] as const;

export interface LaunchConfig {
  token: {
    name: string;
    symbol: string;
    uri: string;
    decimals: number;
    totalSupplyTokens: string;
  };
  pool: {
    realVaultSol: string;
    maxExtractableSol: string;
    binStepBps: number;
    aBuy: string;
    compoundingFeeBps: number;
    f0Bps: number;
    fMinBps: number;
    fMaxSellBps: number;
    maxSellImpactBps: number;
    gammaBps: number;
    fallbackTransferFeeBps: number;
    fallbackMaximumFeeAtoms: string;
    rentStep: RentStep;
  };
  network: {
    cluster: string;
    rpcUrl: string;
    commitment: string;
  };
  paths: {
    payerKeypairPath: string;
    programId: string;
    programBinaryPath: string;
    programKeypairPath: string;
    mintKeypairPath: string;
    launchReceiptPath: string;
  };
  jito: {
    enabled: boolean;
    blockEngineUrl: string;
    tipLamports: number;
    tipAccount: string;
  };
}

export interface DecodedRatchetPoolPda {
  sqrtPHighWater: bigint;
  sqrtPCurrent: bigint;
  sqrtPStart: bigint;
  sqrtPGraduation: bigint;
  vAUp: bigint;
  vADown: bigint;
  aBuy: bigint;
  aSell: bigint;
  lastDecayTimestamp: bigint;
  realVaultLamports: bigint;
  maxExtractableLamports: bigint;
  compoundedSolTotal: bigint;
  protocolFeeLamports: bigint;
  graduationSlot: bigint;
  totalVolumeQuote: bigint;
  harvestedTransferFeeAtoms: bigint;
  activeBinId: number;
  floorBinId: number;
  binArrayStartId: number;
  decayPeriodSeconds: number;
  compoundingFeeBps: number;
  f0Bps: number;
  fMinBps: number;
  fMaxSellBps: number;
  binStepBps: number;
  maxSellImpactBps: number;
  gammaBps: number;
  transferFeeBps: number;
  graduationStatus: number;
  authorityRevoked: number;
  baseMint: string;
  quoteMint: string;
  baseVault: string;
  quoteVault: string;
  authority: string;
  activeBinArray: string;
  creatorFeeVault: string;
}

export interface BuiltGenesisPacket {
  name: string;
  description: string;
  transaction: Transaction;
  serializedBytes: number;
  withinPacketLimit: boolean;
  requiresMintSigner: boolean;
}

export interface GenesisTxPlan {
  pdas: DerivedRatchetPdas;
  payer: PublicKey;
  mint: PublicKey;
  payerBaseAta: PublicKey;
  payerWsolAta: PublicKey;
  totalMintAccountSpace: number;
  decimals: number;
  totalSupplyAtoms: bigint;
  realVaultLamports: bigint;
  maxExtractableLamports: bigint;
  surplusCushionLamports: bigint;
  packets: BuiltGenesisPacket[];
  maxPacketBytes: number;
  allPacketsWithinLimit: boolean;
}

export function solDecimalToLamports(solAmount: string): bigint {
  const trimmed = solAmount.trim();
  if (!/^\d+(\.\d{1,9})?$/.test(trimmed)) {
    throw new RangeError(`Invalid SOL decimal amount: "${solAmount}"`);
  }
  const [wholeStr, fracStr = ""] = trimmed.split(".");
  const whole = BigInt(wholeStr ?? "0");
  const frac = BigInt(fracStr.padEnd(9, "0"));
  return whole * BigInt(LAMPORTS_PER_SOL) + frac;
}

export function parseTokenSupplyAtoms(totalSupplyTokens: string, decimals: number): bigint {
  const trimmed = totalSupplyTokens.trim();
  if (!/^\d+$/.test(trimmed)) {
    throw new RangeError(`totalSupplyTokens must be an integer string, got "${totalSupplyTokens}"`);
  }
  const tokens = BigInt(trimmed);
  if (tokens <= 0n) {
    throw new RangeError("totalSupplyTokens must be > 0");
  }
  const atoms = tokens * 10n ** BigInt(decimals);
  if (atoms > (1n << 64n) - 1n) {
    throw new RangeError(`totalSupplyAtoms (${atoms}) exceeds u64::MAX`);
  }
  return atoms;
}

export function findPlaceholderViolations(config: LaunchConfig): string[] {
  const warnings: string[] = [];
  if (!config.token.name.trim() || config.token.name.includes("REPLACE_WITH")) {
    warnings.push(`token.name is still a placeholder ("${config.token.name}")`);
  }
  if (!config.token.symbol.trim() || config.token.symbol.includes("REPLACE_WITH")) {
    warnings.push(`token.symbol is still a placeholder ("${config.token.symbol}")`);
  }
  if (!config.token.uri.trim() || config.token.uri.includes("REPLACE_WITH")) {
    warnings.push(`token.uri is still a placeholder ("${config.token.uri}")`);
  }
  if (!config.paths.programId.trim() || config.paths.programId.includes("REPLACE_WITH")) {
    warnings.push(`paths.programId is still a placeholder ("${config.paths.programId}")`);
  }
  return warnings;
}

export function resolveUserPath(filePath: string, baseDir?: string): string {
  const expanded =
    filePath === "~"
      ? os.homedir()
      : filePath.startsWith("~/") || filePath.startsWith("~\\")
        ? path.join(os.homedir(), filePath.slice(2))
        : filePath;
  if (path.isAbsolute(expanded)) {
    return path.normalize(expanded);
  }
  return baseDir ? path.resolve(baseDir, expanded) : path.resolve(expanded);
}

export function loadAndValidateLaunchConfig(
  configPath: string,
  options: { allowPlaceholder?: boolean } = {}
): { config: LaunchConfig; placeholderWarnings: string[] } {
  const resolved = resolveUserPath(configPath);
  if (!fs.existsSync(resolved)) {
    throw new Error(
      `Launch config not found at ${resolved}. Copy deploy/launch.config.example.json to deploy/launch.config.json and configure your token & RPC settings.`
    );
  }
  const raw = fs.readFileSync(resolved, "utf8");
  const config = JSON.parse(raw) as LaunchConfig;

  const placeholderWarnings = findPlaceholderViolations(config);
  if (placeholderWarnings.length > 0 && !options.allowPlaceholder) {
    throw new Error(
      `Placeholder branding/config detected in ${resolved}:\n - ${placeholderWarnings.join("\n - ")}\nEdit deploy/launch.config.json (or pass --allow-placeholder in read-only preflight).`
    );
  }

  if (config.paths.programId !== DEFAULT_RATCHET_PROGRAM_ID) {
    throw new Error(
      `paths.programId (${config.paths.programId}) does not match canonical PROGRAM_ID (${DEFAULT_RATCHET_PROGRAM_ID})`
    );
  }

  const realVaultLamports = solDecimalToLamports(config.pool.realVaultSol);
  const maxExtractableLamports = solDecimalToLamports(config.pool.maxExtractableSol);
  if (maxExtractableLamports <= 0n || maxExtractableLamports > realVaultLamports) {
    throw new RangeError(
      `Gate G06 solvency violation: maxExtractableSol (${config.pool.maxExtractableSol}) must be in (0, realVaultSol (${config.pool.realVaultSol})]`
    );
  }

  if (BigInt(config.pool.f0Bps) !== DEFAULT_F0_BPS) {
    throw new RangeError(`Slot-0 f0Bps must be ${DEFAULT_F0_BPS} (80%), got ${config.pool.f0Bps}`);
  }
  if (BigInt(config.pool.fMinBps) !== DEFAULT_F_MIN_BPS) {
    throw new RangeError(`fMinBps must be ${DEFAULT_F_MIN_BPS}, got ${config.pool.fMinBps}`);
  }
  if (BigInt(config.pool.fMaxSellBps) !== IMMUTABLE_F_MAX_SELL_BPS) {
    throw new RangeError(
      `Immutable fMaxSellBps must be ${IMMUTABLE_F_MAX_SELL_BPS} (25%), got ${config.pool.fMaxSellBps}`
    );
  }
  if (BigInt(config.pool.compoundingFeeBps) < IMMUTABLE_MIN_COMPOUNDING_FEE_BPS) {
    throw new RangeError(
      `Immutable compoundingFeeBps must be >= ${IMMUTABLE_MIN_COMPOUNDING_FEE_BPS} (75%), got ${config.pool.compoundingFeeBps}`
    );
  }
  if (config.pool.maxSellImpactBps > Number(HARD_SELL_IMPACT_CAP_BPS)) {
    throw new RangeError(
      `maxSellImpactBps (${config.pool.maxSellImpactBps}) > hard cap ${HARD_SELL_IMPACT_CAP_BPS}`
    );
  }
  if (config.pool.binStepBps <= 0 || config.pool.binStepBps > 10_000) {
    throw new RangeError(`binStepBps out of range (0, 10000]: ${config.pool.binStepBps}`);
  }
  if (config.pool.rentStep !== "step2" && config.pool.rentStep !== "step5") {
    throw new RangeError(`rentStep must be "step2" or "step5", got "${config.pool.rentStep}"`);
  }

  parseTokenSupplyAtoms(config.token.totalSupplyTokens, config.token.decimals);
  return { config, placeholderWarnings };
}

/**
 * Computes `floor(sqrt(1 + binStepBps / 10_000) * 2^64)` using exact BigInt Newton-Raphson.
 */
export function computeSqrtBinStepQ64(binStepBps: number): bigint {
  const num = 10_000n + BigInt(binStepBps);
  const targetQ128 = (num << 128n) / 10_000n;
  let x = Q64_ONE;
  for (let i = 0; i < 64; i++) {
    const next = (x + targetQ128 / x) >> 1n;
    if (next === x || next + 1n === x) break;
    x = next;
  }
  while (x * x > targetQ128) x -= 1n;
  while ((x + 1n) * (x + 1n) <= targetQ128) x += 1n;
  return x;
}

/**
 * Exact floor integer square root for arbitrary-precision non-negative BigInt.
 */
export function bigIntSqrt(n: bigint): bigint {
  if (n < 0n) {
    throw new RangeError(`bigIntSqrt requires non-negative input, got ${n}`);
  }
  if (n < 2n) {
    return n;
  }
  let low = 1n;
  let high = 1n << BigInt((n.toString(2).length + 1) >> 1);
  while (low < high) {
    const mid = (low + high + 1n) >> 1n;
    if (mid * mid <= n) {
      low = mid;
    } else {
      high = mid - 1n;
    }
  }
  return low;
}

function computeToken2022TransferFee(
  amount: bigint,
  feeBps: number,
  maxFee: bigint
): bigint {
  if (amount <= 0n || feeBps <= 0 || maxFee <= 0n) {
    return 0n;
  }
  const raw = (amount * BigInt(feeBps) + 9_999n) / 10_000n;
  return raw > maxFee ? maxFee : raw;
}

function grossDepositForNetAmount(
  netTarget: bigint,
  feeBps: number,
  maxFee: bigint
): { gross: bigint; fee: bigint } {
  if (netTarget <= 0n || feeBps <= 0 || maxFee <= 0n) {
    return { gross: netTarget, fee: 0n };
  }
  let gross = netTarget + computeToken2022TransferFee(netTarget, feeBps, maxFee);
  for (let i = 0; i < 8; i++) {
    const fee = computeToken2022TransferFee(gross, feeBps, maxFee);
    const delivered = gross - fee;
    if (delivered >= netTarget) {
      return { gross, fee };
    }
    gross += netTarget - delivered;
  }
  const fee = computeToken2022TransferFee(gross, feeBps, maxFee);
  return { gross, fee };
}

export interface GenesisTxBuildOptions {
  config: LaunchConfig;
  payer: PublicKey;
  mint: PublicKey;
  mintRentLamports: bigint;
  recentBlockhash?: string;
}

/**
 * Builds real Token-2022 (`TransferFeeConfig` + `MetadataPointer` + `TokenMetadata`) +
 * WSOL + Ratchet genesis transactions (`0x01`..`0x04`), split into <= 1,232-byte packets.
 */
export function buildRealGenesisTransactionPlan(options: GenesisTxBuildOptions): GenesisTxPlan {
  const { config, payer, mint, mintRentLamports } = options;
  const recentBlockhash = options.recentBlockhash ?? "11111111111111111111111111111111";
  const programId = new PublicKey(config.paths.programId);

  const binArrayStartId = -8;
  const activeBinId = 0;
  const floorBinId = -8;
  const pdas = deriveRatchetPdas(mint.toBase58(), binArrayStartId, programId.toBase58());
  const poolPda = new PublicKey(pdas.ratchetPoolPda);
  const binArrayPda = new PublicKey(pdas.activeBinArrayPda);
  const poolAuthorityPda = new PublicKey(pdas.poolAuthorityPda);
  const baseVault = new PublicKey(pdas.baseVault);
  const quoteVault = new PublicKey(pdas.quoteVault);

  const decimals = config.token.decimals;
  const totalSupplyAtoms = parseTokenSupplyAtoms(config.token.totalSupplyTokens, decimals);
  const realVaultLamports = solDecimalToLamports(config.pool.realVaultSol);
  const maxExtractableLamports = solDecimalToLamports(config.pool.maxExtractableSol);
  const surplusCushionLamports = realVaultLamports - maxExtractableLamports;

  const fallbackFeeBps = config.pool.fallbackTransferFeeBps;
  const fallbackMaxFee = BigInt(config.pool.fallbackMaximumFeeAtoms);

  // Account for Token-2022 transfer fees withheld across 3 SeedInitialBins batches when fallbackFeeBps > 0
  const maxBatchSeedCount = 3n;
  const maxTotalSeedFee =
    fallbackFeeBps > 0 && fallbackMaxFee > 0n
      ? (() => {
          const est =
            ((totalSupplyAtoms * BigInt(fallbackFeeBps) + 9_999n) / 10_000n);
          const perBatchCap = est > fallbackMaxFee ? fallbackMaxFee : est;
          return perBatchCap * maxBatchSeedCount;
        })()
      : 0n;
  const seedableBaseAtoms = totalSupplyAtoms - maxTotalSeedFee;

  // Token-2022 Mint layout sizing
  const mintExtensions = [ExtensionType.TransferFeeConfig, ExtensionType.MetadataPointer];
  const baseMintLen = getMintLen(mintExtensions);
  const metadataPayloadLen = packTokenMetadata({
    mint,
    name: config.token.name,
    symbol: config.token.symbol,
    uri: config.token.uri,
    additionalMetadata: [],
  }).length;
  const totalMintAccountSpace = baseMintLen + TYPE_SIZE + LENGTH_SIZE + metadataPayloadLen;

  const payerBaseAta = getAssociatedTokenAddressSync(
    mint,
    payer,
    false,
    TOKEN_2022_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID
  );
  const payerWsolAta = getAssociatedTokenAddressSync(
    NATIVE_MINT,
    payer,
    false,
    TOKEN_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID
  );

  // Calibrate Q64.64 start & graduation sqrt(P) dynamically from vault liquidity & seedable supply
  const binStepBps = config.pool.binStepBps;
  const binStepQ64 = computeSqrtBinStepQ64(binStepBps);
  const targetStartPriceQ64 =
    seedableBaseAtoms > 0n
      ? ((maxExtractableLamports << 64n) + seedableBaseAtoms / 2n) / seedableBaseAtoms
      : Q64_ONE / 10_000n;
  const clampedPriceQ64 = targetStartPriceQ64 > 1_000n ? targetStartPriceQ64 : 1_000n;
  const startSqrtPriceQ64 = bigIntSqrt(clampedPriceQ64 << 64n);
  let activeSqrtP = startSqrtPriceQ64;
  for (let i = binArrayStartId; i < activeBinId; i++) {
    activeSqrtP = q64MulRound(activeSqrtP, binStepQ64);
  }
  let gradSqrtP = activeSqrtP;
  for (let i = 0; i < 12; i++) {
    gradSqrtP = q64MulRound(gradSqrtP, binStepQ64);
  }

  // Synthesize discrete bin entries across all 30 bins inside [binArrayStartId .. binArrayStartId + 30)
  // Quote bins: -8..=0 (9 bins); Base bins: 0..=21 (22 bins)
  const quoteBins = activeBinId - floorBinId + 1; // 9 bins
  const baseBins = 22; // bins 0..=21
  const quotePerBin = maxExtractableLamports / BigInt(quoteBins);
  const quoteRem = maxExtractableLamports % BigInt(quoteBins);
  const basePerBin = seedableBaseAtoms / BigInt(baseBins);
  const baseRem = seedableBaseAtoms % BigInt(baseBins);

  const binEntries: Array<{ binId: number; quoteSolLamports: bigint; baseTokenAtoms: bigint }> = [];
  for (let id = floorBinId; id < activeBinId + baseBins; id++) {
    const q =
      id <= activeBinId
        ? quotePerBin + (id === floorBinId ? quoteRem : 0n)
        : 0n;
    const b =
      id >= activeBinId
        ? basePerBin + (id === activeBinId ? baseRem : 0n)
        : 0n;
    if (q > 0n || b > 0n) {
      binEntries.push({ binId: id, quoteSolLamports: q, baseTokenAtoms: b });
    }
  }

  const makeTx = (
    name: string,
    description: string,
    instructions: TransactionInstruction[],
    requiresMintSigner: boolean
  ): BuiltGenesisPacket => {
    const tx = new Transaction();
    tx.feePayer = payer;
    tx.recentBlockhash = recentBlockhash;
    for (const ix of instructions) {
      tx.add(ix);
    }
    const serialized = tx.serialize({
      requireAllSignatures: false,
      verifySignatures: false,
    });
    return {
      name,
      description,
      transaction: tx,
      serializedBytes: serialized.byteLength,
      withinPacketLimit: serialized.byteLength <= MAX_SOLANA_PACKET_BYTES,
      requiresMintSigner,
    };
  };

  const packets: BuiltGenesisPacket[] = [];

  // Packet 1: Create Token-2022 Mint with TransferFeeConfig + MetadataPointer + InitializeMint2
  packets.push(
    makeTx(
      "tx1_create_token2022_mint",
      "Allocate Token-2022 Mint (TransferFeeConfig + MetadataPointer) and initialize decimals/authorities",
      [
        SystemProgram.createAccount({
          fromPubkey: payer,
          newAccountPubkey: mint,
          space: baseMintLen,
          lamports: Number(mintRentLamports),
          programId: TOKEN_2022_PROGRAM_ID,
        }),
        createInitializeTransferFeeConfigInstruction(
          mint,
          payer,
          poolAuthorityPda,
          fallbackFeeBps,
          fallbackMaxFee,
          TOKEN_2022_PROGRAM_ID
        ),
        createInitializeMetadataPointerInstruction(
          mint,
          payer,
          mint,
          TOKEN_2022_PROGRAM_ID
        ),
        createInitializeMintInstruction(
          mint,
          decimals,
          payer,
          payer,
          TOKEN_2022_PROGRAM_ID
        ),
      ],
      true
    )
  );

  // Packet 2: Initialize TokenMetadata (name/symbol/uri), lock updateAuthority=null, mint total supply to payer ATA
  packets.push(
    makeTx(
      "tx2_init_metadata_and_mint_supply",
      "Initialize Token-2022 TokenMetadata, revoke updateAuthority, create payer ATA & mint total supply",
      [
        createInitializeTokenMetadataInstruction({
          programId: TOKEN_2022_PROGRAM_ID,
          metadata: mint,
          updateAuthority: payer,
          mint,
          mintAuthority: payer,
          name: config.token.name,
          symbol: config.token.symbol,
          uri: config.token.uri,
        }),
        createUpdateTokenMetadataAuthorityInstruction({
          programId: TOKEN_2022_PROGRAM_ID,
          metadata: mint,
          oldAuthority: payer,
          newAuthority: null,
        }),
        createAssociatedTokenAccountIdempotentInstruction(
          payer,
          payerBaseAta,
          payer,
          mint,
          TOKEN_2022_PROGRAM_ID,
          ASSOCIATED_TOKEN_PROGRAM_ID
        ),
        createMintToInstruction(
          mint,
          payerBaseAta,
          payer,
          totalSupplyAtoms,
          [],
          TOKEN_2022_PROGRAM_ID
        ),
      ],
      false
    )
  );

  // Packet 3: Wrap initial WSOL + 0x01 InitializePool + 0x02 InitializeBinArray
  const initPoolIx = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: poolPda, isSigner: false, isWritable: true },
      { pubkey: binArrayPda, isSigner: false, isWritable: false },
      { pubkey: poolAuthorityPda, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false },
      { pubkey: NATIVE_MINT, isSigner: false, isWritable: false },
      { pubkey: baseVault, isSigner: false, isWritable: true },
      { pubkey: quoteVault, isSigner: false, isWritable: true },
      { pubkey: payer, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(
      packInitializePoolIxData({
        sqrtPStart: activeSqrtP,
        sqrtPGraduation: gradSqrtP,
        aBuy: BigInt(config.pool.aBuy),
        binStepBps,
        activeBinId,
        floorBinId,
        binArrayStartId,
        compoundingFeeBps: config.pool.compoundingFeeBps,
        gammaBps: config.pool.gammaBps ?? Number(DEFAULT_GAMMA_BPS),
        maxSellImpactBps:
          config.pool.maxSellImpactBps ?? Number(BUFFERED_SELL_IMPACT_CAP_BPS),
        transferFeeBps: fallbackFeeBps,
      })
    ),
  });

  const initBinArrayIx = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payer, isSigner: true, isWritable: true },
      { pubkey: poolPda, isSigner: false, isWritable: false },
      { pubkey: binArrayPda, isSigner: false, isWritable: true },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(
      packInitializeBinArrayIxData({
        startBinId: binArrayStartId,
        activeBinId,
        floorBinId,
        binStepBps,
        startSqrtPriceQ64,
      })
    ),
  });

  packets.push(
    makeTx(
      "tx3_wrap_sol_init_pool_and_bins",
      "Wrap initial WSOL vault reserve, InitializePool (0x01) and InitializeBinArray (0x02)",
      [
        createAssociatedTokenAccountIdempotentInstruction(
          payer,
          payerWsolAta,
          payer,
          NATIVE_MINT,
          TOKEN_PROGRAM_ID,
          ASSOCIATED_TOKEN_PROGRAM_ID
        ),
        SystemProgram.transfer({
          fromPubkey: payer,
          toPubkey: payerWsolAta,
          lamports: realVaultLamports,
        }),
        createSyncNativeInstruction(payerWsolAta, TOKEN_PROGRAM_ID),
        initPoolIx,
        initBinArrayIx,
      ],
      false
    )
  );

  // Packets 4a..4c: Chunked 0x03 SeedInitialBins (10 entries per tx across all 30 bins, <= 1,232 bytes)
  const chunkSize = 10;
  for (let offset = 0; offset < binEntries.length; offset += chunkSize) {
    const chunk = binEntries.slice(offset, offset + chunkSize);
    const batchIdx = Math.floor(offset / chunkSize) + 1;
    const isFirstBatch = offset === 0;
    const batchQuoteSum = chunk.reduce((s, e) => s + e.quoteSolLamports, 0n);
    const batchBaseSum = chunk.reduce((s, e) => s + e.baseTokenAtoms, 0n);
    const quoteDepositLamports =
      batchQuoteSum + (isFirstBatch ? surplusCushionLamports : 0n);
    const { gross: baseDepositAtoms } = grossDepositForNetAmount(
      batchBaseSum,
      fallbackFeeBps,
      fallbackMaxFee
    );

    const seedIx = new TransactionInstruction({
      programId,
      keys: [
        { pubkey: payer, isSigner: true, isWritable: false },
        { pubkey: poolPda, isSigner: false, isWritable: true },
        { pubkey: binArrayPda, isSigner: false, isWritable: true },
        { pubkey: mint, isSigner: false, isWritable: false },
        { pubkey: baseVault, isSigner: false, isWritable: true },
        { pubkey: quoteVault, isSigner: false, isWritable: true },
        { pubkey: payerBaseAta, isSigner: false, isWritable: true },
        { pubkey: payerWsolAta, isSigner: false, isWritable: true },
        { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: Buffer.from(
        packSeedInitialBinsIxData({
          quoteDepositLamports,
          baseDepositAtoms,
          entries: chunk,
        })
      ),
    });

    packets.push(
      makeTx(
        `tx4_seed_initial_bins_batch_${batchIdx}`,
        `SeedInitialBins (0x03) batch ${batchIdx} (${chunk.length} discrete bins)`,
        [seedIx],
        false
      )
    );
  }

  // Packet 5: 0x04 RevokeAndOpenSlot0 (revokes all authorities & opens Slot-0 at 80% base buy fee)
  const revokeIx = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payer, isSigner: true, isWritable: false },
      { pubkey: poolPda, isSigner: false, isWritable: true },
      { pubkey: binArrayPda, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: true },
      { pubkey: quoteVault, isSigner: false, isWritable: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(packRevokeAndOpenSlot0IxData()),
  });

  packets.push(
    makeTx(
      "tx5_revoke_and_open_slot0",
      "RevokeAndOpenSlot0 (0x04): revoke mint/freeze/transferFeeConfig/pool authorities and open Slot-0 at 80% base buy fee",
      [revokeIx],
      false
    )
  );

  const maxPacketBytes = packets.reduce((m, p) => Math.max(m, p.serializedBytes), 0);
  const allPacketsWithinLimit = packets.every((p) => p.withinPacketLimit);

  return {
    pdas,
    payer,
    mint,
    payerBaseAta,
    payerWsolAta,
    totalMintAccountSpace,
    decimals,
    totalSupplyAtoms,
    realVaultLamports,
    maxExtractableLamports,
    surplusCushionLamports,
    packets,
    maxPacketBytes,
    allPacketsWithinLimit,
  };
}

function readU128LE(buf: Uint8Array, offset: number): bigint {
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const lo = view.getBigUint64(offset, true);
  const hi = view.getBigUint64(offset + 8, true);
  return lo | (hi << 64n);
}

/**
 * Decodes the 480-byte zero-copy `RatchetPoolPda` account state matching `programs/ratchet-dlmm-damm/src/state.rs`.
 */
export function decodeRatchetPoolPda(data: Uint8Array): DecodedRatchetPoolPda {
  if (data.length !== Number(RATCHET_POOL_PDA_DATA_BYTES)) {
    throw new Error(
      `Expected ${RATCHET_POOL_PDA_DATA_BYTES}-byte RatchetPoolPda, got ${data.length}`
    );
  }
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const pk = (off: number) => new PublicKey(data.slice(off, off + 32)).toBase58();
  return {
    sqrtPHighWater: readU128LE(data, 0),
    sqrtPCurrent: readU128LE(data, 16),
    sqrtPStart: readU128LE(data, 32),
    sqrtPGraduation: readU128LE(data, 48),
    vAUp: view.getBigUint64(64, true),
    vADown: view.getBigUint64(72, true),
    aBuy: view.getBigUint64(80, true),
    aSell: view.getBigUint64(88, true),
    lastDecayTimestamp: view.getBigInt64(96, true),
    realVaultLamports: view.getBigUint64(104, true),
    maxExtractableLamports: view.getBigUint64(112, true),
    compoundedSolTotal: view.getBigUint64(120, true),
    protocolFeeLamports: view.getBigUint64(128, true),
    graduationSlot: view.getBigUint64(136, true),
    totalVolumeQuote: view.getBigUint64(144, true),
    harvestedTransferFeeAtoms: view.getBigUint64(152, true),
    activeBinId: view.getInt32(160, true),
    floorBinId: view.getInt32(164, true),
    binArrayStartId: view.getInt32(168, true),
    decayPeriodSeconds: view.getUint32(172, true),
    compoundingFeeBps: view.getUint16(176, true),
    f0Bps: view.getUint16(178, true),
    fMinBps: view.getUint16(180, true),
    fMaxSellBps: view.getUint16(182, true),
    binStepBps: view.getUint16(184, true),
    maxSellImpactBps: view.getUint16(186, true),
    gammaBps: view.getUint16(188, true),
    transferFeeBps: view.getUint16(190, true),
    graduationStatus: data[192] ?? 0,
    authorityRevoked: data[193] ?? 0,
    baseMint: pk(200),
    quoteMint: pk(232),
    baseVault: pk(264),
    quoteVault: pk(296),
    authority: pk(328),
    activeBinArray: pk(360),
    creatorFeeVault: pk(392),
  };
}

export interface DecodedLiquidityBin {
  binId: number;
  sqrtPriceQ64: bigint;
  reserveQuoteLamports: bigint;
  reserveBaseAtoms: bigint;
}

export interface DecodedActiveBinArrayPda {
  binStepQ64: bigint;
  pool: string;
  activeBinId: number;
  floorBinId: number;
  startBinId: number;
  binStepBps: number;
  bump: number;
  version: number;
  bins: DecodedLiquidityBin[];
}

/**
 * Decodes the 1,024-byte zero-copy `ActiveBinArrayPda` matching `programs/ratchet-dlmm-damm/src/state.rs`.
 */
export function decodeActiveBinArrayPda(data: Uint8Array): DecodedActiveBinArrayPda {
  if (data.length !== Number(ACTIVE_BIN_ARRAY_PDA_DATA_BYTES)) {
    throw new Error(
      `Expected ${ACTIVE_BIN_ARRAY_PDA_DATA_BYTES}-byte ActiveBinArrayPda, got ${data.length}`
    );
  }
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const startBinId = view.getInt32(56, true);
  const bins: DecodedLiquidityBin[] = [];
  for (let i = 0; i < 30; i++) {
    const off = 64 + i * 32;
    bins.push({
      binId: startBinId + i,
      sqrtPriceQ64: readU128LE(data, off),
      reserveQuoteLamports: view.getBigUint64(off + 16, true),
      reserveBaseAtoms: view.getBigUint64(off + 24, true),
    });
  }
  return {
    binStepQ64: readU128LE(data, 0),
    pool: new PublicKey(data.slice(16, 48)).toBase58(),
    activeBinId: view.getInt32(48, true),
    floorBinId: view.getInt32(52, true),
    startBinId,
    binStepBps: view.getUint16(60, true),
    bump: data[62] ?? 0,
    version: data[63] ?? 0,
    bins,
  };
}

export function validateActiveBinArrayPdaLen(data: Uint8Array): boolean {
  return data.length === Number(ACTIVE_BIN_ARRAY_PDA_DATA_BYTES);
}

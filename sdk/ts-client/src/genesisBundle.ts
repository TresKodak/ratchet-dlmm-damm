/**
 * Atomic Genesis Bundle Builder & exact on-chain binary wire serializers (`0x01`..`0x07`)
 * matching `programs/ratchet-dlmm-damm/src/instruction.rs` bit-for-bit.
 */

import {
  ACTIVE_BIN_ARRAY_PDA_DATA_BYTES,
  ACTIVE_BIN_ARRAY_PDA_TOTAL_BYTES,
  ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS,
  ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS,
  DEFAULT_RATCHET_PROGRAM_ID,
  HARVEST_MODE_TO_MINT,
  IX_GRADUATE_IN_PLACE,
  IX_HARVEST_FALLBACK_TRANSFER_FEES,
  IX_INITIALIZE_BIN_ARRAY,
  IX_INITIALIZE_POOL,
  IX_REVOKE_AND_OPEN_SLOT0,
  IX_SEED_INITIAL_BINS,
  IX_SWAP,
  RATCHET_POOL_PDA_DATA_BYTES,
  RATCHET_POOL_PDA_TOTAL_BYTES,
  RATCHET_POOL_RENT_STEP2_LAMPORTS,
  RATCHET_POOL_RENT_STEP5_LAMPORTS,
  SPL_TOKEN_PROGRAM_ID,
  SYSTEM_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  TOTAL_PDA_RENT_STEP2_LAMPORTS,
  TOTAL_PDA_RENT_STEP5_LAMPORTS,
  WSOL_MINT_ADDRESS,
  assertValidPubkey,
  createTransferFeeFallbackConfig,
  deriveDeterministicAddress,
  deriveRatchetPdas,
  type TransferFeeFallbackConfig,
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
  assertU128,
  assertU64,
} from "./feePreview.js";

export type RentStep = "step2" | "step5";

export interface InitialBinSeed {
  binId: number;
  sqrtPriceQ64: bigint;
  quoteSolLamports: bigint;
  baseTokenAtoms: bigint;
}

export interface AtomicGenesisParams {
  payer: string;
  ratchetProgramId?: string;
  baseTokenMint?: string;
  ratchetPoolPda?: string;
  activeBinArrayPda?: string;
  poolAuthorityPda?: string;
  baseVault?: string;
  quoteVault?: string;
  creatorFeeVault?: string;
  withdrawWithheldAuthority?: string;
  /** Token decimals for Token-2022 mint (default: 6). */
  decimals?: number;
  /** Total base token supply seeded across initial bins (default: 1_000_000_000_000_000n). */
  initialBaseSupplyAtoms?: bigint;
  /** Real SOL vault reserve at genesis in lamports (default: 86_500_000_000n = 86.5 SOL). */
  realVaultLamports?: bigint;
  /** Max extractable SOL lamports enforced by Gate G06 (default: 85_000_000_000n = 85.0 SOL). */
  maxExtractableLamports?: bigint;
  /** Initial Q64.64 sqrt price (default: Q64_ONE). */
  initialSqrtPriceQ64?: bigint;
  /** Graduation Q64.64 sqrt price target (default: 4 * initialSqrtPriceQ64). */
  graduationSqrtPriceQ64?: bigint;
  /** Active bin ID at Slot-0 (default: 0). */
  initialActiveBinId?: number;
  /** Floor bin index at genesis (default: -8). */
  floorBinIndex?: number;
  /** Discrete bin step in bps (default: 25). */
  binStepBps?: number;
  /** Volatility control constant A_buy (default: 50_000n). */
  aBuy?: bigint;
  /** High-water geometric decay factor gamma in bps (default: 9000 = 0.90). */
  gammaBps?: number;
  /** Single-leg sell price impact cap in bps (default: 700, max 750). */
  maxSellImpactBps?: number;
  /** Slot-0 base buy fee in bps (immutable genesis policy: 8000 = 80%). */
  f0Bps?: bigint;
  /** Asymptotic min base fee in bps (default: 25). */
  fMinBps?: bigint;
  /** Immutable max sell fee cap in bps (must equal 2500 = 25%). */
  fMaxSellBps?: bigint;
  /** Immutable min OnlyB auto-compounding share in bps (must be >= 7500 = 75%). */
  compoundingFeeBps?: bigint;
  /** Fallback Token-2022 transfer fee in bps (default: 0, max 2500). */
  fallbackTransferFeeBps?: number;
  /** Max fallback withheld fee per transfer in atoms (default: 0n). */
  fallbackMaximumFeeAtoms?: bigint;
  /** SIMD-0437 active rent step for PDA funding (default: "step2"). */
  rentStep?: RentStep;
  /** Optional custom bin seeds; if omitted, 16 uniform/floor bins are synthesized. */
  initialBins?: InitialBinSeed[];
}

export interface BundleAccountRef {
  pubkey: string;
  isSigner: boolean;
  isWritable: boolean;
}

export interface AtomicGenesisInstruction {
  step: number;
  name: string;
  programId: string;
  accounts: BundleAccountRef[];
  data: Uint8Array;
  lamportsFunded: bigint;
}

export interface RentAllocationSummary {
  activeRentStep: RentStep;
  ratchetPoolPdaDataBytes: bigint;
  ratchetPoolPdaTotalBytes: bigint;
  activeBinArrayPdaDataBytes: bigint;
  activeBinArrayPdaTotalBytes: bigint;
  step2: {
    ratchetPoolLamports: bigint;
    activeBinArrayLamports: bigint;
    totalLamports: bigint;
  };
  step5: {
    ratchetPoolLamports: bigint;
    activeBinArrayLamports: bigint;
    totalLamports: bigint;
  };
  fundedRatchetPoolLamports: bigint;
  fundedActiveBinArrayLamports: bigint;
  fundedTotalLamports: bigint;
}

export interface ImmutablePolicyCertificate {
  authorityRevoked: true;
  mintAuthorityRevoked: true;
  freezeAuthorityRevoked: true;
  transferFeeConfigAuthorityRevoked: true;
  immutableMaxSellFeeBps: bigint;
  immutableMinCompoundingFeeBps: bigint;
  configuredCompoundingFeeBps: bigint;
  slot0BaseBuyFeeBps: bigint;
  slot0Opened: true;
  solvencyReserveLamports: bigint;
  maxExtractableLamports: bigint;
  solvencyMarginLamports: bigint;
}

export interface AtomicGenesisBundle {
  bundleId: string;
  ratchetProgramId: string;
  baseTokenMint: string;
  quoteSolMint: string;
  ratchetPoolPda: string;
  activeBinArrayPda: string;
  poolAuthorityPda: string;
  baseVault: string;
  quoteVault: string;
  creatorFeeVault: string;
  decimals: number;
  binStepBps: number;
  initialActiveBinId: number;
  floorBinIndex: number;
  initialSqrtPriceQ64: bigint;
  graduationSqrtPriceQ64: bigint;
  aBuy: bigint;
  aSell: bigint;
  transferFeeFallbackConfig: TransferFeeFallbackConfig;
  rentAllocation: RentAllocationSummary;
  initialBins: InitialBinSeed[];
  policy: ImmutablePolicyCertificate;
  instructions: AtomicGenesisInstruction[];
}

function encodeU16LE(val: number): Uint8Array {
  const b = new Uint8Array(2);
  new DataView(b.buffer).setUint16(0, val, true);
  return b;
}

function encodeI32LE(val: number): Uint8Array {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setInt32(0, val, true);
  return b;
}

function encodeU64LE(val: bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, val, true);
  return b;
}

function encodeU128LE(val: bigint): Uint8Array {
  const b = new Uint8Array(16);
  const view = new DataView(b.buffer);
  view.setBigUint64(0, val & ((1n << 64n) - 1n), true);
  view.setBigUint64(8, val >> 64n, true);
  return b;
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((acc, c) => acc + c.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

/** Packs `0x01 InitializePool` (64 bytes) matching `instruction.rs::InitializePoolArgs`. */
export function packInitializePoolIxData(args: {
  sqrtPStart: bigint;
  sqrtPGraduation: bigint;
  aBuy: bigint;
  binStepBps: number;
  activeBinId: number;
  floorBinId: number;
  binArrayStartId: number;
  compoundingFeeBps: number;
  gammaBps: number;
  maxSellImpactBps: number;
  transferFeeBps: number;
  flags?: number;
}): Uint8Array {
  return concatBytes(
    new Uint8Array([IX_INITIALIZE_POOL]),
    encodeU128LE(args.sqrtPStart),
    encodeU128LE(args.sqrtPGraduation),
    encodeU64LE(args.aBuy),
    encodeU16LE(args.binStepBps),
    encodeI32LE(args.activeBinId),
    encodeI32LE(args.floorBinId),
    encodeI32LE(args.binArrayStartId),
    encodeU16LE(args.compoundingFeeBps),
    encodeU16LE(args.gammaBps),
    encodeU16LE(args.maxSellImpactBps),
    encodeU16LE(args.transferFeeBps),
    new Uint8Array([args.flags ?? 0])
  );
}

/** Packs `0x02 InitializeBinArray` (31 bytes) matching `instruction.rs::InitializeBinArrayArgs`. */
export function packInitializeBinArrayIxData(args: {
  startBinId: number;
  activeBinId: number;
  floorBinId: number;
  binStepBps: number;
  startSqrtPriceQ64: bigint;
}): Uint8Array {
  return concatBytes(
    new Uint8Array([IX_INITIALIZE_BIN_ARRAY]),
    encodeI32LE(args.startBinId),
    encodeI32LE(args.activeBinId),
    encodeI32LE(args.floorBinId),
    encodeU16LE(args.binStepBps),
    encodeU128LE(args.startSqrtPriceQ64)
  );
}

/** Packs `0x03 SeedInitialBins` (`18 + 20 * entries.length` bytes) matching `instruction.rs::SeedInitialBinsArgs`. */
export function packSeedInitialBinsIxData(args: {
  quoteDepositLamports: bigint;
  baseDepositAtoms: bigint;
  entries: Array<{ binId: number; quoteSolLamports: bigint; baseTokenAtoms: bigint }>;
}): Uint8Array {
  if (args.entries.length === 0 || args.entries.length > 255) {
    throw new RangeError(`SeedInitialBins entries length must be in [1, 255], got ${args.entries.length}`);
  }
  return concatBytes(
    new Uint8Array([IX_SEED_INITIAL_BINS, args.entries.length]),
    encodeU64LE(args.quoteDepositLamports),
    encodeU64LE(args.baseDepositAtoms),
    ...args.entries.map((e) =>
      concatBytes(
        encodeI32LE(e.binId),
        encodeU64LE(e.quoteSolLamports),
        encodeU64LE(e.baseTokenAtoms)
      )
    )
  );
}

/** Packs `0x04 RevokeAndOpenSlot0` (1 byte). */
export function packRevokeAndOpenSlot0IxData(): Uint8Array {
  return new Uint8Array([IX_REVOKE_AND_OPEN_SLOT0]);
}

/** Packs `0x05 Swap` (18 bytes) matching `instruction.rs::SwapArgs`. */
export function packSwapIxData(args: {
  isBuy: boolean;
  amountIn: bigint;
  minAmountOut: bigint;
}): Uint8Array {
  return concatBytes(
    new Uint8Array([IX_SWAP, args.isBuy ? 1 : 0]),
    encodeU64LE(args.amountIn),
    encodeU64LE(args.minAmountOut)
  );
}

/** Packs `0x06 GraduateInPlace` (1 byte). */
export function packGraduateInPlaceIxData(): Uint8Array {
  return new Uint8Array([IX_GRADUATE_IN_PLACE]);
}

/** Packs `0x07 HarvestFallbackTransferFees` (2 bytes). */
export function packHarvestFallbackIxData(mode: 0 | 1 = HARVEST_MODE_TO_MINT): Uint8Array {
  return new Uint8Array([IX_HARVEST_FALLBACK_TRANSFER_FEES, mode]);
}

function synthesizeInitialBins(
  activeBinId: number,
  floorBinIndex: number,
  initialSqrtPriceQ64: bigint,
  binStepBps: number,
  maxExtractableLamports: bigint,
  totalBaseAtoms: bigint
): InitialBinSeed[] {
  const binCount = 16;
  const startBinId = floorBinIndex;
  const bins: InitialBinSeed[] = [];
  const quoteBinCount = BigInt(Math.max(1, activeBinId - startBinId + 1));
  const baseBinCount = BigInt(Math.max(1, startBinId + binCount - activeBinId));
  const quotePerBin = maxExtractableLamports / quoteBinCount;
  const quoteRemainder = maxExtractableLamports % quoteBinCount;
  const basePerBin = totalBaseAtoms / baseBinCount;
  const baseRemainder = totalBaseAtoms % baseBinCount;

  let quoteIdx = 0n;
  let baseIdx = 0n;

  for (let i = 0; i < binCount; i++) {
    const binId = startBinId + i;
    const delta = BigInt(binId - activeBinId);
    const sqrtPriceQ64 =
      delta >= 0n
        ? (initialSqrtPriceQ64 * (20_000n + delta * BigInt(binStepBps))) / 20_000n
        : (initialSqrtPriceQ64 * 20_000n) / (20_000n + -delta * BigInt(binStepBps));

    let quoteSolLamports = 0n;
    let baseTokenAtoms = 0n;
    if (binId <= activeBinId) {
      quoteSolLamports = quotePerBin + (quoteIdx === 0n ? quoteRemainder : 0n);
      quoteIdx++;
    }
    if (binId >= activeBinId) {
      baseTokenAtoms = basePerBin + (baseIdx === 0n ? baseRemainder : 0n);
      baseIdx++;
    }

    bins.push({
      binId,
      sqrtPriceQ64,
      quoteSolLamports,
      baseTokenAtoms,
    });
  }
  return bins;
}

/**
 * Builds the atomic Slot-0 genesis bundle for Ratchet-DLMM-DAMM aligned with
 * `programs/ratchet-dlmm-damm/src/instruction.rs` (`0x01`..`0x04` and real PDAs).
 */
export function buildAtomicGenesisBundle(params: AtomicGenesisParams): AtomicGenesisBundle {
  const payer = assertValidPubkey(params.payer, "payer");
  const ratchetProgramId = assertValidPubkey(
    params.ratchetProgramId ?? DEFAULT_RATCHET_PROGRAM_ID,
    "ratchetProgramId"
  );
  const baseTokenMint = assertValidPubkey(
    params.baseTokenMint ?? deriveDeterministicAddress("token2022_mint", payer, ratchetProgramId),
    "baseTokenMint"
  );

  const floorBinIndex = params.floorBinIndex ?? -8;
  const derivedPdas = deriveRatchetPdas(baseTokenMint, floorBinIndex, ratchetProgramId);

  const ratchetPoolPda = assertValidPubkey(
    params.ratchetPoolPda ?? derivedPdas.ratchetPoolPda,
    "ratchetPoolPda"
  );
  const activeBinArrayPda = assertValidPubkey(
    params.activeBinArrayPda ?? derivedPdas.activeBinArrayPda,
    "activeBinArrayPda"
  );
  const poolAuthorityPda = assertValidPubkey(
    params.poolAuthorityPda ?? derivedPdas.poolAuthorityPda,
    "poolAuthorityPda"
  );
  const baseVault = assertValidPubkey(
    params.baseVault ?? derivedPdas.baseVault,
    "baseVault"
  );
  const quoteVault = assertValidPubkey(
    params.quoteVault ?? derivedPdas.quoteVault,
    "quoteVault"
  );
  const creatorFeeVault = assertValidPubkey(
    params.creatorFeeVault ?? payer,
    "creatorFeeVault"
  );
  const withdrawWithheldAuthority = assertValidPubkey(
    params.withdrawWithheldAuthority ?? poolAuthorityPda,
    "withdrawWithheldAuthority"
  );

  const decimals = params.decimals ?? 6;
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 18) {
    throw new RangeError(`decimals must be in [0, 18], got ${decimals}`);
  }

  const initialBaseSupplyAtoms = assertU64(
    params.initialBaseSupplyAtoms ?? 1_000_000_000_000_000n,
    "initialBaseSupplyAtoms"
  );
  const realVaultLamports = assertU64(
    params.realVaultLamports ?? 86_500_000_000n,
    "realVaultLamports"
  );
  const maxExtractableLamports = assertU64(
    params.maxExtractableLamports ?? 85_000_000_000n,
    "maxExtractableLamports"
  );
  if (maxExtractableLamports > realVaultLamports) {
    throw new RangeError(
      `Gate G06 violation: maxExtractableLamports (${maxExtractableLamports}) > realVaultLamports (${realVaultLamports})`
    );
  }

  const initialSqrtPriceQ64 = assertU128(
    params.initialSqrtPriceQ64 ?? Q64_ONE,
    "initialSqrtPriceQ64"
  );
  if (initialSqrtPriceQ64 === 0n) {
    throw new RangeError("initialSqrtPriceQ64 must be > 0");
  }
  const graduationSqrtPriceQ64 = assertU128(
    params.graduationSqrtPriceQ64 ?? initialSqrtPriceQ64 * 4n,
    "graduationSqrtPriceQ64"
  );
  if (graduationSqrtPriceQ64 < initialSqrtPriceQ64) {
    throw new RangeError("graduationSqrtPriceQ64 must be >= initialSqrtPriceQ64");
  }

  const initialActiveBinId = params.initialActiveBinId ?? 0;
  if (floorBinIndex > initialActiveBinId) {
    throw new RangeError(
      `floorBinIndex (${floorBinIndex}) must be <= initialActiveBinId (${initialActiveBinId})`
    );
  }

  const binStepBps = params.binStepBps ?? 25;
  if (!Number.isInteger(binStepBps) || binStepBps <= 0 || binStepBps > 10_000) {
    throw new RangeError(`binStepBps must be in (0, 10000], got ${binStepBps}`);
  }

  const gammaBps = params.gammaBps ?? Number(DEFAULT_GAMMA_BPS);
  const maxSellImpactBps = params.maxSellImpactBps ?? Number(BUFFERED_SELL_IMPACT_CAP_BPS);
  if (maxSellImpactBps < 0 || maxSellImpactBps > Number(HARD_SELL_IMPACT_CAP_BPS)) {
    throw new RangeError(`maxSellImpactBps must be <= ${HARD_SELL_IMPACT_CAP_BPS}`);
  }

  const aBuy = assertU64(params.aBuy ?? 50_000n, "aBuy");
  const aSell = aBuy * 5n;

  const f0Bps = params.f0Bps ?? DEFAULT_F0_BPS;
  if (f0Bps !== DEFAULT_F0_BPS) {
    throw new RangeError(
      `Slot-0 genesis requires immutable 80% base buy fee (${DEFAULT_F0_BPS} bps), got ${f0Bps}`
    );
  }

  const fMinBps = params.fMinBps ?? DEFAULT_F_MIN_BPS;
  if (fMinBps > f0Bps) {
    throw new RangeError(`fMinBps (${fMinBps}) cannot exceed f0Bps (${f0Bps})`);
  }

  const fMaxSellBps = params.fMaxSellBps ?? IMMUTABLE_F_MAX_SELL_BPS;
  if (fMaxSellBps !== IMMUTABLE_F_MAX_SELL_BPS) {
    throw new RangeError(
      `Immutable 25% max sell fee (${IMMUTABLE_F_MAX_SELL_BPS} bps) cannot be overridden; got ${fMaxSellBps}`
    );
  }

  const compoundingFeeBps = params.compoundingFeeBps ?? IMMUTABLE_MIN_COMPOUNDING_FEE_BPS;
  if (
    compoundingFeeBps < IMMUTABLE_MIN_COMPOUNDING_FEE_BPS ||
    compoundingFeeBps > 10_000n
  ) {
    throw new RangeError(
      `Immutable minimum auto-compounding is ${IMMUTABLE_MIN_COMPOUNDING_FEE_BPS} bps (75%); got ${compoundingFeeBps}`
    );
  }

  const rentStep: RentStep = params.rentStep ?? "step2";
  const fundedRatchetPoolLamports =
    rentStep === "step2" ? RATCHET_POOL_RENT_STEP2_LAMPORTS : RATCHET_POOL_RENT_STEP5_LAMPORTS;
  const fundedActiveBinArrayLamports =
    rentStep === "step2"
      ? ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS
      : ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS;
  const fundedTotalLamports = fundedRatchetPoolLamports + fundedActiveBinArrayLamports;

  const rentAllocation: RentAllocationSummary = {
    activeRentStep: rentStep,
    ratchetPoolPdaDataBytes: RATCHET_POOL_PDA_DATA_BYTES,
    ratchetPoolPdaTotalBytes: RATCHET_POOL_PDA_TOTAL_BYTES,
    activeBinArrayPdaDataBytes: ACTIVE_BIN_ARRAY_PDA_DATA_BYTES,
    activeBinArrayPdaTotalBytes: ACTIVE_BIN_ARRAY_PDA_TOTAL_BYTES,
    step2: {
      ratchetPoolLamports: RATCHET_POOL_RENT_STEP2_LAMPORTS,
      activeBinArrayLamports: ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS,
      totalLamports: TOTAL_PDA_RENT_STEP2_LAMPORTS,
    },
    step5: {
      ratchetPoolLamports: RATCHET_POOL_RENT_STEP5_LAMPORTS,
      activeBinArrayLamports: ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS,
      totalLamports: TOTAL_PDA_RENT_STEP5_LAMPORTS,
    },
    fundedRatchetPoolLamports,
    fundedActiveBinArrayLamports,
    fundedTotalLamports,
  };

  const transferFeeFallbackConfig = createTransferFeeFallbackConfig({
    mint: baseTokenMint,
    transferFeeBasisPoints: params.fallbackTransferFeeBps ?? 0,
    maximumFeeAtoms: params.fallbackMaximumFeeAtoms ?? 0n,
    transferFeeConfigAuthority: null,
    withdrawWithheldAuthority,
  });

  const initialBins =
    params.initialBins ??
    synthesizeInitialBins(
      initialActiveBinId,
      floorBinIndex,
      initialSqrtPriceQ64,
      binStepBps,
      maxExtractableLamports,
      initialBaseSupplyAtoms
    );

  const seededQuoteTotal = initialBins.reduce((s, b) => s + b.quoteSolLamports, 0n);
  const seededBaseTotal = initialBins.reduce((s, b) => s + b.baseTokenAtoms, 0n);
  if (seededQuoteTotal > realVaultLamports) {
    throw new RangeError(
      `Seeded quote bin lamports (${seededQuoteTotal}) exceed realVaultLamports (${realVaultLamports})`
    );
  }
  if (seededBaseTotal === 0n) {
    throw new RangeError("Initial bins must seed non-zero base token liquidity");
  }

  const startSqrtPriceQ64 = initialBins[0]?.sqrtPriceQ64 ?? initialSqrtPriceQ64;
  const depositorBaseAccount = deriveDeterministicAddress("depositor_base", payer, baseTokenMint);
  const depositorQuoteAccount = deriveDeterministicAddress("depositor_quote", payer, WSOL_MINT_ADDRESS);

  const instructions: AtomicGenesisInstruction[] = [
    {
      step: 1,
      name: "CreateToken2022MintAccount",
      programId: SYSTEM_PROGRAM_ID,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: true },
        { pubkey: baseTokenMint, isSigner: true, isWritable: true },
      ],
      data: concatBytes(new Uint8Array([0, 0, 0, 0]), encodeU64LE(234n)),
      lamportsFunded: 2_500_000n,
    },
    {
      step: 2,
      name: "InitializeTransferFeeConfigFallback",
      programId: TOKEN_2022_PROGRAM_ID,
      accounts: [{ pubkey: baseTokenMint, isSigner: false, isWritable: true }],
      data: transferFeeFallbackConfig.serializedInitPayload,
      lamportsFunded: 0n,
    },
    {
      step: 3,
      name: "InitializeToken2022Mint2",
      programId: TOKEN_2022_PROGRAM_ID,
      accounts: [{ pubkey: baseTokenMint, isSigner: false, isWritable: true }],
      data: new Uint8Array([20, decimals, 0]),
      lamportsFunded: 0n,
    },
    {
      step: 4,
      name: "InitializePool",
      programId: ratchetProgramId,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: true },
        { pubkey: ratchetPoolPda, isSigner: false, isWritable: true },
        { pubkey: activeBinArrayPda, isSigner: false, isWritable: false },
        { pubkey: poolAuthorityPda, isSigner: false, isWritable: false },
        { pubkey: baseTokenMint, isSigner: false, isWritable: false },
        { pubkey: WSOL_MINT_ADDRESS, isSigner: false, isWritable: false },
        { pubkey: baseVault, isSigner: false, isWritable: true },
        { pubkey: quoteVault, isSigner: false, isWritable: true },
        { pubkey: creatorFeeVault, isSigner: false, isWritable: false },
        { pubkey: SYSTEM_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SPL_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: packInitializePoolIxData({
        sqrtPStart: initialSqrtPriceQ64,
        sqrtPGraduation: graduationSqrtPriceQ64,
        aBuy,
        binStepBps,
        activeBinId: initialActiveBinId,
        floorBinId: floorBinIndex,
        binArrayStartId: floorBinIndex,
        compoundingFeeBps: Number(compoundingFeeBps),
        gammaBps,
        maxSellImpactBps,
        transferFeeBps: params.fallbackTransferFeeBps ?? 0,
      }),
      lamportsFunded: fundedRatchetPoolLamports,
    },
    {
      step: 5,
      name: "InitializeBinArray",
      programId: ratchetProgramId,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: true },
        { pubkey: ratchetPoolPda, isSigner: false, isWritable: false },
        { pubkey: activeBinArrayPda, isSigner: false, isWritable: true },
        { pubkey: SYSTEM_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: packInitializeBinArrayIxData({
        startBinId: floorBinIndex,
        activeBinId: initialActiveBinId,
        floorBinId: floorBinIndex,
        binStepBps,
        startSqrtPriceQ64,
      }),
      lamportsFunded: fundedActiveBinArrayLamports,
    },
    {
      step: 6,
      name: "SeedInitialBins",
      programId: ratchetProgramId,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: false },
        { pubkey: ratchetPoolPda, isSigner: false, isWritable: true },
        { pubkey: activeBinArrayPda, isSigner: false, isWritable: true },
        { pubkey: baseTokenMint, isSigner: false, isWritable: false },
        { pubkey: baseVault, isSigner: false, isWritable: true },
        { pubkey: quoteVault, isSigner: false, isWritable: true },
        { pubkey: depositorBaseAccount, isSigner: false, isWritable: true },
        { pubkey: depositorQuoteAccount, isSigner: false, isWritable: true },
        { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SPL_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: packSeedInitialBinsIxData({
        quoteDepositLamports: realVaultLamports,
        baseDepositAtoms: seededBaseTotal,
        entries: initialBins,
      }),
      lamportsFunded: realVaultLamports,
    },
    {
      step: 7,
      name: "RevokeAndOpenSlot0",
      programId: ratchetProgramId,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: false },
        { pubkey: ratchetPoolPda, isSigner: false, isWritable: true },
        { pubkey: activeBinArrayPda, isSigner: false, isWritable: false },
        { pubkey: baseTokenMint, isSigner: false, isWritable: true },
        { pubkey: quoteVault, isSigner: false, isWritable: false },
        { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
      ],
      data: packRevokeAndOpenSlot0IxData(),
      lamportsFunded: 0n,
    },
    {
      step: 8,
      name: "OpenSlot0MarketCertificate",
      programId: ratchetProgramId,
      accounts: [
        { pubkey: payer, isSigner: true, isWritable: false },
        { pubkey: ratchetPoolPda, isSigner: false, isWritable: true },
        { pubkey: activeBinArrayPda, isSigner: false, isWritable: false },
      ],
      data: packSwapIxData({ isBuy: true, amountIn: 0n, minAmountOut: 0n }),
      lamportsFunded: 0n,
    },
  ];

  const bundleId = deriveDeterministicAddress(
    "atomic_genesis_bundle",
    baseTokenMint,
    ratchetPoolPda,
    activeBinArrayPda
  );

  return {
    bundleId,
    ratchetProgramId,
    baseTokenMint,
    quoteSolMint: WSOL_MINT_ADDRESS,
    ratchetPoolPda,
    activeBinArrayPda,
    poolAuthorityPda,
    baseVault,
    quoteVault,
    creatorFeeVault,
    decimals,
    binStepBps,
    initialActiveBinId,
    floorBinIndex,
    initialSqrtPriceQ64,
    graduationSqrtPriceQ64,
    aBuy,
    aSell,
    transferFeeFallbackConfig,
    rentAllocation,
    initialBins,
    policy: {
      authorityRevoked: true,
      mintAuthorityRevoked: true,
      freezeAuthorityRevoked: true,
      transferFeeConfigAuthorityRevoked: true,
      immutableMaxSellFeeBps: IMMUTABLE_F_MAX_SELL_BPS,
      immutableMinCompoundingFeeBps: IMMUTABLE_MIN_COMPOUNDING_FEE_BPS,
      configuredCompoundingFeeBps: compoundingFeeBps,
      slot0BaseBuyFeeBps: f0Bps,
      slot0Opened: true,
      solvencyReserveLamports: realVaultLamports,
      maxExtractableLamports,
      solvencyMarginLamports: realVaultLamports - maxExtractableLamports,
    },
    instructions,
  };
}

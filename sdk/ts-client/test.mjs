import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { Keypair, PublicKey } from "@solana/web3.js";
import {
  ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS,
  ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS,
  BUFFERED_SELL_IMPACT_CAP_BPS,
  DEFAULT_F0_BPS,
  DEFAULT_F_MIN_BPS,
  DEFAULT_RATCHET_PROGRAM_ID,
  EXACT_SWAP_ACCOUNT_COUNT,
  IMMUTABLE_F_MAX_SELL_BPS,
  IMMUTABLE_MIN_COMPOUNDING_FEE_BPS,
  IX_GRADUATE_IN_PLACE,
  IX_HARVEST_FALLBACK_TRANSFER_FEES,
  IX_INITIALIZE_BIN_ARRAY,
  IX_INITIALIZE_POOL,
  IX_REVOKE_AND_OPEN_SLOT0,
  IX_SEED_INITIAL_BINS,
  IX_SWAP,
  MAX_SOLANA_PACKET_BYTES,
  Q64_ONE,
  RATCHET_POOL_RENT_STEP2_LAMPORTS,
  RATCHET_POOL_RENT_STEP5_LAMPORTS,
  RATCHET_SWAP_CPI_DEPTH,
  SIMD0339_MAX_ACCOUNTS,
  TOKEN_2022_PROGRAM_ID,
  TOTAL_PDA_RENT_STEP2_LAMPORTS,
  TOTAL_PDA_RENT_STEP5_LAMPORTS,
  U128_MAX,
  VOLATILITY_DENOMINATOR,
  assertSellImpactWithinCap,
  buildAtomicGenesisBundle,
  buildRealGenesisTransactionPlan,
  computeStepsAboveSqrtPMax,
  createTransferFeeFallbackConfig,
  decodeActiveBinArrayPda,
  decodeBase58,
  decodeRatchetPoolPda,
  deriveDeterministicAddress,
  deriveRatchetPdas,
  divCeilU128,
  loadAndValidateLaunchConfig,
  packGraduateInPlaceIxData,
  packHarvestFallbackIxData,
  packInitializeBinArrayIxData,
  packInitializePoolIxData,
  packRevokeAndOpenSlot0IxData,
  packSeedInitialBinsIxData,
  packSwapIxData,
  previewAutoCompoundSol,
  previewBuyFeeBreakdown,
  previewBuyFeeBps,
  previewSellFeeBreakdown,
  previewSellFeeBps,
  resolveSwapAccounts28,
  updateHighWaterSqrtPrice,
} from "./dist/index.js";

// 0. Canonical PROGRAM_ID & Base58 roundtrip
assert.equal(DEFAULT_RATCHET_PROGRAM_ID, "5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5");
assert.equal(decodeBase58(DEFAULT_RATCHET_PROGRAM_ID).length, 32);

// 1. Exact u128 ceiling division checks
assert.equal(divCeilU128(0n), 0n);
assert.equal(divCeilU128(1n), 1n);
assert.equal(divCeilU128(VOLATILITY_DENOMINATOR), 1n);
assert.equal(divCeilU128(VOLATILITY_DENOMINATOR + 1n), 2n);
assert.throws(() => divCeilU128(U128_MAX + 1n), RangeError);

// 2. High-water buy ratchet & Slot-0 80% base fee
const slot0Buy = previewBuyFeeBps({
  aBuy: 50_000n,
  vAUp: 0n,
  binStepBps: 25n,
  stepsAbove: 0n,
});
assert.equal(slot0Buy, DEFAULT_F0_BPS);

const sniperBuy = previewBuyFeeBreakdown({
  aBuy: 50_000n,
  vAUp: 200n,
  binStepBps: 25n,
  stepsAbove: 0n,
});
assert.equal(sniperBuy.decayedBaseFeeBps, 8_000n);
assert.equal(sniperBuy.variableFeeBps, 13n);
assert.equal(sniperBuy.totalFeeBps, 8_013n);

const hwUp = updateHighWaterSqrtPrice(Q64_ONE, Q64_ONE * 4n);
assert.equal(hwUp, Q64_ONE * 4n);
const hwAfterDump = updateHighWaterSqrtPrice(hwUp, Q64_ONE * 2n);
assert.equal(hwAfterDump, hwUp);

const stepsFor4xSqrt = computeStepsAboveSqrtPMax(Q64_ONE * 4n, Q64_ONE, 25n);
assert.ok(stepsFor4xSqrt > 1000n);

const asymptoticBuy = previewBuyFeeBps({
  aBuy: 50_000n,
  vAUp: 0n,
  binStepBps: 25n,
  sqrtPGenesis: Q64_ONE,
  sqrtPHighWater: Q64_ONE * 4n,
});
assert.equal(asymptoticBuy, DEFAULT_F_MIN_BPS);

// 3. Asymmetric 5x Dump-Brake sell fee & immutable 25% cap + u128 overflow safety
const sellMild = previewSellFeeBreakdown({
  fBaseBps: 25n,
  aBuy: 50_000n,
  vADown: 200n,
  binStepBps: 25n,
});
assert.equal(sellMild.aSell, 250_000n);
assert.equal(sellMild.variableFeeBps, 63n);
assert.equal(sellMild.totalFeeBps, 88n);
assert.equal(sellMild.dumpBrakeCapped, false);

const sellExtreme = previewSellFeeBreakdown({
  fBaseBps: 100n,
  aBuy: 50_000n,
  vADown: 2_000n,
  binStepBps: 25n,
});
assert.equal(sellExtreme.totalFeeBps, IMMUTABLE_F_MAX_SELL_BPS);
assert.equal(sellExtreme.dumpBrakeCapped, true);

const safeHugeSell = previewSellFeeBps({
  fBaseBps: 25n,
  aBuy: 50_000n,
  vADown: 10n ** 18n,
  binStepBps: 25n,
});
assert.equal(safeHugeSell, IMMUTABLE_F_MAX_SELL_BPS);
assert.throws(
  () =>
    previewSellFeeBps({
      fBaseBps: 25n,
      aBuy: 50_000n,
      vADown: 10n ** 18n,
      binStepBps: 25n,
      strictU128Overflow: true,
    }),
  RangeError
);

// 4. 75% OnlyB quote auto-compounding & 0-lamport conservation drift
for (const gross of [10_000_000_000n, 1_234_567_891n, 99_999n, 1n]) {
  const ac = previewAutoCompoundSol({
    grossSolOutLamports: gross,
    sellFeeBps: 2_500n,
    compoundingFeeBps: IMMUTABLE_MIN_COMPOUNDING_FEE_BPS,
  });
  assert.equal(ac.conservationDriftLamports, 0n);
  assert.equal(
    ac.netSolOutLamports + ac.compoundedSolLamports + ac.remainingFeeLamports,
    gross
  );
}
const ac10Sol = previewAutoCompoundSol({
  grossSolOutLamports: 10_000_000_000n,
  sellFeeBps: 2_500n,
});
assert.equal(ac10Sol.totalSellFeeLamports, 2_500_000_000n);
assert.equal(ac10Sol.compoundedSolLamports, 1_875_000_000n);
assert.equal(ac10Sol.remainingFeeLamports, 625_000_000n);
assert.equal(ac10Sol.netSolOutLamports, 7_500_000_000n);

// 5. Gate G09 700 bps single-leg sell impact cap
assert.equal(assertSellImpactWithinCap(Q64_ONE, Q64_ONE), 0n);
assert.ok(
  assertSellImpactWithinCap(Q64_ONE * 100n, Q64_ONE * 97n) <= BUFFERED_SELL_IMPACT_CAP_BPS
);
assert.throws(() => assertSellImpactWithinCap(Q64_ONE * 100n, Q64_ONE * 90n), RangeError);

// 6. Real PDA derivation + 28-account swap instruction resolution
const sampleMint = Keypair.generate().publicKey.toBase58();
const realPdas = deriveRatchetPdas(sampleMint, -8, DEFAULT_RATCHET_PROGRAM_ID);
assert.equal(realPdas.programId, DEFAULT_RATCHET_PROGRAM_ID);
const [expectedPool] = PublicKey.findProgramAddressSync(
  [Buffer.from("ratchet_pool"), new PublicKey(sampleMint).toBuffer()],
  new PublicKey(DEFAULT_RATCHET_PROGRAM_ID)
);
assert.equal(realPdas.ratchetPoolPda, expectedPool.toBase58());

const swapAccounts = resolveSwapAccounts28({
  ratchetPoolPda: realPdas.ratchetPoolPda,
  activeBinArrayPda: realPdas.activeBinArrayPda,
  baseTokenMint: sampleMint,
  baseVault: realPdas.baseVault,
  quoteVault: realPdas.quoteVault,
  userTransferAuthority: deriveDeterministicAddress("user"),
  userBaseAccount: deriveDeterministicAddress("user_base"),
  userQuoteAccount: deriveDeterministicAddress("user_quote"),
  creatorFeeVault: deriveDeterministicAddress("creator"),
});
assert.equal(swapAccounts.accountCount, EXACT_SWAP_ACCOUNT_COUNT);
assert.equal(swapAccounts.accounts.length, 28);
assert.equal(swapAccounts.cpiDepth, RATCHET_SWAP_CPI_DEPTH);
assert.ok(swapAccounts.cpiDepth <= 4);
assert.ok(swapAccounts.accountCount <= SIMD0339_MAX_ACCOUNTS);
assert.equal(swapAccounts.withinExecutionLimits, true);
assert.ok(swapAccounts.accounts.some((a) => a.pubkey === TOKEN_2022_PROGRAM_ID));
assert.equal(swapAccounts.accounts[6].isSigner, true);
assert.equal(swapAccounts.accounts[21].pubkey, realPdas.poolAuthorityPda);

// 7. Wire packers match programs/ratchet-dlmm-damm/src/instruction.rs
assert.equal(
  packInitializePoolIxData({
    sqrtPStart: Q64_ONE,
    sqrtPGraduation: Q64_ONE * 2n,
    aBuy: 50_000n,
    binStepBps: 25,
    activeBinId: 0,
    floorBinId: -8,
    binArrayStartId: -8,
    compoundingFeeBps: 7500,
    gammaBps: 9000,
    maxSellImpactBps: 700,
    transferFeeBps: 0,
  }).length,
  64
);
assert.equal(packInitializePoolIxData({
  sqrtPStart: Q64_ONE,
  sqrtPGraduation: Q64_ONE * 2n,
  aBuy: 50_000n,
  binStepBps: 25,
  activeBinId: 0,
  floorBinId: -8,
  binArrayStartId: -8,
  compoundingFeeBps: 7500,
  gammaBps: 9000,
  maxSellImpactBps: 700,
  transferFeeBps: 0,
})[0], IX_INITIALIZE_POOL);
assert.equal(
  packInitializeBinArrayIxData({
    startBinId: -8,
    activeBinId: 0,
    floorBinId: -8,
    binStepBps: 25,
    startSqrtPriceQ64: Q64_ONE,
  }).length,
  31
);
assert.equal(packInitializeBinArrayIxData({
  startBinId: -8,
  activeBinId: 0,
  floorBinId: -8,
  binStepBps: 25,
  startSqrtPriceQ64: Q64_ONE,
})[0], IX_INITIALIZE_BIN_ARRAY);
const seedWire = packSeedInitialBinsIxData({
  quoteDepositLamports: 1_000n,
  baseDepositAtoms: 2_000n,
  entries: [
    { binId: 0, quoteSolLamports: 500n, baseTokenAtoms: 1_000n },
    { binId: 1, quoteSolLamports: 500n, baseTokenAtoms: 1_000n },
  ],
});
assert.equal(seedWire.length, 18 + 2 * 20);
assert.equal(seedWire[0], IX_SEED_INITIAL_BINS);
assert.equal(packRevokeAndOpenSlot0IxData()[0], IX_REVOKE_AND_OPEN_SLOT0);
const swapWire = packSwapIxData({ isBuy: true, amountIn: 100n, minAmountOut: 1n });
assert.equal(swapWire.length, 18);
assert.equal(swapWire[0], IX_SWAP);
assert.equal(packGraduateInPlaceIxData()[0], IX_GRADUATE_IN_PLACE);
assert.deepEqual(Array.from(packHarvestFallbackIxData(0)), [IX_HARVEST_FALLBACK_TRANSFER_FEES, 0]);

// 8. Token-2022 TransferFeeConfig fallback config
const tfConfig = createTransferFeeFallbackConfig({
  mint: sampleMint,
  withdrawWithheldAuthority: realPdas.poolAuthorityPda,
});
assert.equal(tfConfig.extensionType, "TransferFeeConfig");
assert.equal(tfConfig.writeLockFreeHarvesting, true);
assert.equal(tfConfig.transferFeeConfigAuthority, null);
assert.equal(tfConfig.serializedInitPayload[0], 26);

// 9. Atomic Genesis Bundle (Step 2 & Step 5 rent, solvency, authority revocation, Slot-0 80%)
const payer = deriveDeterministicAddress("genesis_payer");
const bundleStep2 = buildAtomicGenesisBundle({ payer, rentStep: "step2" });
assert.equal(bundleStep2.rentAllocation.step2.ratchetPoolLamports, RATCHET_POOL_RENT_STEP2_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step2.ratchetPoolLamports, 3_088_640n);
assert.equal(bundleStep2.rentAllocation.step2.activeBinArrayLamports, ACTIVE_BIN_ARRAY_RENT_STEP2_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step2.activeBinArrayLamports, 5_852_160n);
assert.equal(bundleStep2.rentAllocation.step2.totalLamports, TOTAL_PDA_RENT_STEP2_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step2.totalLamports, 8_940_800n);
assert.equal(bundleStep2.rentAllocation.fundedTotalLamports, 8_940_800n);

assert.equal(bundleStep2.rentAllocation.step5.ratchetPoolLamports, RATCHET_POOL_RENT_STEP5_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step5.ratchetPoolLamports, 423_168n);
assert.equal(bundleStep2.rentAllocation.step5.activeBinArrayLamports, ACTIVE_BIN_ARRAY_RENT_STEP5_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step5.activeBinArrayLamports, 801_792n);
assert.equal(bundleStep2.rentAllocation.step5.totalLamports, TOTAL_PDA_RENT_STEP5_LAMPORTS);
assert.equal(bundleStep2.rentAllocation.step5.totalLamports, 1_224_960n);

const bundleStep5 = buildAtomicGenesisBundle({ payer, rentStep: "step5" });
assert.equal(bundleStep5.rentAllocation.fundedRatchetPoolLamports, 423_168n);
assert.equal(bundleStep5.rentAllocation.fundedActiveBinArrayLamports, 801_792n);
assert.equal(bundleStep5.rentAllocation.fundedTotalLamports, 1_224_960n);

assert.equal(bundleStep2.policy.authorityRevoked, true);
assert.equal(bundleStep2.policy.mintAuthorityRevoked, true);
assert.equal(bundleStep2.policy.freezeAuthorityRevoked, true);
assert.equal(bundleStep2.policy.transferFeeConfigAuthorityRevoked, true);
assert.equal(bundleStep2.policy.immutableMaxSellFeeBps, 2_500n);
assert.equal(bundleStep2.policy.immutableMinCompoundingFeeBps, 7_500n);
assert.equal(bundleStep2.policy.slot0BaseBuyFeeBps, 8_000n);
assert.equal(bundleStep2.policy.slot0Opened, true);
assert.equal(bundleStep2.policy.solvencyReserveLamports, 86_500_000_000n);
assert.equal(bundleStep2.policy.maxExtractableLamports, 85_000_000_000n);
assert.equal(bundleStep2.instructions.length, 8);
assert.equal(bundleStep2.initialBins.length, 16);

assert.throws(() => buildAtomicGenesisBundle({ payer, fMaxSellBps: 3_000n }), RangeError);
assert.throws(() => buildAtomicGenesisBundle({ payer, compoundingFeeBps: 5_000n }), RangeError);
assert.throws(() => buildAtomicGenesisBundle({ payer, f0Bps: 1_000n }), RangeError);
assert.throws(
  () =>
    buildAtomicGenesisBundle({
      payer,
      realVaultLamports: 80_000_000_000n,
      maxExtractableLamports: 85_000_000_000n,
    }),
  RangeError
);

// 10. Real launch config validation & <= 1,232-byte genesis packet verification
const launchConfigPath = path.resolve("../../deploy/launch.config.json");
const exampleConfigPath = path.resolve("../../deploy/launch.config.example.json");
assert.ok(fs.existsSync(launchConfigPath));
assert.ok(fs.existsSync(exampleConfigPath));

// Placeholder branding in launch.config.json MUST fail without allowPlaceholder
assert.throws(() => loadAndValidateLaunchConfig(launchConfigPath, { allowPlaceholder: false }));
const { config: placeholderCfg, placeholderWarnings } = loadAndValidateLaunchConfig(
  launchConfigPath,
  { allowPlaceholder: true }
);
assert.ok(placeholderWarnings.length >= 3);

// Example config builds real Solana transactions all strictly <= 1,232 bytes
const { config: exampleCfg, placeholderWarnings: exWarn } = loadAndValidateLaunchConfig(
  exampleConfigPath,
  { allowPlaceholder: false }
);
assert.equal(exWarn.length, 0);

const realTxPlan = buildRealGenesisTransactionPlan({
  config: exampleCfg,
  payer: Keypair.generate().publicKey,
  mint: Keypair.generate().publicKey,
  mintRentLamports: 3_200_000n,
});
assert.equal(realTxPlan.allPacketsWithinLimit, true);
assert.ok(realTxPlan.maxPacketBytes <= MAX_SOLANA_PACKET_BYTES);
assert.ok(realTxPlan.packets.length >= 5);

// Also verify decodeRatchetPoolPda on a zero-initialized 480-byte buffer
const dummyPoolBuf = new Uint8Array(480);
const view = new DataView(dummyPoolBuf.buffer);
view.setUint16(176, 7500, true);
view.setUint16(178, 8000, true);
view.setUint16(182, 2500, true);
dummyPoolBuf[193] = 1;
const decodedPool = decodeRatchetPoolPda(dummyPoolBuf);
assert.equal(decodedPool.compoundingFeeBps, 7500);
assert.equal(decodedPool.f0Bps, 8000);
assert.equal(decodedPool.fMaxSellBps, 2500);
assert.equal(decodedPool.authorityRevoked, 1);

const dummyBinBuf = new Uint8Array(1024);
const binView = new DataView(dummyBinBuf.buffer);
binView.setInt32(48, 0, true);
binView.setInt32(52, -8, true);
binView.setInt32(56, -8, true);
binView.setUint16(60, 100, true);
binView.setBigUint64(64 + 16, 123_456n, true);
const decodedBins = decodeActiveBinArrayPda(dummyBinBuf);
assert.equal(decodedBins.bins.length, 30);
assert.equal(decodedBins.bins[0].binId, -8);
assert.equal(decodedBins.bins[0].reserveQuoteLamports, 123_456n);

console.log(
  JSON.stringify(
    {
      status: "PASS",
      suite: "@ratchet-dlmm-damm/ts-client",
      programId: placeholderCfg.paths.programId,
      maxGenesisPacketBytes: realTxPlan.maxPacketBytes,
      packetCount: realTxPlan.packets.length,
      checks: [
        "Canonical PROGRAM_ID + Base58 decode",
        "divCeilU128 + previewBuyFeeBps + previewSellFeeBps + previewAutoCompoundSol + assertSellImpactWithinCap",
        "deriveRatchetPdas (real PublicKey.findProgramAddressSync) + resolveSwapAccounts28",
        "0x01..0x07 binary wire packers matching instruction.rs",
        "buildAtomicGenesisBundle (Step 2 = 8,940,800 L / Step 5 = 1,224,960 L)",
        "buildRealGenesisTransactionPlan (Token-2022 TransferFeeConfig + MetadataPointer + TokenMetadata <= 1,232 bytes)",
        "Placeholder branding guardrail + RatchetPoolPda 480B decoder",
      ],
    },
    null,
    2
  )
);

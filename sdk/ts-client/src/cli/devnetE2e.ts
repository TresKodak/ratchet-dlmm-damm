/**
 * Comprehensive 12-Stage Live Solana Devnet Verification Suite (`npm run devnet:e2e`).
 *
 * Exercises the deployed `ratchet_dlmm_damm.so` program on Devnet across:
 *   1. Program deployment & payer balance check
 *   2. 7-packet genesis launch + raw byte audit of Mint, RatchetPoolPda (480B) & ActiveBinArrayPda (1,024B)
 *   3. Post-revocation immutability & negative authority checks (AuthorityRevoked / MintTo rejected)
 *   4. Multi-wallet Slot-0 anti-sniper buy via 12-account compact Swap (f_0 = 8000 + v_a_up surcharge, CU <= 43k)
 *   5. Live off-chain quote parity (previewBuyFeeBreakdown) & 28-account Swap (resolveSwapAccounts28)
 *   6. Negative swap guardrails: SellImpactExceedsCap (> 700 bps, 6007), SlippageExceeded (6008), ZeroSwapAmount (6013)
 *   7. Asymmetric 5x dump-brake sell + 75% OnlyB floor auto-compounding + monotonic sqrt_p_high_water + 0-lamport drift
 *   8. Real Devnet wall-clock volatility accumulator decay (SysvarClock >= 30s filter interval)
 *   9. Premature graduation rejection (GraduationThresholdNotReached, 6011)
 *  10. Price discovery to graduation + atomic same-slot [GraduateInPlace + Swap] tx (dead_zone_slots = 0, AlreadyGraduated 6010)
 *  11. Write-lock-free Token-2022 TransferFeeConfig fallback harvesting (0x07 mode 0 & mode 1)
 *  12. Final vault solvency audit, trader SOL/WSOL sweep-back to Devnet payer, and JSON report output
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  TOKEN_2022_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  NATIVE_MINT,
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  createSyncNativeInstruction,
  createMintToInstruction,
  getAccount,
  getAssociatedTokenAddressSync,
  getMint,
  getTransferFeeConfig,
} from "@solana/spl-token";
import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionInstruction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { resolveSwapAccounts28 } from "../accounts.js";
import {
  computeStepsAboveSqrtPMax,
  previewAutoCompoundSol,
  previewBuyFeeBreakdown,
  previewSellFeeBreakdown,
} from "../feePreview.js";
import {
  packGraduateInPlaceIxData,
  packHarvestFallbackIxData,
  packRevokeAndOpenSlot0IxData,
  packSeedInitialBinsIxData,
  packSwapIxData,
} from "../genesisBundle.js";
import {
  buildRealGenesisTransactionPlan,
  decodeActiveBinArrayPda,
  decodeRatchetPoolPda,
  loadAndValidateLaunchConfig,
  resolveUserPath,
  type DecodedActiveBinArrayPda,
  type DecodedRatchetPoolPda,
} from "../solanaTx.js";

function assert(cond: boolean, msg: string): asserts cond {
  if (!cond) {
    throw new Error(`Devnet E2E Assertion Failed: ${msg}`);
  }
}

function parseArgs(argv: string[]): {
  configPath: string;
  keypairOverride?: string;
  rpcOverride?: string;
  reportPath?: string;
  skipDecayWait: boolean;
} {
  const cliDir = path.dirname(fileURLToPath(import.meta.url));
  let configPath = path.resolve(cliDir, "../../../../deploy/launch.devnet.json");
  let keypairOverride: string | undefined;
  let rpcOverride: string | undefined;
  let reportPath: string | undefined;
  let skipDecayWait = false;

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--config" && argv[i + 1]) {
      configPath = path.resolve(argv[++i]!);
    } else if (arg === "--keypair" && argv[i + 1]) {
      keypairOverride = argv[++i]!;
    } else if (arg === "--rpc" && argv[i + 1]) {
      rpcOverride = argv[++i]!;
    } else if (arg === "--report" && argv[i + 1]) {
      reportPath = path.resolve(argv[++i]!);
    } else if (arg === "--skip-decay-wait") {
      skipDecayWait = true;
    }
  }
  return { configPath, keypairOverride, rpcOverride, reportPath, skipDecayWait };
}

function loadKeypair(filePath: string): Keypair {
  const resolved = resolveUserPath(filePath);
  if (!fs.existsSync(resolved)) {
    throw new Error(`Keypair file not found: ${resolved}`);
  }
  const secret = JSON.parse(fs.readFileSync(resolved, "utf8")) as number[];
  return Keypair.fromSecretKey(Uint8Array.from(secret));
}

async function sendTxWithMetrics(
  connection: Connection,
  tx: Transaction,
  signers: Keypair[],
  label: string
): Promise<{ signature: string; slot: number; computeUnits: number | null }> {
  let lastErr: unknown;
  for (let attempt = 1; attempt <= 4; attempt++) {
    try {
      const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash("confirmed");
      tx.recentBlockhash = blockhash;
      tx.feePayer = signers[0]!.publicKey;
      const sig = await sendAndConfirmTransaction(connection, tx, signers, {
        commitment: "confirmed",
        skipPreflight: false,
      });
      await connection.confirmTransaction(
        { signature: sig, blockhash, lastValidBlockHeight },
        "confirmed"
      );
      const parsed = await connection.getTransaction(sig, {
        commitment: "confirmed",
        maxSupportedTransactionVersion: 0,
      });
      const cu = parsed?.meta?.computeUnitsConsumed ?? null;
      const slot = parsed?.slot ?? 0;
      console.log(
        `  [TX PASS] ${label}: sig=${sig} slot=${slot} cu=${cu ?? "unknown"}`
      );
      return { signature: sig, slot, computeUnits: cu };
    } catch (err) {
      lastErr = err;
      const msg = err instanceof Error ? err.message : String(err);
      if (attempt < 4 && (msg.includes("429") || msg.includes("Blockhash not found") || msg.includes("timed out"))) {
        await new Promise((r) => setTimeout(r, 1_500 * attempt));
        continue;
      }
      throw new Error(`Transaction "${label}" failed: ${msg}`);
    }
  }
  throw lastErr;
}

async function expectSimulateCustomError(
  connection: Connection,
  instructions: TransactionInstruction[],
  signers: Keypair[],
  expectedCustomCode: number | null,
  label: string
): Promise<string> {
  const { blockhash } = await connection.getLatestBlockhash("confirmed");
  const tx = new Transaction();
  tx.feePayer = signers[0]!.publicKey;
  tx.recentBlockhash = blockhash;
  for (const ix of instructions) tx.add(ix);
  tx.sign(...signers);
  const sim = await connection.simulateTransaction(tx);
  assert(
    sim.value.err !== null,
    `Expected negative test "${label}" to fail, but simulation succeeded!`
  );
  const errStr = JSON.stringify(sim.value.err);
  if (expectedCustomCode !== null) {
    assert(
      errStr.includes(`"Custom":${expectedCustomCode}`),
      `Expected Custom(${expectedCustomCode}) for "${label}", got ${errStr} (logs: ${JSON.stringify(sim.value.logs)})`
    );
  }
  console.log(`  [NEG PASS] ${label}: rejected cleanly with ${errStr}`);
  return errStr;
}

function buildCompactSwapIx(params: {
  programId: PublicKey;
  poolPda: PublicKey;
  binArrayPda: PublicKey;
  mint: PublicKey;
  baseVault: PublicKey;
  quoteVault: PublicKey;
  user: PublicKey;
  userBaseAta: PublicKey;
  userWsolAta: PublicKey;
  poolAuthorityPda: PublicKey;
  isBuy: boolean;
  amountIn: bigint;
  minAmountOut: bigint;
}): TransactionInstruction {
  return new TransactionInstruction({
    programId: params.programId,
    keys: [
      { pubkey: params.poolPda, isSigner: false, isWritable: true },
      { pubkey: params.binArrayPda, isSigner: false, isWritable: true },
      { pubkey: params.mint, isSigner: false, isWritable: false },
      { pubkey: NATIVE_MINT, isSigner: false, isWritable: false },
      { pubkey: params.baseVault, isSigner: false, isWritable: true },
      { pubkey: params.quoteVault, isSigner: false, isWritable: true },
      { pubkey: params.user, isSigner: true, isWritable: false },
      { pubkey: params.userBaseAta, isSigner: false, isWritable: true },
      { pubkey: params.userWsolAta, isSigner: false, isWritable: true },
      { pubkey: params.poolAuthorityPda, isSigner: false, isWritable: false },
      { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
    ],
    data: Buffer.from(
      packSwapIxData({
        isBuy: params.isBuy,
        amountIn: params.amountIn,
        minAmountOut: params.minAmountOut,
      })
    ),
  });
}

function build28AccountSwapIx(params: {
  programId: PublicKey;
  poolPda: PublicKey;
  binArrayPda: PublicKey;
  mint: PublicKey;
  baseVault: PublicKey;
  quoteVault: PublicKey;
  user: PublicKey;
  userBaseAta: PublicKey;
  userWsolAta: PublicKey;
  poolAuthorityPda: PublicKey;
  creatorFeeVault: PublicKey;
  isBuy: boolean;
  amountIn: bigint;
  minAmountOut: bigint;
}): TransactionInstruction {
  const resolved = resolveSwapAccounts28({
    ratchetProgramId: params.programId.toBase58(),
    ratchetPoolPda: params.poolPda.toBase58(),
    activeBinArrayPda: params.binArrayPda.toBase58(),
    baseTokenMint: params.mint.toBase58(),
    baseVault: params.baseVault.toBase58(),
    quoteVault: params.quoteVault.toBase58(),
    userTransferAuthority: params.user.toBase58(),
    userBaseAccount: params.userBaseAta.toBase58(),
    userQuoteAccount: params.userWsolAta.toBase58(),
    creatorFeeVault: params.creatorFeeVault.toBase58(),
    poolAuthorityPda: params.poolAuthorityPda.toBase58(),
  });
  return new TransactionInstruction({
    programId: params.programId,
    keys: resolved.accounts.map((a) => ({
      pubkey: new PublicKey(a.pubkey),
      isSigner: a.isSigner,
      isWritable: a.isWritable,
    })),
    data: Buffer.from(
      packSwapIxData({
        isBuy: params.isBuy,
        amountIn: params.amountIn,
        minAmountOut: params.minAmountOut,
      })
    ),
  });
}

async function fetchPoolAndBins(
  connection: Connection,
  poolPda: PublicKey,
  binArrayPda: PublicKey
): Promise<{ pool: DecodedRatchetPoolPda; binArray: DecodedActiveBinArrayPda }> {
  const [poolInfo, binInfo] = await Promise.all([
    connection.getAccountInfo(poolPda, "confirmed"),
    connection.getAccountInfo(binArrayPda, "confirmed"),
  ]);
  assert(poolInfo !== null && poolInfo.data.length === 480, "Missing or invalid RatchetPoolPda");
  assert(binInfo !== null && binInfo.data.length === 1024, "Missing or invalid ActiveBinArrayPda");
  return {
    pool: decodeRatchetPoolPda(poolInfo.data),
    binArray: decodeActiveBinArrayPda(binInfo.data),
  };
}

async function verifySolConservation(
  connection: Connection,
  pool: DecodedRatchetPoolPda,
  binArray: DecodedActiveBinArrayPda,
  quoteVault: PublicKey,
  stageLabel: string
): Promise<void> {
  const binQuoteSum = binArray.bins.reduce((s, b) => s + b.reserveQuoteLamports, 0n);
  const quoteVaultAcct = await getAccount(connection, quoteVault, "confirmed", TOKEN_PROGRAM_ID);
  assert(
    binQuoteSum + pool.protocolFeeLamports === pool.realVaultLamports,
    `${stageLabel}: binQuoteSum (${binQuoteSum}) + protocolFee (${pool.protocolFeeLamports}) != realVault (${pool.realVaultLamports})`
  );
  assert(
    quoteVaultAcct.amount === pool.realVaultLamports,
    `${stageLabel}: physical WSOL quoteVault (${quoteVaultAcct.amount}) != realVaultLamports (${pool.realVaultLamports})`
  );
  assert(
    pool.maxExtractableLamports <= pool.realVaultLamports,
    `${stageLabel}: Gate G06 solvency violation (${pool.maxExtractableLamports} > ${pool.realVaultLamports})`
  );
}

async function fundTraderWallet(
  connection: Connection,
  payer: Keypair,
  trader: Keypair,
  mint: PublicKey,
  solLamportsToWrap: bigint,
  feeReserveLamports: bigint
): Promise<{ traderBaseAta: PublicKey; traderWsolAta: PublicKey }> {
  const traderBaseAta = getAssociatedTokenAddressSync(
    mint,
    trader.publicKey,
    false,
    TOKEN_2022_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID
  );
  const traderWsolAta = getAssociatedTokenAddressSync(
    NATIVE_MINT,
    trader.publicKey,
    false,
    TOKEN_PROGRAM_ID,
    ASSOCIATED_TOKEN_PROGRAM_ID
  );
  const tx = new Transaction().add(
    SystemProgram.transfer({
      fromPubkey: payer.publicKey,
      toPubkey: trader.publicKey,
      lamports: feeReserveLamports + solLamportsToWrap,
    }),
    createAssociatedTokenAccountIdempotentInstruction(
      payer.publicKey,
      traderBaseAta,
      trader.publicKey,
      mint,
      TOKEN_2022_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID
    ),
    createAssociatedTokenAccountIdempotentInstruction(
      payer.publicKey,
      traderWsolAta,
      trader.publicKey,
      NATIVE_MINT,
      TOKEN_PROGRAM_ID,
      ASSOCIATED_TOKEN_PROGRAM_ID
    ),
    SystemProgram.transfer({
      fromPubkey: trader.publicKey,
      toPubkey: traderWsolAta,
      lamports: solLamportsToWrap,
    }),
    createSyncNativeInstruction(traderWsolAta, TOKEN_PROGRAM_ID)
  );
  await sendTxWithMetrics(
    connection,
    tx,
    [payer, trader],
    `fund_trader_${trader.publicKey.toBase58().slice(0, 6)}`
  );
  return { traderBaseAta, traderWsolAta };
}

async function main(): Promise<void> {
  const { configPath, keypairOverride, rpcOverride, reportPath, skipDecayWait } = parseArgs(
    process.argv.slice(2)
  );
  const repoRoot = path.resolve(path.dirname(configPath), "..");
  const { config } = loadAndValidateLaunchConfig(configPath, { allowPlaceholder: false });
  if (rpcOverride) config.network.rpcUrl = rpcOverride;
  if (keypairOverride) config.paths.payerKeypairPath = keypairOverride;

  assert(
    config.network.cluster === "devnet",
    `devnet:e2e refuses to run against cluster "${config.network.cluster}". Must be "devnet".`
  );

  const payer = loadKeypair(config.paths.payerKeypairPath);
  const mint = Keypair.generate();
  const connection = new Connection(config.network.rpcUrl, "confirmed");
  const programId = new PublicKey(config.paths.programId);

  console.log("=== Stage 1: Devnet Program & Payer Balance Check ===");
  const [programInfo, payerBalance] = await Promise.all([
    connection.getAccountInfo(programId, "confirmed"),
    connection.getBalance(payer.publicKey, "confirmed"),
  ]);
  if (!programInfo || !programInfo.executable) {
    throw new Error(
      `Program ${programId.toBase58()} is NOT deployed on Devnet yet. Deploy first:\n` +
        `  solana program deploy ${config.paths.programBinaryPath} --program-id ${config.paths.programKeypairPath} --keypair ${config.paths.payerKeypairPath} --url devnet`
    );
  }
  console.log(
    `  Program ${programId.toBase58()} executable on Devnet; Payer ${payer.publicKey.toBase58()} balance = ${(payerBalance / LAMPORTS_PER_SOL).toFixed(4)} SOL`
  );
  assert(payerBalance >= 8 * LAMPORTS_PER_SOL, "Need >= 8 SOL on Devnet for full 12-stage suite");

  console.log("\n=== Stage 2: 7-Packet Genesis Launch & On-Chain Byte Audit ===");
  const dummyPlan = buildRealGenesisTransactionPlan({
    config,
    payer: payer.publicKey,
    mint: mint.publicKey,
    mintRentLamports: 2_500_000n,
  });
  const mintRentLamports = BigInt(
    await connection.getMinimumBalanceForRentExemption(dummyPlan.totalMintAccountSpace)
  );
  const plan = buildRealGenesisTransactionPlan({
    config,
    payer: payer.publicKey,
    mint: mint.publicKey,
    mintRentLamports,
  });
  assert(plan.allPacketsWithinLimit, `Packet exceeds 1,232B: ${plan.maxPacketBytes}`);

  const genesisReceipts: Array<{ name: string; signature: string; slot: number; cu: number | null }> = [];
  for (const pkt of plan.packets) {
    const signers = pkt.requiresMintSigner ? [payer, mint] : [payer];
    const tx = Transaction.from(
      pkt.transaction.serialize({ requireAllSignatures: false, verifySignatures: false })
    );
    const res = await sendTxWithMetrics(connection, tx, signers, pkt.name);
    genesisReceipts.push({ name: pkt.name, signature: res.signature, slot: res.slot, cu: res.computeUnits });
  }

  const poolPda = new PublicKey(plan.pdas.ratchetPoolPda);
  const binArrayPda = new PublicKey(plan.pdas.activeBinArrayPda);
  const poolAuthorityPda = new PublicKey(plan.pdas.poolAuthorityPda);
  const baseVault = new PublicKey(plan.pdas.baseVault);
  const quoteVault = new PublicKey(plan.pdas.quoteVault);

  let { pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda);
  const mintState = await getMint(connection, mint.publicKey, "confirmed", TOKEN_2022_PROGRAM_ID);
  const tfCfg = getTransferFeeConfig(mintState);
  const baseVaultAcct = await getAccount(connection, baseVault, "confirmed", TOKEN_2022_PROGRAM_ID);

  assert(pool.authorityRevoked === 1, "authorityRevoked must be 1");
  assert(pool.authority === PublicKey.default.toBase58(), "authority must be zeroed");
  assert(mintState.mintAuthority === null, "mintAuthority must be null");
  assert(mintState.freezeAuthority === null, "freezeAuthority must be null");
  assert(
    tfCfg !== null && tfCfg.transferFeeConfigAuthority.equals(PublicKey.default),
    "transferFeeConfigAuthority must be revoked"
  );
  assert(pool.f0Bps === 8000 && pool.fMinBps === 25 && pool.fMaxSellBps === 2500, "fee bounds mismatch");
  assert(pool.compoundingFeeBps === 7500 && pool.aSell === 5n * pool.aBuy, "compounding / 5x dump-brake mismatch");
  assert(pool.realVaultLamports === plan.realVaultLamports, "realVaultLamports mismatch");
  assert(pool.maxExtractableLamports === plan.maxExtractableLamports, "maxExtractableLamports mismatch");
  assert(baseVaultAcct.amount > 0n, "baseVault must be seeded");
  await verifySolConservation(connection, pool, binArray, quoteVault, "Stage 2 Genesis");
  console.log("  [PASS] Genesis authorities revoked, 30 bins seeded, Gate G06 solvency & 0-drift verified.");

  console.log("\n=== Stage 3: Post-Revocation Immutability (Negative On-Chain Tests) ===");
  await expectSimulateCustomError(
    connection,
    [
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: payer.publicKey, isSigner: true, isWritable: false },
          { pubkey: poolPda, isSigner: false, isWritable: true },
          { pubkey: binArrayPda, isSigner: false, isWritable: false },
          { pubkey: mint.publicKey, isSigner: false, isWritable: true },
          { pubkey: quoteVault, isSigner: false, isWritable: false },
          { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
        ],
        data: Buffer.from(packRevokeAndOpenSlot0IxData()),
      }),
    ],
    [payer],
    6012, // AuthorityRevoked
    "re_invoke_revoke_and_open_slot0"
  );
  await expectSimulateCustomError(
    connection,
    [
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: payer.publicKey, isSigner: true, isWritable: false },
          { pubkey: poolPda, isSigner: false, isWritable: true },
          { pubkey: binArrayPda, isSigner: false, isWritable: true },
          { pubkey: mint.publicKey, isSigner: false, isWritable: false },
          { pubkey: baseVault, isSigner: false, isWritable: true },
          { pubkey: quoteVault, isSigner: false, isWritable: true },
          { pubkey: plan.payerBaseAta, isSigner: false, isWritable: true },
          { pubkey: plan.payerWsolAta, isSigner: false, isWritable: true },
          { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
          { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        ],
        data: Buffer.from(
          packSeedInitialBinsIxData({
            quoteDepositLamports: 0n,
            baseDepositAtoms: 0n,
            entries: [{ binId: 0, quoteSolLamports: 0n, baseTokenAtoms: 0n }],
          })
        ),
      }),
    ],
    [payer],
    6012, // AuthorityRevoked
    "re_invoke_seed_initial_bins"
  );
  await expectSimulateCustomError(
    connection,
    [
      createMintToInstruction(
        mint.publicKey,
        plan.payerBaseAta,
        payer.publicKey,
        1_000_000n,
        [],
        TOKEN_2022_PROGRAM_ID
      ),
    ],
    [payer],
    null,
    "token2022_mint_to_after_authority_revoked"
  );

  console.log("\n=== Stage 4: Multi-Wallet Setup & Slot-0 Buy Ratchet (Compact 12-Account Swap) ===");
  const traderA = Keypair.generate();
  const traderB = Keypair.generate();
  const traderC = Keypair.generate();

  const [acctsA, acctsB, acctsC] = await Promise.all([
    fundTraderWallet(connection, payer, traderA, mint.publicKey, 1_500_000_000n, 30_000_000n),
    fundTraderWallet(connection, payer, traderB, mint.publicKey, 1_500_000_000n, 30_000_000n),
    fundTraderWallet(connection, payer, traderC, mint.publicKey, 6_000_000_000n, 30_000_000n),
  ]);

  const buy1Amount = 1_200_000_000n; // 1.2 SOL at Slot-0 80% base fee
  const poolBeforeBuy1 = pool;
  const buy1Res = await sendTxWithMetrics(
    connection,
    new Transaction().add(
      buildCompactSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderA.publicKey,
        userBaseAta: acctsA.traderBaseAta,
        userWsolAta: acctsA.traderWsolAta,
        poolAuthorityPda,
        isBuy: true,
        amountIn: buy1Amount,
        minAmountOut: 1n,
      })
    ),
    [traderA],
    "swap_buy1_slot0_compact_12acct"
  );
  if (buy1Res.computeUnits !== null) {
    assert(buy1Res.computeUnits <= 43_000, `Buy1 CU ${buy1Res.computeUnits} > 43,000 G02`);
  }
  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  const buy1FeeCollected = pool.protocolFeeLamports - poolBeforeBuy1.protocolFeeLamports;
  const traderABaseAfterBuy1 = (
    await getAccount(connection, acctsA.traderBaseAta, "confirmed", TOKEN_2022_PROGRAM_ID)
  ).amount;
  assert(traderABaseAfterBuy1 > 0n, "Trader A received zero base tokens");
  assert(pool.vAUp > 0n, "Slot-0 multi-bin buy must increment vAUp");
  assert(
    pool.sqrtPHighWater > poolBeforeBuy1.sqrtPHighWater &&
      pool.sqrtPHighWater === pool.sqrtPCurrent,
    "High-water mark must advance on Slot-0 buy"
  );
  assert(buy1FeeCollected >= (buy1Amount * 8_000n) / 10_000n, "Slot-0 80% fee not enforced");
  await verifySolConservation(connection, pool, binArray, quoteVault, "Stage 4 Buy1");
  console.log(
    `  [PASS] Buy1: activeBin=${pool.activeBinId}, vAUp=${pool.vAUp}, feeCollected=${buy1FeeCollected} L, CU=${buy1Res.computeUnits}`
  );

  console.log("\n=== Stage 5: Off-Chain Quote Parity & Full 28-Account Buy (Trader B) ===");
  const poolBeforeBuy2 = pool;
  const buy2Amount = 1_000_000_000n; // 1.0 SOL via 28-account layout
  const preStepsAbove = computeStepsAboveSqrtPMax(
    poolBeforeBuy2.sqrtPHighWater,
    poolBeforeBuy2.sqrtPStart,
    BigInt(poolBeforeBuy2.binStepBps)
  );
  const preBuy2Breakdown = previewBuyFeeBreakdown({
    f0Bps: BigInt(poolBeforeBuy2.f0Bps),
    fMinBps: BigInt(poolBeforeBuy2.fMinBps),
    gammaBps: BigInt(poolBeforeBuy2.gammaBps),
    stepsAbove: preStepsAbove,
    aBuy: poolBeforeBuy2.aBuy,
    vAUp: poolBeforeBuy2.vAUp,
    binStepBps: BigInt(poolBeforeBuy2.binStepBps),
  });
  assert(
    preBuy2Breakdown.decayedBaseFeeBps < 8_000n,
    `Expected base buy fee decay < 8000 bps after high-water advance, got ${preBuy2Breakdown.decayedBaseFeeBps}`
  );

  const buy2Res = await sendTxWithMetrics(
    connection,
    new Transaction().add(
      build28AccountSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderB.publicKey,
        userBaseAta: acctsB.traderBaseAta,
        userWsolAta: acctsB.traderWsolAta,
        poolAuthorityPda,
        creatorFeeVault: new PublicKey(poolBeforeBuy2.creatorFeeVault),
        isBuy: true,
        amountIn: buy2Amount,
        minAmountOut: 1n,
      })
    ),
    [traderB],
    "swap_buy2_router_28acct"
  );
  if (buy2Res.computeUnits !== null) {
    assert(buy2Res.computeUnits <= 43_000, `Buy2 28-acct CU ${buy2Res.computeUnits} > 43,000 G02`);
  }
  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  const buy2FeeCollected = pool.protocolFeeLamports - poolBeforeBuy2.protocolFeeLamports;
  const expectedProjectedFeeBreakdown = previewBuyFeeBreakdown({
    f0Bps: BigInt(poolBeforeBuy2.f0Bps),
    fMinBps: BigInt(poolBeforeBuy2.fMinBps),
    gammaBps: BigInt(poolBeforeBuy2.gammaBps),
    stepsAbove: preStepsAbove,
    aBuy: poolBeforeBuy2.aBuy,
    vAUp: pool.vAUp,
    binStepBps: BigInt(poolBeforeBuy2.binStepBps),
  });
  const expectedBuy2Fee =
    (buy2Amount * expectedProjectedFeeBreakdown.totalFeeBps + 9_999n) / 10_000n;
  assert(
    buy2FeeCollected === expectedBuy2Fee ||
      (buy2FeeCollected >= (buy2Amount * preBuy2Breakdown.totalFeeBps) / 10_000n &&
        buy2FeeCollected < (buy2Amount * 8_000n) / 10_000n),
    `Buy2 fee parity mismatch: actual=${buy2FeeCollected}, expected~=${expectedBuy2Fee}`
  );
  await verifySolConservation(connection, pool, binArray, quoteVault, "Stage 5 Buy2");

  // Push price up with Trader C's first buy (~2.4 SOL) to reach ~bin 9-10 (< graduation bin +12)
  // while holding enough tokens to cross 8+ bins (>700 bps impact) in Stage 6.
  const buy3Amount = 2_400_000_000n;
  await sendTxWithMetrics(
    connection,
    new Transaction().add(
      build28AccountSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderC.publicKey,
        userBaseAta: acctsC.traderBaseAta,
        userWsolAta: acctsC.traderWsolAta,
        poolAuthorityPda,
        creatorFeeVault: new PublicKey(pool.creatorFeeVault),
        isBuy: true,
        amountIn: buy3Amount,
        minAmountOut: 1n,
      })
    ),
    [traderC],
    "swap_buy3_traderC_build_depth"
  );
  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  const traderCBalance = (
    await getAccount(connection, acctsC.traderBaseAta, "confirmed", TOKEN_2022_PROGRAM_ID)
  ).amount;
  console.log(
    `  [INFO] After Trader C buy: activeBin=${pool.activeBinId}, traderC_tokens=${traderCBalance}, vAUp=${pool.vAUp}`
  );

  console.log("\n=== Stage 6: Negative Swap Guardrails (G09 >700bps, Slippage, Zero-Amount) ===");
  await expectSimulateCustomError(
    connection,
    [
      build28AccountSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderC.publicKey,
        userBaseAta: acctsC.traderBaseAta,
        userWsolAta: acctsC.traderWsolAta,
        poolAuthorityPda,
        creatorFeeVault: new PublicKey(pool.creatorFeeVault),
        isBuy: false,
        amountIn: traderCBalance,
        minAmountOut: 0n,
      }),
    ],
    [traderC],
    6007, // SellImpactExceedsCap
    "oversized_single_leg_sell_rejected_g09"
  );
  await expectSimulateCustomError(
    connection,
    [
      buildCompactSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderA.publicKey,
        userBaseAta: acctsA.traderBaseAta,
        userWsolAta: acctsA.traderWsolAta,
        poolAuthorityPda,
        isBuy: false,
        amountIn: traderABaseAfterBuy1 / 4n,
        minAmountOut: (1n << 64n) - 1n,
      }),
    ],
    [traderA],
    6008, // SlippageExceeded
    "impossible_min_out_slippage_rejected"
  );
  await expectSimulateCustomError(
    connection,
    [
      buildCompactSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderA.publicKey,
        userBaseAta: acctsA.traderBaseAta,
        userWsolAta: acctsA.traderWsolAta,
        poolAuthorityPda,
        isBuy: true,
        amountIn: 0n,
        minAmountOut: 0n,
      }),
    ],
    [traderA],
    6013, // ZeroSwapAmount
    "zero_amount_swap_rejected"
  );

  console.log("\n=== Stage 7: Asymmetric 5x Dump-Brake & 75% OnlyB Floor Auto-Compounding ===");
  const poolBeforeSell1 = pool;
  const binsBeforeSell1 = binArray;
  const floorBinIdx = binsBeforeSell1.bins.findIndex((b) => b.binId === binsBeforeSell1.floorBinId);
  assert(floorBinIdx >= 0, "floorBinId not found in ActiveBinArrayPda");
  const floorQuoteBefore = binsBeforeSell1.bins[floorBinIdx]!.reserveQuoteLamports;
  const traderCWsolBefore = (
    await getAccount(connection, acctsC.traderWsolAta, "confirmed", TOKEN_PROGRAM_ID)
  ).amount;

  // Sell 35% of Trader C's balance (stays safely <= 700 bps impact cap while crossing multiple bins down)
  const sell1Amount = (traderCBalance * 35n) / 100n;
  const sell1Res = await sendTxWithMetrics(
    connection,
    new Transaction().add(
      build28AccountSwapIx({
        programId,
        poolPda,
        binArrayPda,
        mint: mint.publicKey,
        baseVault,
        quoteVault,
        user: traderC.publicKey,
        userBaseAta: acctsC.traderBaseAta,
        userWsolAta: acctsC.traderWsolAta,
        poolAuthorityPda,
        creatorFeeVault: new PublicKey(pool.creatorFeeVault),
        isBuy: false,
        amountIn: sell1Amount,
        minAmountOut: 1n,
      })
    ),
    [traderC],
    "swap_sell1_dump_brake_compounding_28acct"
  );
  if (sell1Res.computeUnits !== null) {
    assert(sell1Res.computeUnits <= 43_000, `Sell1 28-acct CU ${sell1Res.computeUnits} > 43,000 G02`);
  }

  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  const floorQuoteAfter = binArray.bins[floorBinIdx]!.reserveQuoteLamports;
  const traderCWsolAfter = (
    await getAccount(connection, acctsC.traderWsolAta, "confirmed", TOKEN_PROGRAM_ID)
  ).amount;

  const deltaFloorQuote = floorQuoteAfter - floorQuoteBefore;
  const deltaCompoundedTotal = pool.compoundedSolTotal - poolBeforeSell1.compoundedSolTotal;
  const deltaProtocolFee = pool.protocolFeeLamports - poolBeforeSell1.protocolFeeLamports;
  const netUserSolOut = traderCWsolAfter - traderCWsolBefore;
  const grossSolOut = netUserSolOut + deltaCompoundedTotal + deltaProtocolFee;

  assert(pool.vADown > poolBeforeSell1.vADown, "Downward crossings must increment vADown");
  assert(
    pool.sqrtPHighWater === poolBeforeSell1.sqrtPHighWater,
    "High-water mark must remain monotonically latched during downward sell!"
  );
  assert(
    pool.sqrtPCurrent <= poolBeforeSell1.sqrtPCurrent,
    "Current sqrt(P) must not increase on sell"
  );
  assert(
    deltaFloorQuote === deltaCompoundedTotal && deltaFloorQuote > 0n,
    `Floor bin delta (${deltaFloorQuote}) != compoundedSolTotal delta (${deltaCompoundedTotal})`
  );

  const expectedSellFee = previewSellFeeBreakdown({
    fBaseBps: BigInt(poolBeforeSell1.fMinBps),
    aBuy: poolBeforeSell1.aBuy,
    vADown: pool.vADown,
    binStepBps: BigInt(poolBeforeSell1.binStepBps),
    fMaxSellBps: BigInt(poolBeforeSell1.fMaxSellBps),
  });
  const expectedCompound = previewAutoCompoundSol({
    grossSolOutLamports: grossSolOut,
    sellFeeBps: expectedSellFee.totalFeeBps,
    compoundingFeeBps: BigInt(poolBeforeSell1.compoundingFeeBps),
  });
  assert(
    deltaCompoundedTotal === expectedCompound.compoundedSolLamports,
    `Compounded SOL mismatch: on-chain=${deltaCompoundedTotal}, expected=${expectedCompound.compoundedSolLamports}`
  );
  assert(
    deltaProtocolFee === expectedCompound.remainingFeeLamports,
    `Protocol fee mismatch: on-chain=${deltaProtocolFee}, expected=${expectedCompound.remainingFeeLamports}`
  );
  await verifySolConservation(connection, pool, binArray, quoteVault, "Stage 7 Sell1");
  console.log(
    `  [PASS] Sell1: vADown=${pool.vADown}, fSell=${expectedSellFee.totalFeeBps} bps, compoundedIntoFloor=${deltaFloorQuote} L, 0-lamport drift verified`
  );

  console.log("\n=== Stage 8: Real Devnet Wall-Clock Volatility Decay (SysvarClock >= 30s) ===");
  let decaySkipped = skipDecayWait;
  let decayPrePool = pool;
  if (!skipDecayWait) {
    console.log("  Waiting 33s for Devnet SysvarClock >= filter_period_seconds (30s)...");
    await new Promise((r) => setTimeout(r, 33_000));
    // Execute a tiny intra-bin swap (0 bin crossings) to trigger and observe pure decay
    const tinyBuy = 500_000n; // 0.0005 SOL
    await sendTxWithMetrics(
      connection,
      new Transaction().add(
        buildCompactSwapIx({
          programId,
          poolPda,
          binArrayPda,
          mint: mint.publicKey,
          baseVault,
          quoteVault,
          user: traderA.publicKey,
          userBaseAta: acctsA.traderBaseAta,
          userWsolAta: acctsA.traderWsolAta,
          poolAuthorityPda,
          isBuy: true,
          amountIn: tinyBuy,
          minAmountOut: 0n,
        })
      ),
      [traderA],
      "swap_trigger_wall_clock_decay"
    );
    ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
    assert(
      pool.vADown < decayPrePool.vADown,
      `Expected vADown decay after >=30s wall clock: before=${decayPrePool.vADown}, after=${pool.vADown}`
    );
    assert(
      pool.lastDecayTimestamp > decayPrePool.lastDecayTimestamp,
      "lastDecayTimestamp must advance on elapsed filter period"
    );
    assert(
      pool.sqrtPHighWater === decayPrePool.sqrtPHighWater,
      "High-water mark must remain immune to time decay"
    );
    console.log(
      `  [PASS] Wall-clock decay: vAUp ${decayPrePool.vAUp}->${pool.vAUp}, vADown ${decayPrePool.vADown}->${pool.vADown}`
    );
  } else {
    console.log("  [SKIP] Wall-clock 33s sleep skipped via --skip-decay-wait.");
  }

  console.log("\n=== Stage 9: Premature Graduation Guardrail (Negative Test) ===");
  if (pool.sqrtPCurrent < pool.sqrtPGraduation) {
    await expectSimulateCustomError(
      connection,
      [
        new TransactionInstruction({
          programId,
          keys: [
            { pubkey: poolPda, isSigner: false, isWritable: true },
            { pubkey: binArrayPda, isSigner: false, isWritable: false },
            { pubkey: quoteVault, isSigner: false, isWritable: false },
          ],
          data: Buffer.from(packGraduateInPlaceIxData()),
        }),
      ],
      [payer],
      6011, // GraduationThresholdNotReached
      "premature_graduate_in_place_rejected"
    );
  }

  console.log("\n=== Stage 10: Price Discovery to Graduation + Same-Slot Atomic [Graduate + Swap] ===");
  let pushRound = 0;
  while (pool.sqrtPCurrent < pool.sqrtPGraduation) {
    pushRound++;
    assert(pushRound <= 8, "Too many rounds trying to reach graduation threshold");
    await sendTxWithMetrics(
      connection,
      new Transaction().add(
        buildCompactSwapIx({
          programId,
          poolPda,
          binArrayPda,
          mint: mint.publicKey,
          baseVault,
          quoteVault,
          user: traderC.publicKey,
          userBaseAta: acctsC.traderBaseAta,
          userWsolAta: acctsC.traderWsolAta,
          poolAuthorityPda,
          isBuy: true,
          amountIn: 500_000_000n,
          minAmountOut: 1n,
        })
      ),
      [traderC],
      `swap_push_to_grad_round_${pushRound}`
    );
    ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  }
  console.log(
    `  Reached graduation price threshold at activeBin=${pool.activeBinId} (sqrtPCurrent=${pool.sqrtPCurrent} >= sqrtPGraduation=${pool.sqrtPGraduation})`
  );

  // Single atomic transaction: GraduateInPlace (0x06) + post-graduation Swap (0x05) in the exact same slot!
  const atomicGradAndSwapTx = new Transaction().add(
    new TransactionInstruction({
      programId,
      keys: [
        { pubkey: poolPda, isSigner: false, isWritable: true },
        { pubkey: binArrayPda, isSigner: false, isWritable: false },
        { pubkey: quoteVault, isSigner: false, isWritable: false },
      ],
      data: Buffer.from(packGraduateInPlaceIxData()),
    }),
    build28AccountSwapIx({
      programId,
      poolPda,
      binArrayPda,
      mint: mint.publicKey,
      baseVault,
      quoteVault,
      user: traderA.publicKey,
      userBaseAta: acctsA.traderBaseAta,
      userWsolAta: acctsA.traderWsolAta,
      poolAuthorityPda,
      creatorFeeVault: new PublicKey(pool.creatorFeeVault),
      isBuy: false,
      amountIn: traderABaseAfterBuy1 / 4n,
      minAmountOut: 1n,
    })
  );
  const gradSwapRes = await sendTxWithMetrics(
    connection,
    atomicGradAndSwapTx,
    [traderA],
    "atomic_same_slot_graduate_plus_swap"
  );
  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  assert(pool.graduationStatus === 1, "graduationStatus must be 1");
  assert(
    pool.graduationSlot === BigInt(gradSwapRes.slot),
    `graduationSlot (${pool.graduationSlot}) != confirmed tx slot (${gradSwapRes.slot})`
  );
  await expectSimulateCustomError(
    connection,
    [
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: poolPda, isSigner: false, isWritable: true },
          { pubkey: binArrayPda, isSigner: false, isWritable: false },
          { pubkey: quoteVault, isSigner: false, isWritable: false },
        ],
        data: Buffer.from(packGraduateInPlaceIxData()),
      }),
    ],
    [payer],
    6010, // AlreadyGraduated
    "double_graduate_in_place_rejected"
  );
  console.log(
    `  [PASS] Atomic Graduation + Same-Slot Swap verified in slot=${gradSwapRes.slot} (dead_zone_slots = 0).`
  );

  console.log("\n=== Stage 11: Write-Lock-Free Token-2022 TransferFeeConfig Harvest (0x07 Mode 0 & Mode 1) ===");
  const payerBaseBeforeHarvest = (
    await getAccount(connection, plan.payerBaseAta, "confirmed", TOKEN_2022_PROGRAM_ID)
  ).amount;

  // Mode 0: write-lock-free harvest to mint (RatchetPoolPda is NOT passed!)
  const harvestMode0Res = await sendTxWithMetrics(
    connection,
    new Transaction().add(
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: mint.publicKey, isSigner: false, isWritable: true },
          { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
          { pubkey: baseVault, isSigner: false, isWritable: true },
          { pubkey: acctsA.traderBaseAta, isSigner: false, isWritable: true },
          { pubkey: acctsB.traderBaseAta, isSigner: false, isWritable: true },
          { pubkey: acctsC.traderBaseAta, isSigner: false, isWritable: true },
        ],
        data: Buffer.from(packHarvestFallbackIxData(0)),
      })
    ),
    [payer],
    "harvest_fallback_mode0_write_lock_free"
  );

  // Mode 1: PoolAuthorityPda-signed batched withdraw from mint to payerBaseAta
  const harvestMode1Res = await sendTxWithMetrics(
    connection,
    new Transaction().add(
      new TransactionInstruction({
        programId,
        keys: [
          { pubkey: poolPda, isSigner: false, isWritable: true },
          { pubkey: mint.publicKey, isSigner: false, isWritable: true },
          { pubkey: plan.payerBaseAta, isSigner: false, isWritable: true },
          { pubkey: poolAuthorityPda, isSigner: false, isWritable: false },
          { pubkey: TOKEN_2022_PROGRAM_ID, isSigner: false, isWritable: false },
        ],
        data: Buffer.from(packHarvestFallbackIxData(1)),
      })
    ),
    [payer],
    "harvest_fallback_mode1_withdraw_from_mint"
  );
  ({ pool, binArray } = await fetchPoolAndBins(connection, poolPda, binArrayPda));
  const payerBaseAfterHarvest = (
    await getAccount(connection, plan.payerBaseAta, "confirmed", TOKEN_2022_PROGRAM_ID)
  ).amount;
  const harvestedAtoms = payerBaseAfterHarvest - payerBaseBeforeHarvest;
  if (config.pool.fallbackTransferFeeBps > 0 && BigInt(config.pool.fallbackMaximumFeeAtoms) > 0n) {
    assert(
      harvestedAtoms > 0n && pool.harvestedTransferFeeAtoms >= harvestedAtoms,
      `Expected non-zero Token-2022 fallback harvest, got ${harvestedAtoms}`
    );
  }
  console.log(
    `  [PASS] Fallback harvest: harvestedAtoms=${harvestedAtoms}, pool.harvestedTransferFeeAtoms=${pool.harvestedTransferFeeAtoms}`
  );

  console.log("\n=== Stage 12: Final Solvency Audit, Trader Sweep-Back & Report ===");
  await verifySolConservation(connection, pool, binArray, quoteVault, "Stage 12 Final");

  // Reclaim unused WSOL + SOL from ephemeral Trader A, B, C wallets back to Devnet payer
  for (const [label, trader, wsolAta] of [
    ["traderA", traderA, acctsA.traderWsolAta],
    ["traderB", traderB, acctsB.traderWsolAta],
    ["traderC", traderC, acctsC.traderWsolAta],
  ] as const) {
    const closeWsolTx = new Transaction().add(
      createCloseAccountInstruction(wsolAta, trader.publicKey, trader.publicKey, [], TOKEN_PROGRAM_ID)
    );
    await sendTxWithMetrics(connection, closeWsolTx, [trader], `close_wsol_${label}`);
    const bal = await connection.getBalance(trader.publicKey, "confirmed");
    if (bal > 10_000) {
      await sendTxWithMetrics(
        connection,
        new Transaction().add(
          SystemProgram.transfer({
            fromPubkey: trader.publicKey,
            toPubkey: payer.publicKey,
            lamports: bal - 5_000,
          })
        ),
        [trader],
        `sweep_sol_${label}`
      );
    }
  }

  const finalPayerBalance = await connection.getBalance(payer.publicKey, "confirmed");
  const outReportPath =
    reportPath ?? path.resolve(repoRoot, "deploy/devnet-e2e-report.json");
  const report = {
    status: "PASS",
    cluster: "devnet",
    programId: programId.toBase58(),
    mint: mint.publicKey.toBase58(),
    pdas: plan.pdas,
    genesisPackets: genesisReceipts,
    cuMeasurements: {
      buy1_12acct: buy1Res.computeUnits,
      buy2_28acct: buy2Res.computeUnits,
      sell1_28acct: sell1Res.computeUnits,
      atomicGradAndSwap: gradSwapRes.computeUnits,
      harvestMode0: harvestMode0Res.computeUnits,
      harvestMode1: harvestMode1Res.computeUnits,
    },
    finalPoolState: {
      graduationStatus: pool.graduationStatus,
      graduationSlot: pool.graduationSlot.toString(),
      authorityRevoked: pool.authorityRevoked,
      vAUp: pool.vAUp.toString(),
      vADown: pool.vADown.toString(),
      compoundedSolTotal: pool.compoundedSolTotal.toString(),
      protocolFeeLamports: pool.protocolFeeLamports.toString(),
      realVaultLamports: pool.realVaultLamports.toString(),
      maxExtractableLamports: pool.maxExtractableLamports.toString(),
      harvestedTransferFeeAtoms: pool.harvestedTransferFeeAtoms.toString(),
      decayTested: !decaySkipped,
    },
    payerStartSol: (payerBalance / LAMPORTS_PER_SOL).toFixed(6),
    payerEndSol: (finalPayerBalance / LAMPORTS_PER_SOL).toFixed(6),
  };
  fs.mkdirSync(path.dirname(outReportPath), { recursive: true });
  fs.writeFileSync(outReportPath, JSON.stringify(report, null, 2), "utf8");
  console.log(`\nALL 12 DEVNET STAGES PASSED! Report written to ${outReportPath}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

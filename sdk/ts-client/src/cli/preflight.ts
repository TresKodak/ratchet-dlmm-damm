/**
 * Read-only Mainnet Launch Preflight CLI (`npm.cmd run mainnet:preflight`).
 *
 * Strictly read-only: NEVER signs, creates keypair files, or broadcasts any transaction.
 */

import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID } from "@solana/spl-token";
import { Connection, Keypair, LAMPORTS_PER_SOL, PublicKey } from "@solana/web3.js";
import {
  ACTIVE_BIN_ARRAY_PDA_DATA_BYTES,
  DEFAULT_RATCHET_PROGRAM_ID,
  RATCHET_POOL_PDA_DATA_BYTES,
  TOTAL_PDA_RENT_STEP2_LAMPORTS,
  TOTAL_PDA_RENT_STEP5_LAMPORTS,
} from "../accounts.js";
import {
  buildRealGenesisTransactionPlan,
  loadAndValidateLaunchConfig,
  resolveUserPath,
} from "../solanaTx.js";

function parseArgs(argv: string[]): { configPath: string; allowPlaceholder: boolean } {
  const cliDir = path.dirname(fileURLToPath(import.meta.url));
  let configPath = path.resolve(cliDir, "../../../../deploy/launch.config.json");
  let allowPlaceholder = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--allow-placeholder") {
      allowPlaceholder = true;
    } else if (arg === "--config" && argv[i + 1]) {
      configPath = path.resolve(argv[i + 1] as string);
      i++;
    }
  }
  return { configPath, allowPlaceholder };
}

function loadKeypairReadOnly(filePath: string): { keypair: Keypair | null; exists: boolean } {
  const resolved = path.resolve(filePath);
  if (!fs.existsSync(resolved)) {
    return { keypair: null, exists: false };
  }
  const secret = JSON.parse(fs.readFileSync(resolved, "utf8")) as number[];
  return { keypair: Keypair.fromSecretKey(Uint8Array.from(secret)), exists: true };
}

async function main(): Promise<void> {
  const { configPath, allowPlaceholder } = parseArgs(process.argv.slice(2));
  const repoRoot = path.resolve(path.dirname(configPath), "..");
  const { config, placeholderWarnings } = loadAndValidateLaunchConfig(configPath, {
    allowPlaceholder,
  });

  const soPath = path.resolve(repoRoot, config.paths.programBinaryPath);
  const programKeypairPath = path.resolve(repoRoot, config.paths.programKeypairPath);
  const payerKeypairPath = resolveUserPath(
    process.env.SOLANA_KEYPAIR_PATH ?? config.paths.payerKeypairPath,
    repoRoot
  );
  const mintKeypairPath = path.resolve(repoRoot, config.paths.mintKeypairPath);

  const soExists = fs.existsSync(soPath);
  const soBytes = soExists ? fs.readFileSync(soPath) : null;
  const soSizeBytes = soBytes?.byteLength ?? 0;
  const soSha256 = soBytes
    ? crypto.createHash("sha256").update(soBytes).digest("hex")
    : null;
  const soIsElf = soBytes ? soBytes.subarray(0, 4).toString("hex") === "7f454c46" : false;

  const { keypair: programKeypair, exists: programKeypairExists } =
    loadKeypairReadOnly(programKeypairPath);
  const programKeypairPubkey = programKeypair?.publicKey.toBase58() ?? null;
  const programIdMatchesKeypair = programKeypairPubkey === config.paths.programId;

  const { keypair: payerKeypair, exists: payerExists } = loadKeypairReadOnly(payerKeypairPath);
  const previewPayer = payerKeypair?.publicKey ?? Keypair.generate().publicKey;

  const { keypair: existingMintKeypair, exists: mintExists } =
    loadKeypairReadOnly(mintKeypairPath);
  const previewMint = existingMintKeypair?.publicKey ?? Keypair.generate().publicKey;

  // Query live mainnet RPC read-only (safe fallback values noted if offline/rate-limited)
  const connection = new Connection(config.network.rpcUrl, "confirmed");
  let rpcOnline = false;
  let rpcError: string | null = null;
  let latestBlockhash = "11111111111111111111111111111111";
  let payerBalanceLamports = 0n;
  let programOnChainDeployed = false;
  let programOnChainExecutable = false;
  let programDataRentLamports = 0n;
  let programAccountRentLamports = 0n;
  let liveMintRentLamports = 2_500_000n;
  let livePoolPdaRentLamports = 4_231_680n;
  let liveBinArrayRentLamports = 8_017_920n;
  let liveBaseVaultRentLamports = 2_129_280n;
  let liveQuoteVaultRentLamports = 2_039_280n;

  // Build plan first to get exact totalMintAccountSpace
  const preliminaryPlan = buildRealGenesisTransactionPlan({
    config,
    payer: previewPayer,
    mint: previewMint,
    mintRentLamports: liveMintRentLamports,
    recentBlockhash: latestBlockhash,
  });

  try {
    const [
      bh,
      payerBal,
      programInfo,
      mintRent,
      poolRent,
      binRent,
      baseVaultRent,
      quoteVaultRent,
      progAccountRent,
      progDataRent,
    ] = await Promise.all([
      connection.getLatestBlockhash("confirmed"),
      payerExists ? connection.getBalance(previewPayer, "confirmed") : Promise.resolve(0),
      connection.getAccountInfo(new PublicKey(config.paths.programId), "confirmed"),
      connection.getMinimumBalanceForRentExemption(preliminaryPlan.totalMintAccountSpace),
      connection.getMinimumBalanceForRentExemption(Number(RATCHET_POOL_PDA_DATA_BYTES)),
      connection.getMinimumBalanceForRentExemption(Number(ACTIVE_BIN_ARRAY_PDA_DATA_BYTES)),
      connection.getMinimumBalanceForRentExemption(178),
      connection.getMinimumBalanceForRentExemption(165),
      connection.getMinimumBalanceForRentExemption(36),
      connection.getMinimumBalanceForRentExemption(soSizeBytes > 0 ? soSizeBytes * 2 + 45 : 250_000),
    ]);
    rpcOnline = true;
    latestBlockhash = bh.blockhash;
    payerBalanceLamports = BigInt(payerBal);
    programOnChainDeployed = programInfo !== null;
    programOnChainExecutable = Boolean(programInfo?.executable);
    liveMintRentLamports = BigInt(mintRent);
    livePoolPdaRentLamports = BigInt(poolRent);
    liveBinArrayRentLamports = BigInt(binRent);
    liveBaseVaultRentLamports = BigInt(baseVaultRent);
    liveQuoteVaultRentLamports = BigInt(quoteVaultRent);
    programAccountRentLamports = BigInt(progAccountRent);
    programDataRentLamports = BigInt(progDataRent);
  } catch (err) {
    rpcError = err instanceof Error ? err.message : String(err);
  }

  const finalPlan = buildRealGenesisTransactionPlan({
    config,
    payer: previewPayer,
    mint: previewMint,
    mintRentLamports: liveMintRentLamports,
    recentBlockhash: latestBlockhash,
  });

  if (!finalPlan.allPacketsWithinLimit) {
    throw new Error(
      `One or more genesis transactions exceed ${1232} bytes (max=${finalPlan.maxPacketBytes})`
    );
  }

  const txFeesLamports = BigInt(finalPlan.packets.length) * 5_000n;
  const jitoTipLamports = config.jito.enabled ? BigInt(config.jito.tipLamports) : 0n;
  const programDeployEstimateLamports = programOnChainDeployed
    ? 0n
    : programAccountRentLamports + programDataRentLamports + 2_000_000n;

  const genesisPdaAndVaultRentLamports =
    liveMintRentLamports +
    livePoolPdaRentLamports +
    liveBinArrayRentLamports +
    liveBaseVaultRentLamports * 2n +
    liveQuoteVaultRentLamports * 2n;

  const totalSolNeededLamports =
    programDeployEstimateLamports +
    genesisPdaAndVaultRentLamports +
    finalPlan.realVaultLamports +
    txFeesLamports +
    jitoTipLamports;

  const report = {
    mode: "READ_ONLY_PREFLIGHT",
    mainnetBroadcastExecuted: false,
    program: {
      programId: config.paths.programId,
      canonicalProgramId: DEFAULT_RATCHET_PROGRAM_ID,
      programKeypairExists,
      programKeypairPubkey,
      programIdMatchesKeypair,
      soPath: config.paths.programBinaryPath,
      soExists,
      soSizeBytes,
      soIsElf,
      soSha256,
      onChainDeployed: programOnChainDeployed,
      onChainExecutable: programOnChainExecutable,
    },
    brandingAndParameters: {
      allowPlaceholder,
      placeholderWarnings,
      tokenName: config.token.name,
      tokenSymbol: config.token.symbol,
      tokenUri: config.token.uri,
      decimals: config.token.decimals,
      totalSupplyAtoms: finalPlan.totalSupplyAtoms.toString(),
      realVaultLamports: finalPlan.realVaultLamports.toString(),
      maxExtractableLamports: finalPlan.maxExtractableLamports.toString(),
      surplusCushionLamports: finalPlan.surplusCushionLamports.toString(),
      f0Bps: config.pool.f0Bps,
      fMaxSellBps: config.pool.fMaxSellBps,
      compoundingFeeBps: config.pool.compoundingFeeBps,
      rentStep: config.pool.rentStep,
    },
    pdas: finalPlan.pdas,
    accounts: {
      payerKeypairExists: payerExists,
      payerPubkey: previewPayer.toBase58(),
      mintKeypairExists: mintExists,
      mintPubkeyPreview: previewMint.toBase58(),
      token2022ProgramId: TOKEN_2022_PROGRAM_ID.toBase58(),
      splTokenProgramId: TOKEN_PROGRAM_ID.toBase58(),
    },
    rpc: {
      cluster: config.network.cluster,
      rpcUrl: config.network.rpcUrl,
      rpcOnline,
      rpcError,
      payerBalanceLamports: payerBalanceLamports.toString(),
      payerBalanceSol: (Number(payerBalanceLamports) / LAMPORTS_PER_SOL).toFixed(6),
    },
    solBudgetEstimate: {
      programDeployEstimateLamports: programDeployEstimateLamports.toString(),
      liveToken2022MintRentLamports: liveMintRentLamports.toString(),
      livePoolAndBinPdaRentLamports: (livePoolPdaRentLamports + liveBinArrayRentLamports).toString(),
      simd0437Step2PdaRentLamports: TOTAL_PDA_RENT_STEP2_LAMPORTS.toString(),
      simd0437Step5PdaRentLamports: TOTAL_PDA_RENT_STEP5_LAMPORTS.toString(),
      liveVaultsAndAtasRentLamports: (liveBaseVaultRentLamports * 2n + liveQuoteVaultRentLamports * 2n).toString(),
      initialWsolVaultLamports: finalPlan.realVaultLamports.toString(),
      txFeesLamports: txFeesLamports.toString(),
      jitoTipLamports: jitoTipLamports.toString(),
      totalRequiredLamports: totalSolNeededLamports.toString(),
      totalRequiredSol: (Number(totalSolNeededLamports) / LAMPORTS_PER_SOL).toFixed(6),
      payerSufficientForLaunch: payerBalanceLamports >= totalSolNeededLamports,
    },
    transactions: {
      packetCount: finalPlan.packets.length,
      maxPacketBytes: finalPlan.maxPacketBytes,
      allPacketsWithin1232Bytes: finalPlan.allPacketsWithinLimit,
      packets: finalPlan.packets.map((p) => ({
        name: p.name,
        serializedBytes: p.serializedBytes,
        withinLimit: p.withinPacketLimit,
        requiresMintSigner: p.requiresMintSigner,
        description: p.description,
      })),
    },
    readyForMainnetLaunch:
      soExists &&
      soIsElf &&
      programIdMatchesKeypair &&
      finalPlan.allPacketsWithinLimit &&
      placeholderWarnings.length === 0,
  };

  console.log(JSON.stringify(report, null, 2));

  if (!soExists || !soIsElf) {
    console.error("PREFLIGHT ERROR: compiled .so binary is missing or not a valid ELF.");
    process.exitCode = 1;
    return;
  }
  if (!programIdMatchesKeypair) {
    console.error("PREFLIGHT ERROR: deploy/ratchet_dlmm_damm-keypair.json does not match launch.config.json programId.");
    process.exitCode = 1;
    return;
  }
  if (!finalPlan.allPacketsWithinLimit) {
    console.error("PREFLIGHT ERROR: one or more genesis transactions exceed 1,232 bytes.");
    process.exitCode = 1;
    return;
  }
  if (placeholderWarnings.length > 0 && !allowPlaceholder) {
    console.error("PREFLIGHT BLOCKED: placeholder coin branding detected.");
    process.exitCode = 1;
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

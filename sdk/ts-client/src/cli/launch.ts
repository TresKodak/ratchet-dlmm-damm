/**
 * Armed Mainnet Launch CLI (`npm.cmd run mainnet:launch -- --confirm-mainnet-launch`).
 *
 * SAFETY GUARDRAILS:
 * - Refuses to execute without explicit `--confirm-mainnet-launch`.
 * - Refuses placeholder branding (`REPLACE_WITH_*`).
 * - Verifies compiled program is already deployed & executable on-chain before sending genesis bundle.
 * - Supports `--jito` Jito bundle submission or sequential confirmed RPC execution.
 * - Verifies post-launch on-chain `RatchetPoolPda` invariants (`authorityRevoked === 1`,
 *   `f0Bps === 8000`, `fMaxSellBps === 2500`, `compoundingFeeBps >= 7500`, Gate G06 solvency).
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { encodeBase58 } from "../accounts.js";
import {
  buildRealGenesisTransactionPlan,
  decodeRatchetPoolPda,
  loadAndValidateLaunchConfig,
  resolveUserPath,
  validateActiveBinArrayPdaLen,
} from "../solanaTx.js";

function parseArgs(argv: string[]): {
  configPath: string;
  confirmMainnetLaunch: boolean;
  useJito: boolean;
} {
  const cliDir = path.dirname(fileURLToPath(import.meta.url));
  let configPath = path.resolve(cliDir, "../../../../deploy/launch.config.json");
  let confirmMainnetLaunch = false;
  let useJito = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--confirm-mainnet-launch") {
      confirmMainnetLaunch = true;
    } else if (arg === "--jito") {
      useJito = true;
    } else if (arg === "--config" && argv[i + 1]) {
      configPath = path.resolve(argv[i + 1] as string);
      i++;
    }
  }
  return { configPath, confirmMainnetLaunch, useJito };
}

function loadOrCreateKeypair(filePath: string, createIfMissing: boolean): Keypair {
  const resolved = path.resolve(filePath);
  if (fs.existsSync(resolved)) {
    const secret = JSON.parse(fs.readFileSync(resolved, "utf8")) as number[];
    return Keypair.fromSecretKey(Uint8Array.from(secret));
  }
  if (!createIfMissing) {
    throw new Error(`Required keypair file not found: ${resolved}`);
  }
  fs.mkdirSync(path.dirname(resolved), { recursive: true });
  const kp = Keypair.generate();
  fs.writeFileSync(resolved, JSON.stringify(Array.from(kp.secretKey)), "utf8");
  return kp;
}

async function main(): Promise<void> {
  const { configPath, confirmMainnetLaunch, useJito } = parseArgs(process.argv.slice(2));
  if (!confirmMainnetLaunch) {
    throw new Error(
      "Safety abort: `mainnet:launch` requires explicit `--confirm-mainnet-launch` flag."
    );
  }

  const repoRoot = path.resolve(path.dirname(configPath), "..");
  // Strictly disallow placeholders on launch!
  const { config } = loadAndValidateLaunchConfig(configPath, { allowPlaceholder: false });

  const payer = loadOrCreateKeypair(
    resolveUserPath(process.env.SOLANA_KEYPAIR_PATH ?? config.paths.payerKeypairPath, repoRoot),
    false
  );
  const mint = loadOrCreateKeypair(path.resolve(repoRoot, config.paths.mintKeypairPath), true);

  const connection = new Connection(config.network.rpcUrl, "confirmed");
  const programPubkey = new PublicKey(config.paths.programId);
  const programAcct = await connection.getAccountInfo(programPubkey, "confirmed");
  if (!programAcct || !programAcct.executable) {
    throw new Error(
      `Program ${config.paths.programId} is not deployed/executable on ${config.network.cluster}. Run \`solana program deploy\` first.`
    );
  }

  const dummyPlan = buildRealGenesisTransactionPlan({
    config,
    payer: payer.publicKey,
    mint: mint.publicKey,
    mintRentLamports: 2_500_000n,
  });
  const mintRentLamports = BigInt(
    await connection.getMinimumBalanceForRentExemption(dummyPlan.totalMintAccountSpace)
  );
  const { blockhash } = await connection.getLatestBlockhash("confirmed");

  const plan = buildRealGenesisTransactionPlan({
    config,
    payer: payer.publicKey,
    mint: mint.publicKey,
    mintRentLamports,
    recentBlockhash: blockhash,
  });

  if (!plan.allPacketsWithinLimit) {
    throw new Error(`Genesis transaction packet exceeds 1,232 bytes (max=${plan.maxPacketBytes})`);
  }

  const signatures: string[] = [];
  let jitoBundleId: string | null = null;

  if (useJito || config.jito.enabled) {
    const encodedTxs: string[] = [];
    for (let i = 0; i < plan.packets.length; i++) {
      const pkt = plan.packets[i]!;
      const tx = pkt.transaction;
      tx.recentBlockhash = blockhash;
      if (i === plan.packets.length - 1 && config.jito.tipLamports > 0) {
        tx.add(
          SystemProgram.transfer({
            fromPubkey: payer.publicKey,
            toPubkey: new PublicKey(config.jito.tipAccount),
            lamports: config.jito.tipLamports,
          })
        );
      }
      const signers = pkt.requiresMintSigner ? [payer, mint] : [payer];
      tx.sign(...signers);
      const raw = tx.serialize();
      if (raw.byteLength > 1_232) {
        throw new Error(`Signed Jito tx ${pkt.name} exceeds 1,232 bytes (${raw.byteLength})`);
      }
      encodedTxs.push(encodeBase58(raw));
      if (tx.signature) {
        signatures.push(encodeBase58(tx.signature));
      }
    }

    const res = await fetch(config.jito.blockEngineUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "sendBundle",
        params: [encodedTxs],
      }),
    });
    const json = (await res.json()) as { result?: string; error?: unknown };
    if (!res.ok || json.error || !json.result) {
      throw new Error(`Jito sendBundle failed: ${JSON.stringify(json)}`);
    }
    jitoBundleId = json.result;
  } else {
    for (const pkt of plan.packets) {
      const bh = await connection.getLatestBlockhash("confirmed");
      const tx = Transaction.from(
        pkt.transaction.serialize({ requireAllSignatures: false, verifySignatures: false })
      );
      tx.recentBlockhash = bh.blockhash;
      const signers = pkt.requiresMintSigner ? [payer, mint] : [payer];
      const sig = await sendAndConfirmTransaction(connection, tx, signers, {
        commitment: "confirmed",
      });
      signatures.push(sig);
    }
  }

  // Verify post-launch on-chain state
  const [poolInfo, binArrayInfo] = await Promise.all([
    connection.getAccountInfo(new PublicKey(plan.pdas.ratchetPoolPda), "confirmed"),
    connection.getAccountInfo(new PublicKey(plan.pdas.activeBinArrayPda), "confirmed"),
  ]);
  if (!poolInfo || !binArrayInfo) {
    throw new Error("Post-launch verification failed: PDA accounts not found on-chain.");
  }

  const decodedPool = decodeRatchetPoolPda(poolInfo.data);
  if (!validateActiveBinArrayPdaLen(binArrayInfo.data)) {
    throw new Error("Post-launch verification failed: invalid ActiveBinArrayPda size.");
  }
  if (decodedPool.authorityRevoked !== 1) {
    throw new Error("Post-launch verification failed: authorityRevoked != 1");
  }
  if (decodedPool.f0Bps !== 8_000 || decodedPool.fMaxSellBps !== 2_500 || decodedPool.compoundingFeeBps < 7_500) {
    throw new Error("Post-launch verification failed: immutable fee/compounding bounds violated.");
  }
  if (decodedPool.maxExtractableLamports > decodedPool.realVaultLamports) {
    throw new Error("Post-launch verification failed: Gate G06 solvency violated.");
  }

  const receipt = {
    launchedAt: new Date().toISOString(),
    cluster: config.network.cluster,
    programId: config.paths.programId,
    mint: mint.publicKey.toBase58(),
    pdas: plan.pdas,
    jitoBundleId,
    signatures,
    verifiedOnChainState: {
      authorityRevoked: decodedPool.authorityRevoked,
      f0Bps: decodedPool.f0Bps,
      fMinBps: decodedPool.fMinBps,
      fMaxSellBps: decodedPool.fMaxSellBps,
      compoundingFeeBps: decodedPool.compoundingFeeBps,
      realVaultLamports: decodedPool.realVaultLamports.toString(),
      maxExtractableLamports: decodedPool.maxExtractableLamports.toString(),
      activeBinId: decodedPool.activeBinId,
    },
  };

  const receiptPath = path.resolve(repoRoot, config.paths.launchReceiptPath);
  fs.mkdirSync(path.dirname(receiptPath), { recursive: true });
  fs.writeFileSync(receiptPath, JSON.stringify(receipt, null, 2), "utf8");
  console.log(JSON.stringify(receipt, null, 2));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

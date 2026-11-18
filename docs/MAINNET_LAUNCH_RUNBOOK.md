# Ratchet-DLMM-DAMM — Mainnet Launch Runbook

> [!CAUTION]
> **Nothing in this repository has been deployed to or broadcast on Solana Mainnet-Beta.** No launch receipt (`deploy/launch-receipt.json`) or mint keypair (`deploy/mint-keypair.json`) exists until you execute a live launch. Mainnet launches spend real SOL and permanently revoke mint, freeze, fee-config, and pool authorities at genesis.

## 1. Launch Artifacts

| Item | Value |
| :--- | :--- |
| Program ID | `5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5` |
| Program keypair | `deploy/ratchet_dlmm_damm-keypair.json` (**secret — git-ignored, back up offline or generate your own**) |
| Compiled binary | `programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so` |
| Binary size | `192,032` bytes (`elf64-bpf`) |
| Binary SHA-256 | `E39D554DA42CFC1CACE132777506B396ABDA05C6C227BF84635084097249C194` |
| Build toolchain | Solana `2.1.22` / `platform-tools v1.43` (`rustc 1.79.0`) |
| Launch config template | `deploy/launch.config.example.json` (copy to git-ignored `deploy/launch.config.json`) |
| CLI commands | `sdk/ts-client`: `mainnet:preflight` (read-only), `mainnet:launch` (armed) |

> [!IMPORTANT]
> If you generate a new program keypair (`solana-keygen new -o deploy/ratchet_dlmm_damm-keypair.json`), update `declare_id!` in `programs/ratchet-dlmm-damm/src/lib.rs`, `RATCHET_PROGRAM_ID_STR` in `sdk/jupiter-adapter/src/lib.rs`, `DEFAULT_RATCHET_PROGRAM_ID` in `sdk/ts-client/src/accounts.ts`, and `paths.programId` in your `launch.config.json`, then rebuild the `.so` and re-run preflight so the SHA-256 and Program ID match across the repository.

### Platform Build Notes
- **Linux / macOS**: Run `cargo-build-sbf` inside `programs/ratchet-dlmm-damm` directly.
- **Windows**: Native `cargo-build-sbf` requires `HOME` set (`$env:HOME = $env:USERPROFILE`) and `SeCreateSymbolicLinkPrivilege` (Windows Developer Mode) to unpack `platform-tools`. If Developer Mode is off, build inside a Linux Docker container with Solana `2.1.22` / `platform-tools v1.43` or WSL2.
- **Windows `tests/litesvm-e2e` OpenSSL note**: `litesvm` (`solana-sdk`) builds `openssl-sys` via `perl` + MSVC. If `perl` is not in `PATH`, copy `tests/litesvm-e2e/.cargo/config.toml.example` to `tests/litesvm-e2e/.cargo/config.toml` (git-ignored) and set `OPENSSL_SRC_PERL` to your Perl binary. On Windows PowerShell, use `npm.cmd` if script execution policy restricts `npm.ps1`.

## 2. Instruction & PDA Reference

PDA seeds (`programs/ratchet-dlmm-damm/src/instruction.rs`):

| Account | Seeds |
| :--- | :--- |
| `ratchet_pool_pda` (480 B) | `[b"ratchet_pool", base_mint]` |
| `active_bin_array_pda` (1,024 B) | `[b"bin_array", pool_pda, start_bin_id.to_le_bytes()]` |
| `pool_authority_pda` | `[b"pool_authority", pool_pda]` |
| Base vault (Token-2022) | `[b"base_vault", pool_pda]` |
| Quote vault (WSOL) | `[b"quote_vault", pool_pda]` |

| Disc. | Instruction | Wire size | Purpose |
| :---: | :--- | :---: | :--- |
| `0x01` | `InitializePool` | 64 B | Create/initialize `RatchetPoolPda`, validate mints & vaults |
| `0x02` | `InitializeBinArray` | 31 B | Create/initialize `ActiveBinArrayPda` |
| `0x03` | `SeedInitialBins` | `18 + 20·n` B | Seed bins, deposit base + WSOL, enforce G06 solvency |
| `0x04` | `RevokeAndOpenSlot0` | 1 B | Revoke authorities, open Slot-0 at `f_0 = 8000 bps` |
| `0x05` | `Swap` | 18 B | Buy/sell (12-account compact or 28-account layout) |
| `0x06` | `GraduateInPlace` | 1 B | Atomic `graduation_status` 0→1 (G06 + G10) |
| `0x07` | `HarvestFallbackTransferFees` | 2 B | Write-lock-free Token-2022 fee harvest/withdraw |

## 3. Step 1 — Configure Branding & Vault Parameters

Copy `deploy/launch.config.example.json` to `deploy/launch.config.json` (git-ignored) and set your custom coin metadata and initial liquidity:

```bash
cp deploy/launch.config.example.json deploy/launch.config.json
```

| Field | Meaning | Notes |
| :--- | :--- | :--- |
| `token.name` | On-chain `TokenMetadata` name | Replace any `REPLACE_WITH_*` placeholder |
| `token.symbol` | On-chain symbol | Replace any `REPLACE_WITH_*` placeholder |
| `token.uri` | Off-chain metadata JSON URL | Host on Arweave, IPFS, or HTTPS |
| `token.decimals` | Mint decimals | Default `6` |
| `token.totalSupplyTokens` | Whole-token supply (string) | Default `"1000000000"`; `supply × 10^decimals` must fit in `u64` |
| `pool.realVaultSol` | WSOL deposited into the quote vault | Default `"86.5"` |
| `pool.maxExtractableSol` | G06 extractable bound | Default `"85.0"`; **must be ≤ `realVaultSol`** |
| `pool.rentStep` | `"step2"` (5,080 L/B) or `"step5"` (696 L/B) | Rent estimate hint; preflight also queries live RPC rent |
| `network.rpcUrl` | Solana RPC endpoint | Dedicated RPC recommended (public endpoint is rate-limited) |
| `paths.payerKeypairPath` | Funding wallet path | Default `~/.config/solana/id.json` (or override via `SOLANA_KEYPAIR_PATH`) |

Immutable parameters enforced by the CLI and on-chain checks: `f0Bps = 8000`, `fMinBps = 25`, `fMaxSellBps = 2500`, `compoundingFeeBps >= 7500`, `maxSellImpactBps <= 750` (default `700`).

**Off-chain Metadata JSON** (host publicly and place its URL in `token.uri`):

```json
{
  "name": "My Token",
  "symbol": "MYTKN",
  "description": "One-line description",
  "image": "https://example.com/logo.png"
}
```

> [!WARNING]
> Token-2022 `TokenMetadata` `updateAuthority` is set to `null` during genesis setup. **Token name, symbol, and URI are immutable after launch.**

## 4. Step 2 — Read-Only Preflight

```bash
cd sdk/ts-client
npm install
npm run mainnet:preflight
```

Preflight is strictly read-only: it never signs, writes keypair files, or broadcasts transactions. It checks:

- `.so` existence, ELF magic, byte length, and SHA-256; program keypair pubkey vs. `paths.programId`
- Branding validation (exits non-zero if placeholders remain unless `--allow-placeholder` is passed for dry-run testing)
- Live RPC reads: payer balance, program deployed/executable status, rent-exemption minimums, latest blockhash
- Estimated total SOL required (program deploy rent + mint rent + PDA rent + vault rent + initial WSOL vault + fees [+ optional Jito tip])
- Every genesis transaction packet is ≤ 1,232 bytes (7 packets: tx1–tx3, chunked `SeedInitialBins` batches 4a–4c covering all 30 bins, and tx5)

Proceed only when `readyForMainnetLaunch` and `payerSufficientForLaunch` are both `true` (and `rpcOnline` is `true` for live rent verification).

## 4a. Step 2.5 — Devnet Rehearsal (strongly recommended)

Before spending real SOL, run the complete lifecycle against the same `.so` on Devnet. Devnet SOL has no value, and the suite exercises things no local test can: real RPC behavior, wall-clock accumulator decay, and same-slot graduation on a live cluster.

1. Copy your mainnet config's pool parameters into `deploy/launch.devnet.json` (git-ignored), and set `paths.payerKeypairPath` to a funded Devnet wallet. Fund it generously: the program deploy, the test vault (`realVaultSol`), and the ephemeral trader wallets all draw from it, and most trader funds are swept back in Stage 12.
2. Deploy and run:

```bash
solana program deploy programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so \
  --program-id deploy/ratchet_dlmm_damm-keypair.json \
  --keypair <DEVNET_KEYPAIR_JSON> \
  --url devnet

cd sdk/ts-client
npm run devnet:e2e                  # reads deploy/launch.devnet.json
# options: --keypair <path>  --rpc <url>  --report <path>  --skip-decay-wait
```

The 12 stages:

| # | Stage | What it proves |
| :---: | :--- | :--- |
| 1 | Deployment & balance | Program is executable at the expected ID |
| 2 | 7-packet genesis | Mint extensions and both PDAs decode byte-for-byte as specified |
| 3 | Post-revocation checks | Revoked authorities can't mint or reconfigure |
| 4 | Slot-0 multi-wallet buys | 80% base fee plus `v_a_up` surcharge; 12-account swap |
| 5 | Off-chain quote parity | SDK fee preview matches on-chain fees; 28-account swap |
| 6 | Negative guardrails | `6007` sell impact, `6008` slippage, `6013` zero amount |
| 7 | Dump-brake sell | 5× sell fee, 75% floor compounding, high-water stays latched, 0-lamport drift |
| 8 | Accumulator decay | Real wall-clock decay after the 30 s filter (`--skip-decay-wait` skips it) |
| 9 | Premature graduation | Rejected with `6011` |
| 10 | Graduation + same-slot swap | One transaction; double graduation rejected with `6010` |
| 11 | Transfer-fee harvest | `0x07` mode 0 (harvest to mint) and mode 1 (withdraw) |
| 12 | Solvency & cleanup | Final vault audit, trader SOL swept back, `deploy/devnet-e2e-report.json` written |

Proceed to mainnet only if the run ends with `ALL 12 DEVNET STAGES PASSED` and you have reviewed the report.

> [!NOTE]
> Devnet reuses the same program ID as mainnet. That is intentional (it rehearses the exact binary), but it means the Devnet deployer wallet becomes the Devnet upgrade authority. That has no effect on mainnet.

## 5. Step 3 — Mainnet Deploy & Genesis Launch

Execute these commands only when you are ready to spend real SOL on Solana Mainnet-Beta:

```bash
# 1. Deploy the compiled program
solana program deploy programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so \
  --program-id deploy/ratchet_dlmm_damm-keypair.json \
  --keypair ~/.config/solana/id.json \
  --url mainnet-beta

# 2. Execute the genesis bundle (requires explicit confirmation flag)
cd sdk/ts-client
npm run mainnet:launch -- --confirm-mainnet-launch
```

`mainnet:launch` aborts unless `--confirm-mainnet-launch` is passed, custom branding is non-placeholder, the program is deployed and executable on-chain, and every transaction packet is `<= 1,232` bytes. It submits transactions sequentially at `confirmed` (or atomically as a Jito bundle with `--jito`), then reads back `RatchetPoolPda` to verify `authority_revoked == 1`, `f_0_bps == 8000`, `f_max_sell_bps == 2500`, `compounding_fee_bps >= 7500`, and `max_extractable <= real_vault` before writing `deploy/launch-receipt.json`.

> [!WARNING]
> Sequential mode submits 7 transactions rather than a single atomic bundle; use `--jito` if you want atomic bundle inclusion for the genesis transactions. Back up `deploy/mint-keypair.json` after launch.

## 6. Verification Evidence

### 6.1 Suite Results

> [!NOTE]
> Results were recorded before the 30-bin genesis calibration and the Devnet suite were added. The on-chain `.so` hasn't changed since then (same SHA-256), but `sdk/ts-client` needs a re-run, and the Devnet rehearsal hasn't run yet. Record the commit hash with each new result.

| Check | Result |
| :--- | :--- |
| `programs/ratchet-dlmm-damm`: `cargo test` | 13 passed, 0 failed |
| `programs/ratchet-dlmm-damm`: `cargo clippy --all-targets -- -D warnings` | 0 warnings |
| `.so` build | `192,032` B ELF64 BPF, SHA-256 `E39D554DA42CFC1CACE132777506B396ABDA05C6C227BF84635084097249C194` |
| `sdk/jupiter-adapter`: `cargo test` / `cargo clippy` | 6 passed / 0 warnings |
| `sdk/ts-client`: `npm run check` / `npm test` | exit 0 pre-calibration (`6 packets, max 726 B ≤ 1,232 B`); **re-run needed** after 30-bin/Devnet calibration edits (`7 packets`) |
| `mainnet:preflight` (placeholder blocked) | exits 1 without `--allow-placeholder` |
| `mainnet:preflight -- --allow-placeholder` | exits 0, `mainnetBroadcastExecuted: false`, `programIdMatchesKeypair: true`, `onChainDeployed: false` |
| `audit-harness`: `cargo clippy` / `cargo test` / `cargo run` | `0` warnings; `13 passed`; `10/10 gates PASS`; verifies `.so` ELF64 BPF, SHA-256, cross-repo `declare_id!`, and LiteSVM `G02` CU parity (`35,069 <= 43,000 CU`) |

> [!WARNING]
> `audit-harness` verifies deterministic mathematical invariants and simulated attack scenarios against the model (`43,000 CU` budget model; simulated Gate `G07`/`G08` economic scenarios). See LiteSVM results below for actual sBPF VM measurements on the compiled `.so`.

### 6.2 LiteSVM End-to-End on Compiled `.so` (`tests/litesvm-e2e`)

`tests/litesvm-e2e` (`ratchet_litesvm_e2e`, `litesvm 0.5.0`) loads `programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so` alongside live SPL Token-2022 and SPL Token BPF programs (`cargo clippy --all-targets -- -D warnings`: `0` warnings; `cargo test`: `2 passed, 0 failed`):

| Measured Instruction / Gate Check (`cargo test -- --nocapture --test-threads=1`) | Measured Result |
| :--- | :--- |
| `InitializePool + InitializeBinArray` (`480 B` + `1,024 B`) | `81,566 CU` |
| `SeedInitialBins` (`86.5 SOL` real / `85.0 SOL` max extractable + negative G06 check) | `22,733 CU` |
| `RevokeAndOpenSlot0` (revokes mint/freeze/transfer-fee-config/pool auths; `f_0=8000`, `f_max_sell=2500`, `compounding=7500`) | `15,858 CU` |
| `Swap` Buy 1 (12-acct, Slot-0 `8,000 bps` base fee, `v_a_up` & `sqrt_p_high_water` ratchet) | `27,050 CU` |
| `Swap` Buy 2 (28-acct, high-water decayed base buy fee `< 8,000`) | `31,225 CU` |
| `Swap` Sell 1 (28-acct, `5x` dump-brake + `75%` OnlyB floor compounding + G01 `0`-lamport drift) | `31,605 CU` |
| `Swap` Buy-to-Graduation (12-acct, peak observed swap CU) | `35,069 CU` (**`<= 43,000 CU` G02 budget**) |
| Gate G09 oversized single-leg dump rejection (`> 700 bps`) | Rejected (`Custom(6007)`) |
| `GraduateInPlace` (slot `100`; premature grad rejected w/ `Custom(6011)`) | `1,617 CU` (`dead_zone_slots = 0`) |
| `Swap` Post-Grad Same-Slot Buy (slot `100`, 12-acct) / Next-Slot Sell (slot `101`, 12-acct) | `30,000 CU` / `24,635 CU` (Gate G10) |
| `HarvestFallbackTransferFees` (`100 bps` Token-2022 mint): mode `0` (`HarvestWithheldToMint`) / mode `1` (`WithdrawWithheldFromMint`) | `9,950 CU` / `10,398 CU` (`60,200,000` atoms settled) |

## 7. Known Limits & Risks

- **No third-party audit.** Internal unit tests, deterministic harness checks, and LiteSVM E2E tests do not replace an independent smart-contract security audit before committing real liquidity.
- **Program upgrade authority.** `solana program deploy` leaves the deployer wallet as upgrade authority until you explicitly run `solana program set-upgrade-authority <PROGRAM_ID> --final` (irreversible).
- **Key custody.** Keep `deploy/*-keypair.json`, `deploy/launch.config.json` (if storing a private RPC API key), and your deployer wallet keypair out of version control (`git status` before pushing).
- **Rent schedule.** Step 2 vs. Step 5 rent reflects Solana SIMD-0437 schedule stages; the CLI queries live cluster rent exemptions at runtime.

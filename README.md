# Ratchet-DLMM-DAMM

**Directional Dump-Brake Volatility Accumulator + $\sqrt{P}$ High-Water Fee Scheduler with Quote Auto-Compounding**

Ratchet-DLMM-DAMM is a Solana program for launching and trading a Token-2022 token against SOL. Its fee schedule is designed to make two common launch exploits expensive: buying the entire opening supply in the first slot, and dumping a position in one fast cascade.

> [!CAUTION]
> **Status: not deployed to mainnet, and not independently audited.** The economic results for gates G07 and G08 below come from a simulation model, not from on-chain observation. Launching a pool revokes the mint, freeze, fee-config, and pool authorities permanently. Nothing in this repository is financial advice, and the software is provided without warranty.

---

## 1. How It Works

- **Buy fees start high and only come down as price sets new highs.** Trading opens at an 80% base buy fee. Each time the all-time-high price climbs one bin step, the base fee is multiplied by γ (0.9 by default). Falling prices never reset it.
- **Fast sell-offs pay more, and most of that fee stays in the pool.** Sell fees grow with how many bins price has recently dropped through. They are 5× more sensitive than the buy-side surcharge and capped at 25%. Of every sell fee, 75% is deposited into the pool's floor bin within the same instruction.
- **Single sells that move price too far are rejected.** A sell that would move price more than 700 bps in one transaction fails before any state changes.
- **Graduation happens in place.** When price reaches a preset threshold, a status flag flips inside the pool account. There is no migration to a separate pool, so trading never pauses.

### What the fee curve actually looks like

With the default parameters (`f_0_bps = 8000`, `gamma_bps = 9000`, `f_min_bps = 25`), the **base** buy fee after `n` new high-water bin steps is about $8000 \cdot 0.9^{n}$ bps:

| High-water steps above open | 0 | 4 | 8 | **12** (graduation) | 16 | **21** (top seeded bin) | 55 |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Base buy fee (bps) | 8,000 | ≈5,249 | ≈3,444 | **≈2,259** | ≈1,482 | **≈875** | 25 (floor) |

> [!IMPORTANT]
> **The 25 bps floor can't be reached with the bundled pool geometry.** One `ActiveBinArrayPda` holds 30 bins. The bundled genesis builder ([solanaTx.ts](sdk/ts-client/src/solanaTx.ts)) seeds bins `-8..=21` and puts the graduation threshold at **+12 steps**. That means the base buy fee is still about **22.6% at graduation** and bottoms out around **8.75%** at the highest seeded bin. Reaching 25 bps takes about 55 steps. With a 100 bps bin step, +12 steps is roughly 12.7% above the opening price, and +21 steps is roughly 23%. The upward-volatility surcharge described below is charged on top of these base fees.

---

## 2. Architecture Overview

The program combines three existing Solana AMM design patterns in two zero-copy accounts:

1. **Discrete bins with directional volatility accumulators (from Meteora DLMM).** Upward ($v_a^+$) and downward ($v_a^-$) bin crossings are counted separately. The sell-side coefficient is five times the buy-side one.
2. **A √P high-water fee scheduler (adapted from Meteora DAMM v2's market-cap scheduler).** The base buy fee decays with price progress rather than elapsed time, and it is latched to the highest √P reached.
3. **Quote-side ("OnlyB") compounding, in-place graduation, and fee-harvest isolation.** Sell-fee compounding is paid in SOL. Graduation is a single flag flip. Token-2022 `TransferFeeConfig` withheld fees are harvested by a separate instruction, so harvesting never takes the swap write lock.

| Component | Mechanism | On-chain parameter / invariant |
| :--- | :--- | :--- |
| Compact accounts | Two `#[repr(C)]` `Pod` structs | 480 B pool + 1,024 B bin array |
| High-water buy ratchet | Base fee latched to $\sqrt{P}_{\max}$ | $8000 \cdot \gamma^{n}$ bps, floor 25 bps (see §1 for the reachable range) |
| Directional accumulators | Separate $v_a^+$ / $v_a^-$ counters | Decay after a 30 s filter period; reset after 600 s |
| Dump-brake | 5× sell sensitivity | $A_{\text{sell}} = 5 \cdot A_{\text{buy}}$, hard cap 2,500 bps |
| Sell-fee compounding | 75% of sell fee to floor bin | `compounding_fee_bps = 7500`, exact integer conservation |
| Sell impact guardrail | Pre-mutation bin-traversal check | Rejects single sells above 700 bps (hard cap 750 bps) |
| Solvency & graduation | Enforced invariant + status flag | `max_extractable_lamports <= real_vault_lamports`; graduation is one flag flip |
| Hot-path budget | Single write-lock lane | 43,000 CU design budget; 35,069 CU peak measured in LiteSVM |

Buy fees are credited to `protocol_fee_lamports` and stay in the quote vault. They are not added to any bin's reserves (see `execute_buy_swap` in [math.rs](programs/ratchet-dlmm-damm/src/math.rs)).

---

## 3. Accounts & Rent

- **Mint extensions:** Token-2022 `TransferFeeConfig` (fallback fee on direct wallet-to-wallet transfers, harvested out-of-band), `MetadataPointer`, and `TokenMetadata`.
- **Program accounts:** Rent below follows the [SIMD-0437](https://github.com/solana-foundation/solana-improvement-documents) phased rent schedule, where each account is billed for its data plus 128 bytes of overhead. The CLI always queries live cluster rent at runtime.

| Account | Data bytes | Billable bytes | Step 2 rent (5,080 L/B) | Step 5 rent (696 L/B) |
| :--- | ---: | ---: | ---: | ---: |
| `ratchet_pool_pda` | 480 | 608 | 3,088,640 L | 423,168 L |
| `active_bin_array_pda` | 1,024 | 1,152 | 5,852,160 L | 801,792 L |
| **Total** | **1,504** | **1,760** | **8,940,800 L** | **1,224,960 L** |

Full byte-offset maps are in [TOKEN_CONCEPT_SPEC.md §2](docs/TOKEN_CONCEPT_SPEC.md).

---

## 4. Fee Equations

All fees are in basis points (1 bps = 0.01%). $s$ is `bin_step_bps`, and $n$ is the number of bin steps $\sqrt{P}_{\max}$ sits above $\sqrt{P}_{\text{start}}$.

**Buy fee:**
$$f_{\text{buy}} = \max\left(f_{\min},\ f_0 \cdot \gamma^{n}\right) + \left\lceil \frac{A_{\text{buy}} \cdot (v_a^+ \cdot s)^2}{10^{11}} \right\rceil$$

**Sell fee**, where $f_{\text{base}} = f_{\min} = 25$ bps:
$$f_{\text{sell}} = \min\left(2500,\ f_{\text{base}} + \left\lceil \frac{5 \cdot A_{\text{buy}} \cdot (v_a^- \cdot s)^2}{10^{11}} \right\rceil\right)$$

**SOL compounded into the floor bin per sell:**
$$\Delta R_{\text{sol}} = \left\lfloor \frac{\text{Gross\_SOL\_Out} \cdot f_{\text{sell}} \cdot \text{compounding\_fee\_bps}}{10^{8}} \right\rfloor$$

The remainder of the sell fee goes to `protocol_fee_lamports`. Gross output always equals net user output plus compounded SOL plus the protocol fee, with no rounding drift.

---

## 5. Verification Gates (G01–G10)

Each gate is labeled by the kind of evidence behind it:
- **Measured:** observed by running the compiled `.so` in LiteSVM.
- **Enforced:** an on-chain check that rejects violating transactions.
- **Arithmetic:** a derived figure checked by the harness.
- **Simulated:** output of the economic model in `audit-harness/src/sim.rs`.

| Gate | Check | Result | Evidence |
| :--- | :--- | :--- | :--- |
| G01 | Lamport conservation | 0 lamport drift over 10 SOL of simulated swaps; on-chain `ConservationViolation` check | Simulated + Enforced |
| G02 | Swap compute budget | 35,069 CU peak (43,000 CU budget, 200,000 CU limit) | Measured |
| G03 | Single-lane throughput | 279 swaps/slot at 43,000 CU against the 12M CU per-account limit | Arithmetic |
| G04 | Rent exactness | 8,940,800 L (Step 2) / 1,224,960 L (Step 5) | Arithmetic |
| G05 | CPI depth & accounts | CPI depth 2; 28 accounts on the full swap instruction | Static |
| G06 | Vault solvency | `max_extractable <= real_vault` (benchmark config: 85.0 ≤ 86.5 SOL) | Enforced |
| G07 | Slot-0 sniper share | 15 SOL sniper: 4,280 → 1,850 bps of supply vs. a 1% flat-fee baseline | **Simulated** |
| G08 | Wash-trade fee leak | Insider leak 9,084–9,996 → 80 bps | **Simulated** |
| G09 | Sell impact ceiling | Sells above 700 bps rejected (`6007`) | Enforced + Measured |
| G10 | Graduation dead zone | Graduation and a swap succeed in the same slot | Measured |

> [!NOTE]
> "Audit" in `audit-harness` means automated verification, not a third-party security audit.

---

## 6. Repository Map & Quickstart

```text
.
├── .gitignore                       # Build outputs, .env*, *-keypair.json, local launch/devnet configs & reports
├── README.md
├── deploy/
│   ├── launch.config.example.json   # Public template: branding, vault sizing, RPC
│   ├── launch.config.json           # Local mainnet config (git-ignored)
│   ├── launch.devnet.json           # Local Devnet rehearsal config (git-ignored)
│   └── ratchet_dlmm_damm-keypair.json  # Local program keypair (git-ignored)
├── docs/
│   ├── TOKEN_CONCEPT_SPEC.md        # Formal spec, byte-offset maps, overflow analysis
│   ├── ROADMAP_AND_ITERATION_PLAN.md
│   └── MAINNET_LAUNCH_RUNBOOK.md    # Preflight, Devnet rehearsal, mainnet launch
├── programs/ratchet-dlmm-damm/      # On-chain program
│   └── src/
│       ├── lib.rs                   # declare_id!, module exports, unit tests
│       ├── entrypoint.rs
│       ├── instruction.rs           # 0x01–0x07 wire formats & PDA seeds
│       ├── processor.rs             # Instruction handlers (12- and 28-account swaps)
│       ├── cpi.rs                   # System / SPL Token / Token-2022 CPI wrappers
│       ├── state.rs                 # RatchetPoolPda, ActiveBinArrayPda, rent helpers
│       ├── math.rs                  # Q64.64 math, fee functions, swap engine
│       ├── compounding.rs           # Sell-fee split & floor-bin routing
│       ├── graduation.rs            # Solvency check & in-place graduation
│       └── error.rs                 # RatchetError (custom codes 6000–6015)
├── tests/litesvm-e2e/               # Compiled .so + Token-2022 BPF integration tests
├── audit-harness/                   # Deterministic gate checks & simulations
└── sdk/
    ├── jupiter-adapter/             # Jupiter Amm-trait quoting crate
    └── ts-client/
        ├── test.mjs
        └── src/
            ├── feePreview.ts        # bigint fee previews
            ├── accounts.ts          # PDA derivation, 28-account resolver
            ├── genesisBundle.ts     # Instruction packers
            ├── solanaTx.ts          # Genesis transaction builder (<= 1,232 B packets), config loader, decoders
            └── cli/
                ├── preflight.ts     # npm run mainnet:preflight  (read-only)
                ├── launch.ts        # npm run mainnet:launch     (spends SOL)
                └── devnetE2e.ts     # npm run devnet:e2e         (12-stage live Devnet suite)
```

### Verification commands

```bash
# Program unit tests
(cd programs/ratchet-dlmm-damm && cargo test)

# Build the .so (Linux/macOS, or Docker/WSL on Windows)
(cd programs/ratchet-dlmm-damm && cargo-build-sbf)

# LiteSVM end-to-end tests against the compiled .so
(cd tests/litesvm-e2e && cargo test -- --nocapture --test-threads=1)

# Gate checks + JSON/Markdown reports
(cd audit-harness && cargo test && cargo run)

# Jupiter adapter
(cd sdk/jupiter-adapter && cargo test)

# TypeScript SDK (use npm.cmd on Windows PowerShell if script execution is restricted)
(cd sdk/ts-client && npm install && npm test)

# Live Devnet rehearsal (after deploying the .so to Devnet; see the runbook)
(cd sdk/ts-client && npm run devnet:e2e -- --keypair <DEVNET_KEYPAIR_JSON>)
```

---

## 7. Mainnet Readiness

| Item | Value |
| :--- | :--- |
| Program ID | `5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5` |
| Binary | `programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so` (192,032 B, SHA-256 `E39D554DA42CFC1CACE132777506B396ABDA05C6C227BF84635084097249C194`) |
| Mainnet deployment | None |
| Devnet rehearsal | Suite implemented (`npm run devnet:e2e`); not yet run |
| Third-party audit | None |

**Last recorded results:**
- Program unit tests: 13/13.
- Clippy: 0 warnings.
- LiteSVM end-to-end: 2/2.
- `sdk/jupiter-adapter`: 6/6.
- `audit-harness`: 10/10 gates.

`sdk/ts-client` passed before the 30-bin genesis calibration change, but it has to be re-run after that change.

The full procedure (branding, preflight, Devnet rehearsal, deploy, launch) is in the [Mainnet Launch Runbook](docs/MAINNET_LAUNCH_RUNBOOK.md). In short:

```bash
cd sdk/ts-client
npm run mainnet:preflight                         # read-only; never signs or broadcasts
npm run devnet:e2e -- --keypair <DEVNET_KEYPAIR>  # full live rehearsal on Devnet
npm run mainnet:launch -- --confirm-mainnet-launch  # spends real SOL; irreversible
```

---

## 8. Documentation

- [Mainnet Launch Runbook](docs/MAINNET_LAUNCH_RUNBOOK.md): configuration, preflight, Devnet rehearsal, launch commands, evidence, and known risks.
- [Technical Concept Specification](docs/TOKEN_CONCEPT_SPEC.md): mechanism details, byte-offset maps, overflow analysis, and threat models.
- [Roadmap](docs/ROADMAP_AND_ITERATION_PLAN.md): milestone status, pre-mainnet checklist, and post-genesis plans.

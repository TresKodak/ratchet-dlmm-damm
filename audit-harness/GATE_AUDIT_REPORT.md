# Ratchet-DLMM-DAMM Phase 4 Deterministic 10-Gate Audit Report

- **Protocol**: `Ratchet-DLMM-DAMM`
- **Harness Crate**: `ratchet_audit_harness`
- **Audit Pass Rate**: **1.00** (10 / 10 gates PASS)
- **Core Invariants Verified**: `true`

## 1. 10-Gate Verification Summary

| Gate ID | Status | Specification Threshold | Observed Result |
| :--- | :---: | :--- | :--- |
| `G01_lamport_token_conservation` | **PASS** | LHS == RHS == 10,000,000,000 L; error == 0 | LHS = 10000000000 L, RHS = 10000000000 L, error = 0 L (swaps = 200, token_error = 0) |
| `G02_single_tx_cu_budget` | **PASS** | <= 200,000 CU ceiling (<= 43,000 CU model; verified ELF + LiteSVM CU) | Model 43000 CU / 200000 CU (21.5%); LiteSVM max 35069 CU (buy Some(35069), sell Some(31605)); .so 192032 B ELF64 BPF=true |
| `G03_write_lock_slot_throughput` | **PASS** | 12,000,000 CU/slot @ 43,000 CU/tx == 279 txs/slot | 279 txs/slot (11997000 CU consumed, 3000 CU headroom) |
| `G04_simd0437_rent_exactness` | **PASS** | Step 2 = 8,940,800 L (3,088,640 + 5,852,160); Step 5 = 1,224,960 L (423,168 + 801,792) | Step 2 = 8940800 L (3088640 + 5852160); Step 5 = 1224960 L (423168 + 801792) |
| `G05_cpi_depth_and_account_limit` | **PASS** | CPI depth = 2 (<= 4); max accounts = 28 (<= 255 SIMD-0339) | CPI depth = 2 (<= 4), accounts = 28 (<= 255) |
| `G06_vault_solvency_no_hollow_grad` | **PASS** | Max extractable 85,000,000,000 L <= Real vault 86,500,000,000 L | Max extractable 85000000000 L <= Real vault 86500000000 L (surplus 1500000000 L; hollow grad rejected = true) |
| `G07_slot0_sniper_corner_mitigation` | **PASS** | Supply capture reduced from 4,280 bps (42.80%) to 1,850 bps (18.50%) | Baseline = 4280 bps (42.80%) -> Ratchet = 1850 bps (18.50%) [v_a^+ = 180, peak f_buy = 8587 bps] |
| `G08_wash_fee_siphon_suppression` | **PASS** | Creator fee leak compressed from 9,084-9,996 bps down to 80 bps | Baseline = 9084-9996 bps -> Ratchet = 80 bps (all cohorts negative EV = true) |
| `G09_sell_impact_ceiling_750bps` | **PASS** | Max single-leg sell impact == 700 bps (<= 700 buffered / 750 hard cap) | Max single-leg impact = 700 bps (unconstrained would be 1902 bps; overshoot rejected = true) |
| `G10_zero_dead_zone_graduation` | **PASS** | dead_zone_slots == 0; partial_migration_errors == 0 | dead_zone_slots = 0, partial_migration_errors = 0, status 0 -> 1, authority_revoked = 1 |

## 2. Core Invariant Math & `u128` Overflow Safety

- **High-Water Buy Ratchet `f_buy(t)`**: Slot-0 `8000 bps` (80.00%) -> Asymptotic `25 bps` (0.25%) -> Surge w/ `v_a^+`: `683 bps` (monotonic `sqrt_P_high_water`: `true`)
- **Asymmetric 5x Dump-Brake `f_sell(t)`**: Calm `25 bps` -> Mid-dump `674 bps` -> Hard-capped `2500 bps` (25.00%)
- **OnlyB Quote Auto-Compounding (`Delta_R_sol_compounded = 0.75 * f_sell * Gross_SOL_Out`)**: On `4000000000` L gross sell at `2500 bps`, `total_fee = 1000000000 L`, `Delta_R_sol_compounded = 750000000 L`, `residual_fee = 250000000 L`, `conservation_error = 0 L`
- **`u128` Overflow Safety (`(v_a^- * s)^2 * 5 * A_buy / 10^11`)**: Peak domain numerator (`v_a^- = 1000000000`, `s = 10000`, `A_buy = 1000000`) = `500000000000000000000000000000000` < `u128::MAX` (`340282366920938463463374607431768211455`), leaving **19 bits** of headroom; `5000` deterministic fuzz cases run with **0 silent wraps** and `2500` safe checked overflow rejections on extreme `u64` stress inputs.

## 3. sBPF Hot-Path Compute & Write-Lock Slot Profile (G02 & G03)

| Stage | Compute Units (CU) |
| :--- | ---: |
| Account validation & PDA bump checks (28 accounts) | 4200 |
| Zero-copy cast (RatchetPoolPda 480B + ActiveBinArrayPda 1,024B) | 1800 |
| Directional accumulator (v_a^+/v_a^-) & high-water sqrt(P) math | 6500 |
| Discrete bin array traversal & 700 bps impact guardrail | 11400 |
| OnlyB 75% quote auto-compounding & floor-bin reserve update | 3100 |
| Token-2022 TransferChecked CPI (depth 2, TransferFeeConfig check) | 13500 |
| Zero-copy state writeback & swap telemetry emission | 2500 |
| **Total Hot-Path Swap CU** | **43000** (21.5% of 200000 CU ceiling) |
| **Single Write-Lock Lane Capacity** | **279 txs/slot** (`12000000 / 43000`, 3000 CU remainder) |

## 4. SIMD-0437 Rent Exactness & Account Budget (G04 & G05)

| PDA Account | Data Bytes | Overhead | Total Bytes | Step 2 (5,080 L/B) | Step 5 (696 L/B) |
| :--- | ---: | ---: | ---: | ---: | ---: |
| `ratchet_pool_pda` | 480 | 128 | 608 | 3088640 L | 423168 L |
| `active_bin_array_pda` | 1024 | 128 | 1152 | 5852160 L | 801792 L |
| **Combined Total** | **1504** | **256** | **1760** | **8940800 L** | **1224960 L** |

- **CPI Call Depth**: `2` (<= `4` Solana limit)
- **Swap Account Count**: `28` unique metas (<= `255` SIMD-0339 limit)

## 5. Economic & Attack-Mitigation Simulations (G01, G06-G10)

- **G01 Conservation**: `LHS = 10000000000 L`, `RHS = 10000000000 L`, `error = 0 L` across `200` odd-lamport swaps (`token_error = 0`).
- **G06 Vault Solvency**: `max_extractable = 85000000000 L <= real_vault = 86500000000 L` (`surplus = 1500000000 L`), hollow graduation attempt rejected = `true`.
- **G07 Slot-0 Whale Sniper (15 SOL)**: Baseline `4280 bps` (42.80%) -> Ratchet `1850 bps` (18.50%), `v_a^+` climbed to `180`, peak `f_buy = 8587 bps`.
- **G08 Wash-Fee Siphon Suppression**: Baseline leak `9084-9996 bps` compressed down to `80 bps`; all `5` self-churn cohorts finished with negative EV (`unprofitable = true`).
- **G09 Single-Leg Sell Impact Ceiling**: Clamped at `700 bps` (`<= 700 bps` buffered / `<= 750 bps` hard cap); unconstrained sweep would cause `1902 bps` impact (strict overshoot rejected = `true`).
- **G10 Atomic In-Place Graduation**: Transition `status 0 -> 1` & `authority_revoked = 1` inside slot `314159265` with `dead_zone_slots = 0` and `partial_migration_errors = 0`.

## 6. Compiled `.so` Binary & LiteSVM E2E Compute Parity

- **Artifact**: `C:/Users/tresk/Downloads/token/programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so` (`192032 bytes`, ELF64 = `true`, `e_machine = 247` BPF = `true`, `entrypoint` = `true`)
- **SHA-256**: `E39D554DA42CFC1CACE132777506B396ABDA05C6C227BF84635084097249C194`
- **`declare_id!` & Cross-Repo Consistency**: `lib.rs = 5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5`, `keypair = 5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5`, `jupiter-adapter = 5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5`, `ts-client = 5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5`, `launch.config.json = 5RMfUhMouhtjayxyWNSmzvAtC8fo5XLo8u7eRH9jwNt5` (`consistent = true`, `passed = true`)
- **LiteSVM Measured Swap CU** (`audit-harness/measured_cu.json`): `buy_swap_cu = Some(35069)`, `sell_swap_cu = Some(31605)`, `max_swap_cu = 35069 CU` (`<= 43,000 CU` model: `true`, `<= 200,000 CU` ceiling: `true`)

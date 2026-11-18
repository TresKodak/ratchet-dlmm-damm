# Ratchet-DLMM-DAMM: Technical Concept Specification

> **Protocol Class**: Hybrid Discrete-Bin AMM & Dynamic Anti-Extraction Token Launch / Trading Engine
> **Verification**: 10 deterministic verification gates (G01–G10). Each gate's evidence type (measured, enforced, arithmetic, or simulated) is listed in §6.
> **Status**: Not deployed to mainnet. Not independently audited.

---

## 1. Architectural Synthesis

**Ratchet-DLMM-DAMM** is designed to mitigate the main structural failure modes of flat-fee bonding curves and legacy token launchpads—Slot-0 whale cornering, insider wash-trade fee siphoning, cascading single-slot sell slippage, hollow under-collateralized graduations, and external pool migration dead zones—by uniting three complementary Solana AMM mechanisms into a single zero-copy sBPF program:

1. **Meteora DLMM Discrete Bin Accumulator**:
   - Liquidity is quantized into discrete price bins inside a fixed `1,024`-byte `ActiveBinArrayPda`, tracking directional bin crossings separately for upward momentum ($v_a^+$) and downward sell pressure ($v_a^-$).
   - Introduces a **$5\times$ asymmetric dump-brake** ($A_{\text{sell}} = 5 \cdot A_{\text{buy}}$) so rapid downward bin sweeps dynamically spike sell fees up to a hard `25%` (`2,500 bps`) cap without penalizing organic buyers.
2. **Meteora DAMM v2 $\sqrt{P}$ Market-Cap Fee Scheduler**:
   - Replaces time-decay-only schedules—which snipers wait out or manipulate—with a **monotonic high-water price ratchet** ($\sqrt{P}_{\max}$).
   - Base buy fees open at **8,000 bps (80%)** at Slot 0. They decay geometrically, by a factor of γ for each bin step the high-water √P climbs above the opening √P, toward an asymptotic floor of **25 bps (0.25%)**. Downward price movement never resets the ratchet.
   - **Reachable range:** With the defaults (γ = 0.9) and the bundled 30-bin geometry (bins `-8..=21`, graduation at +12 steps), the base buy fee is about **2,259 bps at graduation** and about **875 bps at the highest seeded bin**. Reaching the 25 bps floor takes about 55 steps, which a single bin array can't provide. See §3.1.
3. **OnlyB Quote Compounding, In-Place Graduation & Write-Lock Isolation**:
   - **OnlyB Quote Auto-Compounding**: `75%` (`compounding_fee_bps = 7500`) of every sell fee is routed atomically in-instruction into the active floor bin's quote SOL reserve ($R_{\text{sol}}$), turning sell pressure directly into deeper exit liquidity.
   - **Zero-Dead-Zone In-Place Graduation**: Replaces multi-transaction external AMM migrations with an atomic `graduation_status` state flip inside `RatchetPoolPda` (`dead_zone_slots = 0`, eliminating `PartialMigrationError`).
   - **Write-Lock-Free Token-2022 Backup Harvesting**: Pairs the core AMM pool with `ExtensionType::TransferFeeConfig` on the Token-2022 mint while keeping fee harvesting decoupled from the hot swap write lock.
   - **SIMD-0437 Compact PDA Packing**: Packs pool header (`480` data bytes) and active bin array (`1,024` data bytes) into two deterministic zero-copy accounts minimizing Solana rent under both Step 2 and Step 5 regimes.

### Core Protocol Primitives

| Component | Subsystem | Architectural Primitive | Primary Invariant / Threshold | Target Gates |
| :--- | :--- | :--- | :--- | :--- |
| **Compact PDA Layout** | State | Zero-Copy PDA Packing & SIMD-0437 Rent | `480 B` pool + `1,024 B` bin array (`8,940,800 L` Step 2 / `1,224,960 L` Step 5) | `G04`, `G05` |
| **High-Water Ratchet** | DAMM Scheduler | $\sqrt{P}_{\max}$ High-Water Fee Decay | Monotonic $\sqrt{P}_{\max}$; base fee $8000 \cdot \gamma^{n}$ bps with asymptotic floor 25 bps (≈875 bps at the top seeded bin) | `G07`, `G08` |
| **Directional Accumulators** | DLMM Bins | Split $v_a^+$ vs. $v_a^-$ Counters | Separate upward vs. downward discrete bin-crossing counters | `G07`, `G08` |
| **Asymmetric Dump-Brake** | DLMM Bins | $5\times$ Downward Volatility Penalty | $A_{\text{sell}} = 5 \cdot A_{\text{buy}}$; immutable hard cap $f_{\max,\text{sell}} = 2,500\text{ bps}$ | `G08` |
| **OnlyB Compounding** | Fee Routing | `75%` Floor Quote Auto-Compounding | $\Delta R_{\text{sol}} = \lfloor \text{Gross\_SOL\_Out} \cdot f_{\text{sell}}(t) \cdot \text{compounding\_fee\_bps} / 10^{8} \rfloor$; $0$-lamport drift | `G01`, `G06` |
| **Sell Impact Guardrail** | Risk Control | Pre-Swap Single-Leg Impact Cap | Max single-leg sell price impact `<= 700 bps` (within `750 bps` cap) | `G09` |
| **Solvency & Graduation** | Lifecycle | Vault Solvency & In-Place Atomic Flip | `85.0 SOL <= 86.5 SOL` extractable bound; `0` dead-zone slots | `G06`, `G10` |
| **Hot-Lane Isolation** | Token-2022 | Write-Lock-Free Backup Harvesting | `43,000 CU` budget (`35,069 CU` measured), `279 txs/slot`, CPI `2`, `28` accts | `G02`, `G03`, `G05` |

---

## 2. PDA Memory Layout, Byte Offsets & Token-2022 Mechanics

### 2.1 SIMD-0437 Account Sizing & Rent Schedule (`G04`)

Under Solana SIMD-0437, each on-chain account carries **`128` bytes** of runtime account metadata overhead in addition to its account data buffer:

| Account | Data Bytes | Metadata Overhead | Total Billable Bytes | Step 2 Rent (`5,080 L/B`) | Step 5 Rent (`696 L/B`) |
| :--- | :---: | :---: | :---: | :---: | :---: |
| `ratchet_pool_pda` (`RatchetPoolPda`) | `480` | `128` | `608` | **`3,088,640 L`** | **`423,168 L`** |
| `active_bin_array_pda` (`ActiveBinArrayPda`) | `1,024` | `128` | `1,152` | **`5,852,160 L`** | **`801,792 L`** |
| **Combined 2-PDA Footprint** | **`1,504`** | **`256`** | **`1,760`** | **`8,940,800 L`** | **`1,224,960 L`** |

- **Step 2 Total**: $608 \times 5,080 + 1,152 \times 5,080 = 3,088,640 + 5,852,160 = \mathbf{8,940,800\text{ L}}$ ($0.0089408\text{ SOL}$).
- **Step 5 Total**: $608 \times 696 + 1,152 \times 696 = 423,168 + 801,792 = \mathbf{1,224,960\text{ L}}$ ($0.00122496\text{ SOL}$), an **$86.30\%$ reduction** ($7,715,840\text{ L}$ reclaimed per pool).

### 2.2 Zero-Copy Byte-by-Byte Offset Maps (`#[repr(C)]` `Pod` + `Zeroable`)

All fields in `RatchetPoolPda`, `LiquidityBin`, and `ActiveBinArrayPda` are ordered strictly by descending natural alignment (`u128` $\to$ `u64`/`i64` $\to$ `i32`/`u32` $\to$ `u16` $\to$ `u8` $\to$ `[u8; 32]`) with explicit zero padding, guaranteeing zero implicit compiler padding and sound `bytemuck::try_from_bytes` zero-copy casting directly from Solana account data slices (`programs/ratchet-dlmm-damm/src/state.rs`).

#### `RatchetPoolPda` — Exact `480`-Byte Offset Map (`align = 16`, `608` Total Bytes w/ SIMD-0437)

| Byte Offset | Size (B) | Field Name | Rust Type | Purpose & Invariant |
| :--- | :---: | :--- | :--- | :--- |
| `0..16` | `16` | `sqrt_p_high_water` | `u128` | Monotonic high-water $\sqrt{P}_{\max}$ in Q64.64 fixed point |
| `16..32` | `16` | `sqrt_p_current` | `u128` | Current pool $\sqrt{P}$ in Q64.64 fixed point |
| `32..48` | `16` | `sqrt_p_start` | `u128` | Genesis Slot-0 baseline $\sqrt{P}$ in Q64.64 fixed point |
| `48..64` | `16` | `sqrt_p_graduation` | `u128` | Target $\sqrt{P}$ threshold enabling atomic in-place graduation |
| `64..72` | `8` | `v_a_up` | `u64` | Upward directional volatility accumulator (bin crossings $v_a^+$) |
| `72..80` | `8` | `v_a_down` | `u64` | Downward directional volatility accumulator (bin crossings $v_a^-$) |
| `80..88` | `8` | `a_buy` | `u64` | Buy-side volatility fee coefficient $A_{\text{buy}}$ |
| `88..96` | `8` | `a_sell` | `u64` | Sell-side dump-brake coefficient $A_{\text{sell}} = 5 \cdot A_{\text{buy}}$ |
| `96..104` | `8` | `last_decay_timestamp` | `i64` | Unix timestamp of last volatility accumulator decay update |
| `104..112` | `8` | `real_vault_lamports` | `u64` | Physical SOL lamports held in pool quote vault (`86,500,000,000 L` bench) |
| `112..120` | `8` | `max_extractable_lamports` | `u64` | Maximum SOL extractable against bin reserves (`85,000,000,000 L` bench) |
| `120..128` | `8` | `compounded_sol_total` | `u64` | Cumulative quote SOL lamports auto-compounded into active floor bin |
| `128..136` | `8` | `protocol_fee_lamports` | `u64` | Accumulated non-compounded protocol/creator SOL fee lamports |
| `136..144` | `8` | `graduation_slot` | `u64` | Slot number at which in-place graduation occurred (`0` pre-graduation) |
| `144..152` | `8` | `total_volume_quote` | `u64` | Cumulative quote volume in lamports |
| `152..160` | `8` | `harvested_transfer_fee_atoms` | `u64` | Token-2022 `TransferFeeConfig` fallback fee atoms harvested out-of-band |
| `160..164` | `4` | `active_bin_id` | `i32` | Current active discrete bin ID |
| `164..168` | `4` | `floor_bin_id` | `i32` | Active floor bin ID receiving OnlyB auto-compounded quote reserves |
| `168..172` | `4` | `bin_array_start_id` | `i32` | Starting bin ID mapped to `ActiveBinArrayPda.bins[0]` |
| `172..176` | `4` | `decay_period_seconds` | `u32` | Horizon in seconds after which accumulators decay to `0` (`600` s default) |
| `176..178` | `2` | `compounding_fee_bps` | `u16` | Share of sell fee routed into floor bin (immutable min `7500` = `75%`) |
| `178..180` | `2` | `f_0_bps` | `u16` | Initial Slot-0 base buy fee (`8000` = `80%`) |
| `180..182` | `2` | `f_min_bps` | `u16` | Minimum steady-state fee floor (`25` = `0.25%`) |
| `182..184` | `2` | `f_max_sell_bps` | `u16` | Immutable maximum sell fee ceiling (`2500` = `25%`) |
| `184..186` | `2` | `bin_step_bps` | `u16` | Discrete price step size $s$ between adjacent bins in bps |
| `186..188` | `2` | `max_sell_impact_bps` | `u16` | Buffered single-leg sell impact limit (default `700` `<= 750` hard cap) |
| `188..190` | `2` | `gamma_bps` | `u16` | High-water ratchet decay multiplier $\gamma$ per step in bps (`9000` default) |
| `190..192` | `2` | `transfer_fee_bps` | `u16` | Fallback Token-2022 `TransferFeeConfig` rate in bps |
| `192..193` | `1` | `graduation_status` | `u8` | `0` = active bonding curve (`GRADUATION_STATUS_ACTIVE_CURVE`), `1` = graduated in-place |
| `193..194` | `1` | `authority_revoked` | `u8` | `0` = mutable genesis authority, `1` = permanently revoked |
| `194..195` | `1` | `vault_bump` | `u8` | PDA bump seed for quote/base vaults |
| `195..196` | `1` | `pool_bump` | `u8` | PDA bump seed for `RatchetPoolPda` |
| `196..197` | `1` | `bin_array_bump` | `u8` | PDA bump seed for `ActiveBinArrayPda` |
| `197..198` | `1` | `version` | `u8` | State layout version (`1`) |
| `198..199` | `1` | `transfer_fee_enabled` | `u8` | `1` if Token-2022 `TransferFeeConfig` fallback harvesting is active |
| `199..200` | `1` | `filter_period_seconds` | `u8` | Filter window before accumulator decay engages (`30` s default) |
| `200..232` | `32` | `base_mint` | `[u8; 32]` | Base Token-2022 mint pubkey |
| `232..264` | `32` | `quote_mint` | `[u8; 32]` | Quote mint pubkey (WSOL) |
| `264..296` | `32` | `base_vault` | `[u8; 32]` | Pool base token vault pubkey |
| `296..328` | `32` | `quote_vault` | `[u8; 32]` | Pool quote SOL vault pubkey |
| `328..360` | `32` | `authority` | `[u8; 32]` | Pool authority pubkey (zeroed `[0; 32]` upon revocation) |
| `360..392` | `32` | `active_bin_array` | `[u8; 32]` | Linked `ActiveBinArrayPda` pubkey |
| `392..424` | `32` | `creator_fee_vault` | `[u8; 32]` | Creator/protocol fee destination pubkey |
| `424..480` | `56` | `padding` | `[u8; 56]` | Explicit zero padding to `480` bytes (`480 % 16 == 0`); `padding[0..2]` caches `PoolAuthorityPda` bump (`padding[0] = bump`, `padding[1] = 1`) for $O(1)$ PDA verification |

#### `ActiveBinArrayPda` — Exact `1,024`-Byte Offset Map (`align = 16`, `1,152` Total Bytes w/ SIMD-0437)

| Byte Offset | Size (B) | Field Name | Rust Type | Purpose & Invariant |
| :--- | :---: | :--- | :--- | :--- |
| `0..16` | `16` | `bin_step_q64` | `u128` | Multiplicative Q64.64 step $\sqrt{1 + \text{bin\_step\_bps} / 10,000}$ |
| `16..48` | `32` | `pool` | `[u8; 32]` | Parent `RatchetPoolPda` pubkey |
| `48..52` | `4` | `active_bin_id` | `i32` | Current active bin index |
| `52..56` | `4` | `floor_bin_id` | `i32` | Active floor bin index receiving OnlyB auto-compounded SOL |
| `56..60` | `4` | `start_bin_id` | `i32` | Lowest bin index represented by `bins[0]` |
| `60..62` | `2` | `bin_step_bps` | `u16` | Discrete price step size in bps |
| `62..63` | `1` | `bump` | `u8` | PDA bump seed |
| `63..64` | `1` | `version` | `u8` | State layout version (`1`) |
| `64..1024` | `960` | `bins` | `[LiquidityBin; 30]` | `30` packed `32`-byte discrete liquidity bins (`30 * 32 = 960` B) |

Each **`LiquidityBin` (`32` bytes, `align = 16`)** element at `64 + idx * 32` has internal layout:

| Relative Offset | Size (B) | Field Name | Rust Type | Purpose |
| :--- | :---: | :--- | :--- | :--- |
| `+0..16` | `16` | `sqrt_price_q64` | `u128` | Fixed bin $\sqrt{P}$ in Q64.64 fixed point |
| `+16..24` | `8` | `reserve_quote_lamports` | `u64` | Quote SOL lamports held in this bin |
| `+24..32` | `8` | `reserve_base_atoms` | `u64` | Base token atoms held in this bin |

### 2.3 Token-2022 `ExtensionType::TransferFeeConfig` Write-Lock-Free Backup Harvesting

A frequent failure mode in Token-2022 AMM designs is forcing every swap instruction to transfer withheld token fees into a single mutable global fee vault ATA. That introduces a secondary global write lock on every transaction and bloats CU consumption.

Ratchet-DLMM-DAMM resolves this via **Write-Lock-Free Backup Harvesting**:

1. **Primary Rail (In-Instruction SOL/Quote Compounding)**: Normal AMM swaps assess `f_buy(t)` and `f_sell(t)` directly inside the AMM swap math (`43,000 CU`), keeping swap execution isolated on the single pool write-lock lane.
2. **Backup Rail (`ExtensionType::TransferFeeConfig` — `108`-Byte SPL Wire Payload)**: The token mint initializes `TransferFeeConfig` (`[0..32]` `transfer_fee_config_authority`, `[32..64]` `withdraw_withheld_authority`, `[64..72]` `withheld_amount`, `[72..90]` `older_transfer_fee` [18 B], `[90..108]` `newer_transfer_fee` [18 B]). Direct external Token-2022 transfers accrue withheld fees inside per-account `TransferFeeAmount` extensions (`ExtensionType::TransferFeeAmount`).
3. **Hot-Lane Write-Lock Isolation**:
   - Swaps only write-lock the pool accounts and trader ATAs—never touching `TransferFeeConfig` mint state or a global Token-2022 fee-harvest sink.
   - In `programs/ratchet-dlmm-damm/src/state.rs`, `TransferFeeConfig::harvest_withheld_tokens_to_mint` deliberately takes no borrow on `RatchetPoolPda`, allowing permissionless asynchronous harvesting (`G02` = `43,000 CU`, `G03` = `279 txs/slot`, `G05` = CPI depth `2 <= 4`, `28 <= 255` accounts).
4. **Batched Settlement**: Off-critical-path cranks invoke `harvest_withheld_tokens_to_mint` to sweep per-account withheld fees to the mint, followed by authorized batched `withdraw_withheld_tokens_from_mint` updating `pool.harvested_transfer_fee_atoms` (`offset 152..160`).

---

## 3. Core On-Chain Invariant Equations

### 3.1 High-Water Buy Ratchet (`f_buy(t)`)

On every swap or state update at time/slot $t$, the monotonic high-water mark updates first:
$$\sqrt{P}_{\max}(t) = \max\left(\sqrt{P}_{\max}(t-1),\ \sqrt{P}_{\text{current}}(t)\right)$$

The dynamic buy fee $f_{\text{buy}}(t)$ (in basis points, `1 bps = 0.01%`) is the sum of the market-cap-ratcheted base fee and the upward volatility accumulator penalty:
$$f_{\text{buy}}(t) = \max\left(f_{\min},\ f_0 \cdot \gamma^{\text{steps\_above}(\sqrt{P}_{\max})}\right) + \left\lceil \frac{A_{\text{buy}} \cdot (v_a^+ \cdot s)^2}{10^{11}} \right\rceil$$

Where:
- $f_0 = 8,000\text{ bps}$ ($80\%$) is the Slot-0 anti-sniper initial base fee.
- $f_{\min} = 25\text{ bps}$ ($0.25\%$) is the steady-state floor fee once the market-cap ratchet matures.
- $\gamma \in (0, 1)$ (`gamma_bps / 10,000`, default 0.9) is the geometric decay factor per bin step.
- $\text{steps\_above}(\sqrt{P}_{\max})$ is the integer number of bin steps by which $\sqrt{P}_{\max}$ exceeds $\sqrt{P}_{\text{start}}$ (`steps_above_high_water` in `math.rs`).
- $v_a^+$ (`v_a_up`) tracks cumulative upward discrete bin crossings inside the active decay window.
- $s$ (`bin_step_bps`) is the discrete bin step width in bps.
- $A_{\text{buy}}$ is the upward volatility sensitivity coefficient.

**Key Property**: If snipers push price up and then dump, $\sqrt{P}_{\max}$ stays latched at the peak. Snipers can't reset the pool to Slot-0 conditions and re-enter cheaply, and later buyers pay the lower fee the ratchet has already earned.

**Reachable fee range (defaults: $f_0 = 8000$, $\gamma = 0.9$, $f_{\min} = 25$):**

| Steps above open ($n$) | 0 | 4 | 8 | 12 (graduation) | 16 | 21 (top seeded bin) | ≥ 55 |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Base buy fee (bps) | 8,000 | ≈5,249 | ≈3,444 | ≈2,259 | ≈1,482 | ≈875 | 25 |

A single `ActiveBinArrayPda` holds 30 bins. The bundled genesis builder seeds bins `-8..=21` and sets the graduation threshold at +12 steps, so the 25 bps floor is **not reachable** with that geometry. It would need about 55 steps, which means either sibling bin arrays (§4.3 of the roadmap) or a smaller γ. Values are rounded; the program computes $\gamma^{n}$ in Q64.64 with round-to-nearest.

### 3.2 Asymmetric $5\times$ Dump-Brake (`f_sell(t)`)

Downward bin crossings ($v_a^-$, `v_a_down`) are tracked independently from $v_a^+$ and weighted with $5\times$ higher sensitivity ($A_{\text{sell}} = 5 \cdot A_{\text{buy}}$), subject to an immutable 2,500 bps (25%) ceiling:
$$f_{\text{sell}}(t) = \min\left(2500,\ f_{\text{base}} + \left\lceil \frac{5 \cdot A_{\text{buy}} \cdot (v_a^- \cdot s)^2}{10^{11}} \right\rceil\right)$$

Here $f_{\text{base}} = f_{\min}$ (`f_min_bps`, 25 bps by default; see `quote_sell` in `math.rs`), and every term is in bps. The 2,500 bps ceiling means a sell can never be charged more than 25%, however high volatility gets.

### 3.3 OnlyB Quote Auto-Compounding (`Delta_R_sol_compounded`)

For every sell transaction outputting `Gross_SOL_Out` lamports prior to fees (`programs/ratchet-dlmm-damm/src/compounding.rs`), let `Total_Fee_SOL` be the exact integer ceiling fee extracted at rate $f_{\text{sell}}(t)$:
$$\text{Total\_Fee\_SOL} = \min\left(\text{Gross\_SOL\_Out},\ \left\lceil \frac{\text{Gross\_SOL\_Out} \cdot f_{\text{sell}}(t)}{10,000} \right\rceil\right)$$
$$\Delta R_{\text{sol, compounded}} = \left\lfloor 0.75 \cdot f_{\text{sell}}(t) \cdot \text{Gross\_SOL\_Out} \right\rfloor = \left\lfloor \frac{\text{Gross\_SOL\_Out} \cdot f_{\text{sell}}(t) \cdot \text{compounding\_fee\_bps}}{10^{8}} \right\rfloor$$
$$\text{Protocol\_Fee\_SOL} = \text{Total\_Fee\_SOL} - \Delta R_{\text{sol, compounded}}, \qquad \text{Net\_User\_SOL\_Out} = \text{Gross\_SOL\_Out} - \text{Total\_Fee\_SOL}$$

- $\Delta R_{\text{sol, compounded}}$ is deposited **atomically in the same swap instruction** (`route_compounded_sol_to_floor_bin`) directly into `ActiveBinArrayPda.floor_bin_mut().reserve_quote_lamports`.
- **Zero-Lamport Conservation (`G01`)**:
  $$\text{LHS} = \text{Gross\_SOL\_Out} = \text{Net\_User\_SOL\_Out} + \Delta R_{\text{sol, compounded}} + \text{Protocol\_Fee\_SOL} = \text{RHS}$$
  with $\text{error} = |\text{LHS} - \text{RHS}| = 0$ lamports across `10,000,000,000 L` (`10 SOL`, `200` swaps verified in `audit-harness` plus `10,000`-swap fuzz tests). Example at `Gross_SOL_Out = 4,000,000,000 L` and `f_sell = 2,500 bps`: `Total_Fee_SOL = 1,000,000,000 L`, `Delta_R_sol_compounded = 750,000,000 L`, `Protocol_Fee_SOL = 250,000,000 L`, `conservation_error = 0 L`.

### 3.4 Single-Leg Sell Impact Guardrail & Atomic Graduation (`G06`, `G09`, `G10`)

- **Impact Cap (`G09`)**: Before mutating bin reserves, the sell engine evaluates discrete bin traversal price impact $\Delta P / P_0$. Any single-leg sell exceeding **`700 bps` ($7.00\%$)** is rejected pre-mutation, leaving a `50 bps` safety buffer below the `750 bps` architectural ceiling.
- **Solvency Check (`G06`)**: At graduation and on vault withdrawals, the pool enforces:
  $$\text{max\_extractable\_lamports} \le \text{real\_vault\_lamports}$$
  with canonical graduation verification parameters **`85,000,000,000 L` (`85.0 SOL`) `<=` `86,500,000,000 L` (`86.5 SOL`)** (`+1.5 SOL` real surplus).
- **Zero-Dead-Zone Graduation (`G10`)**: Graduation flips `graduation_status` atomically inside `RatchetPoolPda` in-place, requiring **`0` dead-zone slots** and eliminating external migration failure states (`PartialMigrationError`).

---

## 4. Q64.64 Fixed-Point & `u128` Overflow Safety Proofs

### 4.1 Proof of Overflow-Free Surcharge Numerator `(v_a^- * s)^2 * 5 * A_buy / 10^11`

Let `U128_MAX` $= 2^{128} - 1 = 340,282,366,920,938,463,463,374,607,431,768,211,455 \approx 3.4028 \times 10^{38}$, and let $D = 10^{11} = 100,000,000,000$.

**Observation 1 (Typical Operating Magnitude, not a safety bound)**:
While the sell fee is below the 2,500 bps ceiling, the surcharge numerator $N = 5 \cdot A_{\text{buy}} \cdot (v_a^- \cdot s)^2$ satisfies:
$$\left\lceil \frac{N}{10^{11}} \right\rceil \le 2,500 \implies N \le 2,500 \times 10^{11} = 2.5 \times 10^{14}$$
Because $2.5 \times 10^{14} < 2^{48}$, in-range numerators sit about $1.36 \times 10^{24}$ times below `u128::MAX` and would fit in `u64` with about 73,786× headroom. **This observation does not make the computation safe.** The cap is applied *after* $N$ is computed, so it can't bound $N$ in advance. Theorems 2 and 3 carry the safety argument.

**Theorem 2 (General Operational Bound — Verified in `audit-harness`)**:
For bounded domain parameters up to bin step $s \le 10,000\text{ bps}$ ($10^4$), accumulated crossings $v_a^- \le 10^9$, and sensitivity $A_{\text{buy}} \le 10^6$:
1. $v_a^- \cdot s \le 10^{13}$ (fits cleanly in `u64`).
2. $(v_a^- \cdot s)^2 \le 10^{26}$ (fits cleanly in `u128`).
3. $N = 5 \cdot A_{\text{buy}} \cdot (v_a^- \cdot s)^2 \le 5 \times 10^{32} \ll 3.4028 \times 10^{38}$ (`u128::MAX`), retaining **19 bits of headroom** ($> 6.8 \times 10^5\times$ margin) even at maximally stressed domain inputs (`5,000` deterministic fuzz cases verified with `0` silent wraps).

**Theorem 3 (Unbounded Adversarial Fuzz Immunity via Cap-First Short-Circuit)**:
Suppose an adversarial fuzzer supplies arbitrary `u64::MAX` for $v_a^-$ or $A_{\text{buy}}$. If an implementation performs raw `u128` multiplication before clamping, `checked_mul` catches overflow cleanly without UB/wraparound; furthermore, because $f_{\text{sell}}(t)$ is strictly capped at `f_max_sell_bps = 2,500`:
$$\text{If } (v_a^- \cdot s)^2 \ge \left\lceil \frac{(f_{\max,\text{sell}} - f_{\text{base}}) \cdot 10^{11}}{5 \cdot A_{\text{buy}}} \right\rceil \implies f_{\text{sell}}(t) = 2,500\text{ bps immediately.}$$
Even simpler: computing step-by-step with `checked_mul` and saturating/clamping overflow to `f_max_sell_bps` guarantees **zero panics and monotonic correctness over the entire `[0, u64::MAX]` input domain**.

### 4.2 Proof of Overflow-Free Ceiling Division (`div_ceil_u128`)

Naive ceiling division `(n + d - 1) / d` panics or wraps if $n > \text{u128::MAX} - (d - 1)$. Using:
$$\text{div\_ceil\_u128}(n, d) = \begin{cases} 0 & \text{if } n = 0 \\ \lfloor (n - 1) / d \rfloor + 1 & \text{if } n > 0 \end{cases}$$
where $d = 10^{11} > 1$:
- For $n \ge 1$, $0 \le n - 1 < \text{u128::MAX}$ never underflows.
- $\lfloor (n - 1) / d \rfloor \le \lfloor (\text{u128::MAX} - 1) / 10^{11} \rfloor \ll \text{u128::MAX} - 1$, so `+ 1` **never overflows `u128`**.
- Exactness: write $n = q d + r$ with $0 \le r < d$.
  - If $r = 0$ and $q \ge 1$: $n - 1 = (q - 1)d + (d - 1) \implies \lfloor (n - 1) / d \rfloor + 1 = (q - 1) + 1 = q = \lceil n / d \rceil$.
  - If $1 \le r < d$: $n - 1 = q d + (r - 1)$ with $0 \le r - 1 < d - 1 \implies \lfloor (n - 1) / d \rfloor + 1 = q + 1 = \lceil n / d \rceil$. $\blacksquare$

### 4.3 Q64.64 Fixed-Point & 256-Bit Widening (`mul_u128_wide` / `mul_div_u128`)

Q64.64 numbers represent $x \in [0, 2^{64})$ as $X = \lfloor x \cdot 2^{64} \rfloor \in \text{u128}$. Direct `u128` multiplication of two Q64.64 prices $X \cdot Y$ can exceed $2^{128}-1$ whenever $X \cdot Y \ge 2^{128}$. In `programs/ratchet-dlmm-damm/src/math.rs`, `mul_u128_wide(a, b) -> (hi, lo)` decomposes operands into upper 64-bit integer and lower 64-bit fractional limbs to produce an exact 256-bit intermediate:
$$X = x_{\text{hi}} \cdot 2^{64} + x_{\text{lo}}, \quad Y = y_{\text{hi}} \cdot 2^{64} + y_{\text{lo}}$$
$$\left\lfloor \frac{X \cdot Y}{2^{64}} \right\rfloor = (\text{hi} \ll 64) \mid (\text{lo} \gg 64)$$
Each partial product is a `u64 * u64 -> u128` operation (which **cannot overflow `u128`** since $(2^{64}-1)^2 < 2^{128}$), while `q64_mul`, `q64_mul_round`, and `mul_div_u128` verify `hi < 2^64` (or `hi < denom`) via checked arithmetic before updating `sqrt_p_current`, `sqrt_p_high_water`, or bin reserve ratios.

---

## 5. Threat Models & Simulated Mitigations

> [!NOTE]
> The G07 and G08 figures in this section are outputs of the economic model in `audit-harness/src/sim.rs`. Nobody has observed them on-chain. G06 and G09 are enforced by on-chain checks, and G10 was measured in LiteSVM.

### 5.1 Attack 1: 15-SOL Slot-0 Whale Sniper Cornering (`4,280 bps -> 1,850 bps`, simulated)

- **Threat Model**: On a bonding curve with a low flat fee (the harness baseline uses 1%), a single 15 SOL Slot-0 bundle buys deep into the initial supply curve before retail can react. In the baseline model it captures **4,280 bps (42.80%)** of supply.
- **Ratchet-DLMM-DAMM Defense**:
  1. Slot 0 opens with an $f_0$ = 8,000 bps (80%) base buy fee, so most of a Slot-0 buyer's SOL never reaches the bins.
  2. As the sniper's swap sweeps upward across bins, $v_a^+$ accumulates and the quadratic surcharge $\lceil A_{\text{buy}} \cdot (v_a^+ \cdot s)^2 / 10^{11} \rceil$ adds to the fee.
  3. **Simulated outcome (G07)**: In the harness model, the same 15 SOL sniper captures **1,850 bps (18.50%)** of supply, a 56.78% relative reduction. Buy fees are credited to `protocol_fee_lamports` and stay in the quote vault. They are not added to bin reserves (`execute_buy_swap` in `math.rs`).

### 5.2 Attack 2: Insider Wash-Trade Fee Siphon (`9,084–9,996 bps -> 80 bps`, simulated)

- **Threat Model**: In bonding curves or AMMs where creators or insiders receive liquid, uncompounded fee rebates, an insider can churn self-funded buy/sell loops and, in the baseline model, extract **9,084–9,996 bps (90.84%–99.96%)** of gross fee flow as SOL.
- **Ratchet-DLMM-DAMM Defense**:
  1. **Sell-fee compounding**: 75% (`compounding_fee_bps = 7500`) of every sell fee stays in the vault and is added to the floor bin's quote reserve.
  2. **5× dump-brake**: Rapid alternating sell legs raise $v_a^-$, pushing the sell fee toward the 2,500 bps cap.
  3. **High-water ratchet**: Price swings don't reopen cheap Slot-0 fee levels.
  4. **Simulated outcome (G08)**: In the harness model, net extractable insider fee leakage drops to **80 bps (0.80%)**.

### 5.3 Attack 3: Single-Leg Cascading Sell Wipeout (`700 bps` Impact Cap)

- **Threat Model**: A single whale dump sweeps across thin intermediate bins, causing $>20\%$ instantaneous price dislocation, triggering toxic cascading liquidations and sandwich extraction.
- **Ratchet-DLMM-DAMM Defense (`G09`)**: Pre-swap bin traversal checks verify single-leg execution impact $\le \mathbf{700\text{ bps}}$ (`50 bps` safety margin inside the `750 bps` hard limit), forcing multi-slot execution during which the $5\times$ dump-brake ($f_{\text{sell}}$) and $75\%$ floor compounding reinforce exit depth between blocks.

### 5.4 Attack 4: Hollow Graduation & Liquidity Gap Dead Zones (`G06`, `G10`)

- **Solvency Invariant (G06, enforced)**: `SeedInitialBins` and `GraduateInPlace` reject any state where `max_extractable_lamports > real_vault_lamports`. The benchmark figures (86.5 SOL real vs. 85.0 SOL max extractable) are **genesis configuration inputs** (`realVaultSol`, `maxExtractableSol`), not results of compounding. On every buy, `real_vault_lamports` grows by the full SOL input while `max_extractable_lamports` grows only by the post-fee amount (`execute_buy_swap`), so buy fees widen the surplus.
- **Zero-Dead-Zone Graduation (G10, measured in LiteSVM)**: Traditional bonding curves pause trading for one or more slots while they withdraw liquidity and initialize an external pool. That window risks a `PartialMigrationError` and MEV snipes on the new pool. Here, graduation sets `graduation_status` in place inside `RatchetPoolPda`, and a swap can execute in the same slot (`dead_zone_slots = 0`).

---

## 6. 10-Gate Deterministic Audit Matrix (`G01`–`G10`)

All gates must pass in the deterministic verification suite. The **Evidence** column says what kind of check backs each one:

| Gate ID | Gate Name | Verified Assertion / Metric | Required Target | Evidence |
| :--- | :--- | :--- | :--- | :--- |
| **`G01`** | `lamport_token_conservation` | Conservation across 10,000,000,000 L of harness swaps; on-chain `ConservationViolation` check in `compute_sell_compounding_split` | `error = 0` | Simulated + Enforced |
| **`G02`** | `single_tx_cu_budget` | Swap CU: 35,069 peak measured vs. 43,000 design budget | `<= 200,000 CU` | Measured (LiteSVM) |
| **`G03`** | `write_lock_slot_throughput` | Single write-lock lane (12,000,000 CU per account per block / 43,000 CU per tx) | `279 txs/slot` | Arithmetic |
| **`G04`** | `simd0437_rent_exactness` | `480/608 B` + `1,024/1,152 B` PDAs: Step 2 8,940,800 L; Step 5 1,224,960 L | Exact lamport match | Arithmetic |
| **`G05`** | `cpi_depth_and_account_limit` | CPI depth 2 (≤ 4); 28 accounts on the swap instruction. SIMD-0339's 255 limit applies to account infos passed into a CPI, which is a different limit from the 28 accounts on the instruction | `2 <= 4` | Static |
| **`G06`** | `vault_solvency_no_hollow_grad` | `max_extractable_lamports <= real_vault_lamports` (benchmark config 85.0 ≤ 86.5 SOL) | Rejected if violated | Enforced |
| **`G07`** | `slot0_sniper_corner_mitigation` | 15 SOL Slot-0 sniper capture vs. 4,280 bps flat-fee baseline | `1,850 bps` | **Simulated** |
| **`G08`** | `wash_fee_siphon_suppression` | Insider wash-trade fee leak vs. 9,084–9,996 bps baseline | `80 bps` | **Simulated** |
| **`G09`** | `sell_impact_ceiling_750bps` | Single-leg sell impact (50 bps buffer inside 750 bps) | `<= 700 bps` | Enforced + Measured |
| **`G10`** | `zero_dead_zone_graduation` | In-place graduation plus a same-slot swap | `dead_zone_slots = 0` | Measured (LiteSVM) |

### 6.1 Swap Compute Budget Allocation (`G02` & `G03`)

> [!NOTE]
> This table is a **design budget allocation**, not a profile. Per-stage costs were never measured individually. The measured whole-instruction figures (peak 35,069 CU) are in the LiteSVM table in [MAINNET_LAUNCH_RUNBOOK.md §6.2](MAINNET_LAUNCH_RUNBOOK.md).

| Execution Stage | Budgeted CU |
| :--- | :---: |
| Account validation & PDA bump checks (`28` accounts) | `4,200` |
| Zero-copy cast (`RatchetPoolPda` `480 B` + `ActiveBinArrayPda` `1,024 B`) | `1,800` |
| Directional accumulator ($v_a^+ / v_a^-$) & high-water $\sqrt{P}$ math | `6,500` |
| Discrete bin array traversal & `700 bps` impact guardrail | `11,400` |
| OnlyB `75%` quote auto-compounding & floor-bin reserve update | `3,100` |
| Token-2022 `TransferChecked` CPI (depth `2`, `TransferFeeConfig` check) | `13,500` |
| Zero-copy state writeback & swap telemetry emission | `2,500` |
| **Total Budgeted Swap CU** | **`43,000 CU` (`21.5%` of `200,000 CU`); measured peak 35,069 CU** |
| **Single Write-Lock Lane Slot Throughput** | **`279 txs/slot` (`12,000,000 / 43,000`, `3,000 CU` headroom)** |

/**
 * Exact BigInt arithmetic & fee preview module for Ratchet-DLMM-DAMM.
 *
 * Mirrors on-chain Q64.64 and checked u128 arithmetic with zero external runtime
 * dependencies so off-chain routers and UIs match on-chain execution to the bps/lamport.
 */

export const U64_MAX = (1n << 64n) - 1n;
export const U128_MAX = (1n << 128n) - 1n;
export const Q64_ONE = 1n << 64n;

/** Denominator for DLMM-style volatility fee term: 10^11. */
export const VOLATILITY_DENOMINATOR = 100_000_000_000n;

/** Basis points denominator (100% = 10,000 bps). */
export const BPS_DENOMINATOR = 10_000n;

/** Slot-0 genesis base buy fee: 80% = 8,000 bps. */
export const DEFAULT_F0_BPS = 8_000n;

/** Asymptotic floor base fee: 25 bps (0.25%). */
export const DEFAULT_F_MIN_BPS = 25n;

/** Immutable maximum sell fee cap: 25% = 2,500 bps. */
export const IMMUTABLE_F_MAX_SELL_BPS = 2_500n;

/** Immutable minimum OnlyB quote auto-compounding share: 75% = 7,500 bps. */
export const IMMUTABLE_MIN_COMPOUNDING_FEE_BPS = 7_500n;

/** Buffered single-leg sell price impact cap (Gate G09): 700 bps inside 750 bps hard cap. */
export const BUFFERED_SELL_IMPACT_CAP_BPS = 700n;

/** Hard single-leg sell price impact ceiling: 750 bps. */
export const HARD_SELL_IMPACT_CAP_BPS = 750n;

/** Default per-step high-water decay multiplier gamma in bps (0.90 = 9,000 bps). */
export const DEFAULT_GAMMA_BPS = 9_000n;

/** Asymmetric dump-brake multiplier: A_sell = 5 * A_buy. */
export const DUMP_BRAKE_MULTIPLIER = 5n;

export interface BuyFeePreviewParams {
  /** Initial Slot-0 base buy fee in bps (default: 8000 = 80%). */
  f0Bps?: bigint;
  /** Minimum asymptotic base fee in bps (default: 25). */
  fMinBps?: bigint;
  /** Per-step decay factor in bps out of 10,000 (default: 9000 = 0.90). Ignored if gammaQ64 is set. */
  gammaBps?: bigint;
  /** Optional exact Q64.64 per-step decay factor in [0, Q64_ONE]. */
  gammaQ64?: bigint;
  /** Explicit steps above genesis high-water sqrt(P_max). Derived from sqrtP values if omitted. */
  stepsAbove?: bigint;
  /** Genesis Q64.64 sqrt price (used if stepsAbove is omitted). */
  sqrtPGenesis?: bigint;
  /** Current or updated high-water Q64.64 sqrt price (used if stepsAbove is omitted). */
  sqrtPHighWater?: bigint;
  /** Optional current Q64.64 sqrt price; advances high-water via max(sqrtPHighWater, sqrtPCurrent). */
  sqrtPCurrent?: bigint;
  /** Volatility control constant A_buy. */
  aBuy: bigint;
  /** Upward volatility accumulator v_a^+ (`v_a_up`). */
  vAUp: bigint;
  /** Bin step `s` in bps (`bin_step_bps`). */
  binStepBps: bigint;
  /** Optional upper clamp in bps for total buy fee (defaults to uncapped unless provided). */
  maxBuyFeeBps?: bigint;
}

export interface BuyFeeBreakdown {
  highWaterSqrtP: bigint;
  stepsAbove: bigint;
  decayedBaseFeeBps: bigint;
  variableFeeBps: bigint;
  totalFeeBps: bigint;
}

export interface SellFeePreviewParams {
  /** Current base fee f_base in bps (typically high-water decayed base fee). */
  fBaseBps: bigint;
  /** Volatility control constant A_buy (A_sell = 5 * A_buy). */
  aBuy: bigint;
  /** Downward volatility accumulator v_a^- (`v_a_down`). */
  vADown: bigint;
  /** Bin step `s` in bps (`bin_step_bps`). */
  binStepBps: bigint;
  /** Immutable max sell fee cap in bps (default: 2500 = 25%, cannot exceed 2500). */
  fMaxSellBps?: bigint;
  /** If true, throw on u128 intermediate overflow instead of saturating to fMaxSellBps. */
  strictU128Overflow?: boolean;
}

export interface SellFeeBreakdown {
  fBaseBps: bigint;
  aSell: bigint;
  variableFeeBps: bigint;
  uncappedFeeBps: bigint;
  totalFeeBps: bigint;
  dumpBrakeCapped: boolean;
}

export interface AutoCompoundParams {
  /** Gross SOL output before fees in lamports (u64). */
  grossSolOutLamports: bigint;
  /** Effective sell fee f_sell(t) in bps (<= 2500). */
  sellFeeBps: bigint;
  /** Auto-compounding share of sell fees in bps (immutable min: 7500 = 75%). */
  compoundingFeeBps?: bigint;
}

export interface AutoCompoundPreview {
  grossSolOutLamports: bigint;
  sellFeeBps: bigint;
  compoundingFeeBps: bigint;
  totalSellFeeLamports: bigint;
  compoundedSolLamports: bigint;
  remainingFeeLamports: bigint;
  netSolOutLamports: bigint;
  /** Exact accounting drift check: (net + compounded + remaining) - gross === 0n. */
  conservationDriftLamports: bigint;
}

export function assertNonNegative(value: bigint, name: string): bigint {
  if (value < 0n) {
    throw new RangeError(`${name} must be non-negative, got ${value}`);
  }
  return value;
}

export function assertU64(value: bigint, name: string): bigint {
  if (value < 0n || value > U64_MAX) {
    throw new RangeError(`${name} out of u64 bounds [0, ${U64_MAX}]: ${value}`);
  }
  return value;
}

export function assertU128(value: bigint, name: string): bigint {
  if (value < 0n || value > U128_MAX) {
    throw new RangeError(`${name} out of u128 bounds [0, ${U128_MAX}]: ${value}`);
  }
  return value;
}

export function checkedAddU128(a: bigint, b: bigint, context = "u128_add"): bigint {
  assertU128(a, `${context}.lhs`);
  assertU128(b, `${context}.rhs`);
  const sum = a + b;
  if (sum > U128_MAX) {
    throw new RangeError(`${context} overflowed u128: ${a} + ${b}`);
  }
  return sum;
}

export function checkedMulU128(a: bigint, b: bigint, context = "u128_mul"): bigint {
  assertU128(a, `${context}.lhs`);
  assertU128(b, `${context}.rhs`);
  const prod = a * b;
  if (prod > U128_MAX) {
    throw new RangeError(`${context} overflowed u128: ${a} * ${b}`);
  }
  return prod;
}

/**
 * Exact unsigned u128 ceiling integer division: ceil(numerator / denominator).
 * Overflow-safe via (numerator - 1n) / denominator + 1n for numerator > 0.
 * Default denominator is 10^11 (100_000_000_000n).
 */
export function divCeilU128(
  numerator: bigint,
  denominator: bigint = VOLATILITY_DENOMINATOR
): bigint {
  assertU128(numerator, "divCeilU128.numerator");
  assertU128(denominator, "divCeilU128.denominator");
  if (denominator === 0n) {
    throw new RangeError("divCeilU128 division by zero");
  }
  if (numerator === 0n) {
    return 0n;
  }
  return (numerator - 1n) / denominator + 1n;
}

/** Arbitrary-precision ceiling division for Q128.128 squared price comparisons. */
export function divCeilBigInt(numerator: bigint, denominator: bigint): bigint {
  assertNonNegative(numerator, "divCeilBigInt.numerator");
  assertNonNegative(denominator, "divCeilBigInt.denominator");
  if (denominator === 0n) {
    throw new RangeError("divCeilBigInt division by zero");
  }
  if (numerator === 0n) {
    return 0n;
  }
  return (numerator - 1n) / denominator + 1n;
}

/**
 * High-water sqrt price monotonic ratchet:
 * sqrt_p_high_water = max(sqrt_p_high_water, sqrt_p_current)
 */
export function updateHighWaterSqrtPrice(
  sqrtPHighWater: bigint,
  sqrtPCurrent: bigint
): bigint {
  assertU128(sqrtPHighWater, "sqrtPHighWater");
  assertU128(sqrtPCurrent, "sqrtPCurrent");
  return sqrtPCurrent > sqrtPHighWater ? sqrtPCurrent : sqrtPHighWater;
}

/**
 * Q64.64 fixed-point round-nearest multiplication: ((a * b) + 2^63) >> 64.
 */
export function q64MulRound(a: bigint, b: bigint): bigint {
  return (a * b + (Q64_ONE >> 1n)) >> 64n;
}

/**
 * Q64.64 fixed-point exponentiation by squaring (round-nearest per step).
 * Matches on-chain `programs/ratchet-dlmm-damm/src/math.rs::pow_q64`.
 */
export function q64Pow(baseQ64: bigint, exp: bigint): bigint {
  assertU128(baseQ64, "baseQ64");
  assertNonNegative(exp, "exp");
  if (baseQ64 > Q64_ONE) {
    throw new RangeError(`gammaQ64 must be <= Q64_ONE (${Q64_ONE}), got ${baseQ64}`);
  }
  let result = Q64_ONE;
  let base = baseQ64;
  let e = exp;
  while (e > 0n) {
    if ((e & 1n) === 1n) {
      result = q64MulRound(result, base);
    }
    e >>= 1n;
    if (e > 0n) {
      base = q64MulRound(base, base);
    }
  }
  return result;
}

/**
 * Computes discrete bin steps above `sqrtPGenesis` reached by `sqrtPHighWater`,
 * where each bin step multiplies price P by `(10_000 + binStepBps) / 10_000`.
 */
export function computeStepsAboveSqrtPMax(
  sqrtPHighWater: bigint,
  sqrtPGenesis: bigint,
  binStepBps: bigint
): bigint {
  assertU128(sqrtPHighWater, "sqrtPHighWater");
  assertU128(sqrtPGenesis, "sqrtPGenesis");
  assertNonNegative(binStepBps, "binStepBps");
  if (sqrtPGenesis === 0n) {
    throw new RangeError("sqrtPGenesis must be > 0");
  }
  if (binStepBps === 0n || sqrtPHighWater <= sqrtPGenesis) {
    return 0n;
  }

  // Compare exact squared prices: (sqrtPHighWater / sqrtPGenesis)^2 >= ((10_000 + s) / 10_000)^k
  const targetNum = sqrtPHighWater * sqrtPHighWater;
  const targetDen = sqrtPGenesis * sqrtPGenesis;
  const stepNum = BPS_DENOMINATOR + binStepBps;
  const stepDen = BPS_DENOMINATOR;

  let steps = 0n;
  let curNum = 1n;
  let curDen = 1n;

  // Exponential search upper bound, then binary search.
  let stepPow = 1n;
  let powNum = stepNum;
  let powDen = stepDen;
  while (curNum * powNum * targetDen <= targetNum * curDen * powDen) {
    curNum *= powNum;
    curDen *= powDen;
    steps += stepPow;
    stepPow <<= 1n;
    powNum *= powNum;
    powDen *= powDen;
    if (steps > 1_000_000n) {
      break;
    }
  }

  // Binary refine remaining steps
  let lo = 0n;
  let hi = stepPow;
  while (lo + 1n < hi) {
    const mid = (lo + hi) >> 1n;
    const midNum = curNum * stepNum ** mid;
    const midDen = curDen * stepDen ** mid;
    if (midNum * targetDen <= targetNum * midDen) {
      lo = mid;
    } else {
      hi = mid;
    }
  }
  return steps + lo;
}

/**
 * Computes the decaying high-water base fee:
 * max(f_min, floor(f_0 * gamma^(steps_above(sqrt_P_max))))
 */
export function computeHighWaterBaseFeeBps(params: {
  f0Bps?: bigint;
  fMinBps?: bigint;
  gammaBps?: bigint;
  gammaQ64?: bigint;
  stepsAbove: bigint;
}): bigint {
  const f0Bps = params.f0Bps ?? DEFAULT_F0_BPS;
  const fMinBps = params.fMinBps ?? DEFAULT_F_MIN_BPS;
  assertNonNegative(f0Bps, "f0Bps");
  assertNonNegative(fMinBps, "fMinBps");
  assertNonNegative(params.stepsAbove, "stepsAbove");

  let gammaQ64: bigint;
  if (params.gammaQ64 !== undefined) {
    gammaQ64 = params.gammaQ64;
    if (gammaQ64 < 0n || gammaQ64 > Q64_ONE) {
      throw new RangeError(`gammaQ64 must be in [0, ${Q64_ONE}], got ${gammaQ64}`);
    }
  } else {
    const gammaBps = params.gammaBps ?? DEFAULT_GAMMA_BPS;
    if (gammaBps < 0n || gammaBps > BPS_DENOMINATOR) {
      throw new RangeError(`gammaBps must be in [0, 10000], got ${gammaBps}`);
    }
    gammaQ64 = ((gammaBps << 64n) + (BPS_DENOMINATOR >> 1n)) / BPS_DENOMINATOR;
  }

  const decayFactorQ64 = q64Pow(gammaQ64, params.stepsAbove);
  const decayed = (f0Bps * decayFactorQ64 + (Q64_ONE >> 1n)) >> 64n;
  return decayed > fMinBps ? decayed : fMinBps;
}

/**
 * Computes checked u128 volatility surcharge:
 * ceil(coeff * (v_a * s)^2 / 10^11)
 */
export function computeVolatilityVariableFeeBps(
  coeff: bigint,
  vA: bigint,
  binStepBps: bigint
): bigint {
  assertU64(coeff, "volatilityCoeff");
  assertU64(vA, "vA");
  assertU64(binStepBps, "binStepBps");

  if (coeff === 0n || vA === 0n || binStepBps === 0n) {
    return 0n;
  }

  const vs = checkedMulU128(vA, binStepBps, "v_a * s");
  const vsSq = checkedMulU128(vs, vs, "(v_a * s)^2");
  const numerator = checkedMulU128(coeff, vsSq, "coeff * (v_a * s)^2");
  return divCeilU128(numerator, VOLATILITY_DENOMINATOR);
}

/**
 * Detailed breakdown for buy fee calculation:
 * f_buy(t) = max(f_min, f_0 * gamma^(steps_above(sqrt_P_max))) + ceil(A_buy * (v_a^+ * s)^2 / 10^11)
 */
export function previewBuyFeeBreakdown(params: BuyFeePreviewParams): BuyFeeBreakdown {
  const f0Bps = params.f0Bps ?? DEFAULT_F0_BPS;
  const fMinBps = params.fMinBps ?? DEFAULT_F_MIN_BPS;

  let highWaterSqrtP = params.sqrtPHighWater ?? 0n;
  if (params.sqrtPCurrent !== undefined) {
    highWaterSqrtP = updateHighWaterSqrtPrice(highWaterSqrtP, params.sqrtPCurrent);
  }

  let stepsAbove: bigint;
  if (params.stepsAbove !== undefined) {
    stepsAbove = assertNonNegative(params.stepsAbove, "stepsAbove");
  } else if (params.sqrtPGenesis !== undefined) {
    stepsAbove = computeStepsAboveSqrtPMax(
      highWaterSqrtP,
      params.sqrtPGenesis,
      params.binStepBps
    );
  } else {
    stepsAbove = 0n;
  }

  const decayedBaseFeeBps = computeHighWaterBaseFeeBps({
    f0Bps,
    fMinBps,
    gammaBps: params.gammaBps,
    gammaQ64: params.gammaQ64,
    stepsAbove,
  });

  const variableFeeBps = computeVolatilityVariableFeeBps(
    params.aBuy,
    params.vAUp,
    params.binStepBps
  );

  let totalFeeBps = checkedAddU128(decayedBaseFeeBps, variableFeeBps, "f_buy_total");
  if (params.maxBuyFeeBps !== undefined && totalFeeBps > params.maxBuyFeeBps) {
    totalFeeBps = params.maxBuyFeeBps;
  }

  return {
    highWaterSqrtP,
    stepsAbove,
    decayedBaseFeeBps,
    variableFeeBps,
    totalFeeBps,
  };
}

/**
 * Computes exact buy fee in basis points:
 * f_buy(t) = max(f_min, f_0 * gamma^(steps_above(sqrt_P_max))) + ceil(A_buy * (v_a^+ * s)^2 / 10^11)
 */
export function previewBuyFeeBps(params: BuyFeePreviewParams): bigint {
  return previewBuyFeeBreakdown(params).totalFeeBps;
}

/**
 * Detailed breakdown for asymmetric sell dump-brake calculation:
 * f_sell(t) = min(25%, f_base + ceil(5 * A_buy * (v_a^- * s)^2 / 10^11))
 */
export function previewSellFeeBreakdown(params: SellFeePreviewParams): SellFeeBreakdown {
  const fBaseBps = assertNonNegative(params.fBaseBps, "fBaseBps");
  const fMaxSellBps = params.fMaxSellBps ?? IMMUTABLE_F_MAX_SELL_BPS;
  if (fMaxSellBps <= 0n || fMaxSellBps > IMMUTABLE_F_MAX_SELL_BPS) {
    throw new RangeError(
      `fMaxSellBps must be in (0, ${IMMUTABLE_F_MAX_SELL_BPS}], got ${fMaxSellBps}`
    );
  }

  const aBuy = assertU64(params.aBuy, "aBuy");
  const aSell = checkedMulU128(DUMP_BRAKE_MULTIPLIER, aBuy, "5 * A_buy");
  const vADown = assertU64(params.vADown, "vADown");
  const binStepBps = assertU64(params.binStepBps, "binStepBps");

  let variableFeeBps: bigint;
  if (aSell === 0n || vADown === 0n || binStepBps === 0n) {
    variableFeeBps = 0n;
  } else if (params.strictU128Overflow) {
    const vs = checkedMulU128(vADown, binStepBps, "v_a^- * s");
    const vsSq = checkedMulU128(vs, vs, "(v_a^- * s)^2");
    const numerator = checkedMulU128(aSell, vsSq, "5 * A_buy * (v_a^- * s)^2");
    variableFeeBps = divCeilU128(numerator, VOLATILITY_DENOMINATOR);
  } else {
    // Check in arbitrary-precision BigInt whether numerator fits u128; if numerator exceeds
    // u128::MAX we safely clamp at fMaxSellBps rather than wrapping or panicking.
    const vs = vADown * binStepBps;
    const vsSq = vs * vs;
    const numerator = aSell * vsSq;
    if (numerator > U128_MAX) {
      variableFeeBps = fMaxSellBps;
    } else {
      variableFeeBps = divCeilU128(numerator, VOLATILITY_DENOMINATOR);
    }
  }

  const uncappedFeeBps = fBaseBps + variableFeeBps;
  const dumpBrakeCapped = uncappedFeeBps > fMaxSellBps;
  const totalFeeBps = dumpBrakeCapped ? fMaxSellBps : uncappedFeeBps;

  return {
    fBaseBps,
    aSell,
    variableFeeBps,
    uncappedFeeBps,
    totalFeeBps,
    dumpBrakeCapped,
  };
}

/**
 * Computes exact asymmetric sell fee in basis points (capped at immutable 25% = 2,500 bps):
 * f_sell(t) = min(25%, f_base + ceil(5 * A_buy * (v_a^- * s)^2 / 10^11))
 */
export function previewSellFeeBps(params: SellFeePreviewParams): bigint {
  return previewSellFeeBreakdown(params).totalFeeBps;
}

/**
 * Computes OnlyB quote auto-compounding and 0-lamport-drift fee split matching on-chain `compounding.rs`:
 *   total_sell_fee = min(Gross_SOL_Out, ceil(Gross_SOL_Out * f_sell(t) / 10_000))
 *   Delta_R_sol_compounded = floor(compounding_fee_bps * f_sell(t) * Gross_SOL_Out / 100_000_000)
 *   remaining_fee = total_sell_fee - Delta_R_sol_compounded
 *   net_sol_out = Gross_SOL_Out - total_sell_fee
 * Guarantee: net_sol_out + Delta_R_sol_compounded + remaining_fee === Gross_SOL_Out (drift == 0)
 */
export function previewAutoCompoundSol(params: AutoCompoundParams): AutoCompoundPreview {
  const grossSolOutLamports = assertU64(params.grossSolOutLamports, "grossSolOutLamports");
  const sellFeeBps = assertNonNegative(params.sellFeeBps, "sellFeeBps");
  if (sellFeeBps > IMMUTABLE_F_MAX_SELL_BPS) {
    throw new RangeError(
      `sellFeeBps ${sellFeeBps} exceeds immutable sell fee cap ${IMMUTABLE_F_MAX_SELL_BPS}`
    );
  }
  const compoundingFeeBps = params.compoundingFeeBps ?? IMMUTABLE_MIN_COMPOUNDING_FEE_BPS;
  if (
    compoundingFeeBps < IMMUTABLE_MIN_COMPOUNDING_FEE_BPS ||
    compoundingFeeBps > BPS_DENOMINATOR
  ) {
    throw new RangeError(
      `compoundingFeeBps must be in [${IMMUTABLE_MIN_COMPOUNDING_FEE_BPS}, ${BPS_DENOMINATOR}], got ${compoundingFeeBps}`
    );
  }

  const rawFee =
    grossSolOutLamports === 0n || sellFeeBps === 0n
      ? 0n
      : divCeilU128(grossSolOutLamports * sellFeeBps, BPS_DENOMINATOR);
  const totalSellFeeLamports =
    rawFee > grossSolOutLamports ? grossSolOutLamports : rawFee;
  const compoundedSolLamports =
    (grossSolOutLamports * sellFeeBps * compoundingFeeBps) / 100_000_000n;
  const remainingFeeLamports = totalSellFeeLamports - compoundedSolLamports;
  const netSolOutLamports = grossSolOutLamports - totalSellFeeLamports;
  const conservationDriftLamports =
    netSolOutLamports + compoundedSolLamports + remainingFeeLamports - grossSolOutLamports;

  return {
    grossSolOutLamports,
    sellFeeBps,
    compoundingFeeBps,
    totalSellFeeLamports,
    compoundedSolLamports,
    remainingFeeLamports,
    netSolOutLamports,
    conservationDriftLamports,
  };
}

/**
 * Validates Gate G09 single-leg sell price impact against the 700 bps buffered ceiling
 * (inside the 750 bps hard limit).
 */
export function assertSellImpactWithinCap(
  sqrtPBefore: bigint,
  sqrtPAfter: bigint,
  maxImpactBps: bigint = BUFFERED_SELL_IMPACT_CAP_BPS
): bigint {
  assertU128(sqrtPBefore, "sqrtPBefore");
  assertU128(sqrtPAfter, "sqrtPAfter");
  if (sqrtPBefore === 0n) {
    throw new RangeError("sqrtPBefore must be > 0");
  }
  if (maxImpactBps > HARD_SELL_IMPACT_CAP_BPS) {
    throw new RangeError(
      `maxImpactBps ${maxImpactBps} exceeds hard cap ${HARD_SELL_IMPACT_CAP_BPS}`
    );
  }
  if (sqrtPAfter >= sqrtPBefore) {
    return 0n;
  }
  // Exact price ratio impact in bps: ceil((P_before - P_after) * 10_000 / P_before)
  // Note: P = (sqrtP_Q64)^2 is Q128.128, so use exact 256-bit safe bigint ceiling division.
  const pBefore = sqrtPBefore * sqrtPBefore;
  const pAfter = sqrtPAfter * sqrtPAfter;
  const impactBps = divCeilBigInt((pBefore - pAfter) * BPS_DENOMINATOR, pBefore);
  if (impactBps > maxImpactBps) {
    throw new RangeError(
      `Sell price impact ${impactBps} bps exceeds Gate G09 cap ${maxImpactBps} bps`
    );
  }
  return impactBps;
}

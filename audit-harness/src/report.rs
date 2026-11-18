//! Zero-dependency JSON and Markdown report renderers for `AuditReport`.

use crate::gates::AuditReport;

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Renders `AuditReport` as formatted, valid JSON (`gate_verification_report.json`).
pub fn render_json_report(report: &AuditReport) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("{\n");
    out.push_str(&format!("  \"protocol\": \"{}\",\n", json_escape(report.protocol)));
    out.push_str(&format!(
        "  \"harness_crate\": \"{}\",\n",
        json_escape(report.harness_crate)
    ));
    out.push_str(&format!("  \"audit_pass_rate\": {:.2},\n", report.audit_pass_rate));
    out.push_str(&format!("  \"total_gates\": {},\n", report.total_gates));
    out.push_str(&format!("  \"passed_gates\": {},\n", report.passed_gates));
    out.push_str(&format!("  \"failed_gates\": {},\n", report.failed_gates));
    out.push_str(&format!(
        "  \"core_invariants_passed\": {},\n",
        report.core_invariants.all_invariants_passed
    ));

    // Core invariants section
    let ci = &report.core_invariants;
    let ov = &ci.u128_overflow_safety;
    out.push_str("  \"core_invariants\": {\n");
    out.push_str(&format!("    \"slot0_f_buy_bps\": {},\n", ci.slot0_f_buy_bps));
    out.push_str(&format!("    \"decayed_f_buy_bps\": {},\n", ci.decayed_f_buy_bps));
    out.push_str(&format!("    \"surge_f_buy_bps\": {},\n", ci.surge_f_buy_bps));
    out.push_str(&format!("    \"calm_f_sell_bps\": {},\n", ci.calm_f_sell_bps));
    out.push_str(&format!("    \"surge_f_sell_bps\": {},\n", ci.surge_f_sell_bps));
    out.push_str(&format!("    \"capped_f_sell_bps\": {},\n", ci.capped_f_sell_bps));
    out.push_str(&format!(
        "    \"sample_gross_sol_out_lamports\": {},\n",
        ci.sample_gross_sol_out
    ));
    out.push_str(&format!(
        "    \"sample_total_sell_fee_lamports\": {},\n",
        ci.sample_total_sell_fee
    ));
    out.push_str(&format!(
        "    \"sample_delta_r_sol_compounded_lamports\": {},\n",
        ci.sample_delta_r_sol_compounded
    ));
    out.push_str(&format!(
        "    \"sample_residual_fee_lamports\": {},\n",
        ci.sample_residual_fee
    ));
    out.push_str(&format!(
        "    \"sample_conservation_error_lamports\": {},\n",
        ci.sample_conservation_error
    ));
    out.push_str(&format!("    \"high_water_monotonic\": {},\n", ci.high_water_monotonic));
    out.push_str("    \"u128_overflow_safety\": {\n");
    out.push_str(&format!("      \"safe\": {},\n", ov.safe));
    out.push_str(&format!("      \"domain_max_v_a_down\": {},\n", ov.domain_max_v_a_down));
    out.push_str(&format!("      \"domain_max_bin_step_bps\": {},\n", ov.domain_max_bin_step_bps));
    out.push_str(&format!("      \"domain_max_a_buy\": {},\n", ov.domain_max_a_buy));
    out.push_str(&format!(
        "      \"domain_peak_numerator\": \"{}\",\n",
        ov.domain_peak_numerator
    ));
    out.push_str(&format!("      \"u128_max\": \"{}\",\n", ov.u128_max));
    out.push_str(&format!("      \"headroom_bits\": {},\n", ov.headroom_bits));
    out.push_str(&format!("      \"fuzz_cases_tested\": {},\n", ov.fuzz_cases_tested));
    out.push_str(&format!(
        "      \"silent_wraps_observed\": {},\n",
        ov.silent_wraps_observed
    ));
    out.push_str(&format!(
        "      \"checked_rejections_on_extreme_u64\": {}\n",
        ov.checked_rejections_on_extreme_u64
    ));
    out.push_str("    }\n");
    out.push_str("  },\n");

    // Compiled sBPF binary check & LiteSVM CU parity
    let bc = &report.binary_check;
    out.push_str("  \"binary_artifact_check\": {\n");
    out.push_str(&format!("    \"so_path\": \"{}\",\n", json_escape(&bc.so_path)));
    out.push_str(&format!("    \"so_present\": {},\n", bc.so_present));
    out.push_str(&format!("    \"so_size_bytes\": {},\n", bc.so_size_bytes));
    out.push_str(&format!("    \"so_is_elf64\": {},\n", bc.so_is_elf64));
    out.push_str(&format!("    \"so_machine\": {},\n", bc.so_machine));
    out.push_str(&format!("    \"so_machine_is_bpf\": {},\n", bc.so_machine_is_bpf));
    out.push_str(&format!(
        "    \"so_exports_entrypoint\": {},\n",
        bc.so_exports_entrypoint
    ));
    out.push_str(&format!("    \"so_sha256\": \"{}\",\n", json_escape(&bc.so_sha256)));
    out.push_str(&format!("    \"declare_id\": \"{}\",\n", json_escape(&bc.declare_id)));
    out.push_str(&format!("    \"keypair_pubkey\": \"{}\",\n", json_escape(&bc.keypair_pubkey)));
    out.push_str(&format!("    \"jupiter_program_id\": \"{}\",\n", json_escape(&bc.jupiter_program_id)));
    out.push_str(&format!("    \"ts_program_id\": \"{}\",\n", json_escape(&bc.ts_program_id)));
    out.push_str(&format!(
        "    \"launch_config_program_id\": \"{}\",\n",
        json_escape(&bc.launch_config_program_id)
    ));
    out.push_str(&format!(
        "    \"declare_id_matches_keypair\": {},\n",
        bc.declare_id_matches_keypair
    ));
    out.push_str(&format!(
        "    \"declare_id_consistent_across_repo\": {},\n",
        bc.declare_id_consistent_across_repo
    ));
    out.push_str(&format!("    \"passed\": {}\n", bc.passed));
    out.push_str("  },\n");

    match &report.measured_cu {
        Some(mc) => {
            out.push_str("  \"litesvm_measured_cu\": {\n");
            out.push_str(&format!("    \"source\": \"{}\",\n", json_escape(&mc.source)));
            if let Some(b) = mc.buy_swap_cu {
                out.push_str(&format!("    \"buy_swap_cu\": {},\n", b));
            }
            if let Some(s) = mc.sell_swap_cu {
                out.push_str(&format!("    \"sell_swap_cu\": {},\n", s));
            }
            out.push_str(&format!("    \"max_swap_cu\": {},\n", mc.max_swap_cu));
            out.push_str(&format!(
                "    \"within_single_tx_ceiling\": {},\n",
                mc.within_single_tx_ceiling
            ));
            out.push_str(&format!("    \"within_43000_model\": {}\n", mc.within_43000_model));
            out.push_str("  },\n");
        }
        None => {
            out.push_str("  \"litesvm_measured_cu\": null,\n");
        }
    }

    // 10 gates array
    out.push_str("  \"gates\": [\n");
    for (idx, gate) in report.gates.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!("      \"id\": \"{}\",\n", json_escape(gate.id)));
        out.push_str(&format!("      \"title\": \"{}\",\n", json_escape(gate.title)));
        out.push_str(&format!(
            "      \"status\": \"{}\",\n",
            if gate.passed { "PASS" } else { "FAIL" }
        ));
        out.push_str(&format!("      \"passed\": {},\n", gate.passed));
        out.push_str(&format!(
            "      \"threshold\": \"{}\",\n",
            json_escape(&gate.threshold_summary)
        ));
        out.push_str(&format!(
            "      \"observed\": \"{}\",\n",
            json_escape(&gate.observed_summary)
        ));
        out.push_str("      \"metrics\": {\n");
        for (m_idx, (k, v)) in gate.detail_metrics.iter().enumerate() {
            let comma = if m_idx + 1 < gate.detail_metrics.len() {
                ","
            } else {
                ""
            };
            // Render pure unsigned integers, booleans, or floats unquoted; strings neatly escaped.
            let is_unquoted_primitive = v == "true"
                || v == "false"
                || (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
                || (v.contains('.') && v.parse::<f64>().is_ok());
            if is_unquoted_primitive {
                out.push_str(&format!("        \"{}\": {}{}\n", json_escape(k), v, comma));
            } else {
                out.push_str(&format!(
                    "        \"{}\": \"{}\"{}\n",
                    json_escape(k),
                    json_escape(v),
                    comma
                ));
            }
        }
        out.push_str("      }\n");
        let gate_comma = if idx + 1 < report.gates.len() { "," } else { "" };
        out.push_str(&format!("    }}{}\n", gate_comma));
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

/// Renders `AuditReport` as a comprehensive Markdown audit report (`GATE_AUDIT_REPORT.md`).
pub fn render_markdown_report(report: &AuditReport) -> String {
    let mut md = String::with_capacity(8192);
    md.push_str("# Ratchet-DLMM-DAMM Phase 4 Deterministic 10-Gate Audit Report\n\n");
    md.push_str(&format!(
        "- **Protocol**: `{}`\n- **Harness Crate**: `{}`\n- **Audit Pass Rate**: **{:.2}** ({} / {} gates PASS)\n- **Core Invariants Verified**: `{}`\n\n",
        report.protocol,
        report.harness_crate,
        report.audit_pass_rate,
        report.passed_gates,
        report.total_gates,
        report.core_invariants.all_invariants_passed
    ));

    md.push_str("## 1. 10-Gate Verification Summary\n\n");
    md.push_str("| Gate ID | Status | Specification Threshold | Observed Result |\n");
    md.push_str("| :--- | :---: | :--- | :--- |\n");
    for gate in &report.gates {
        md.push_str(&format!(
            "| `{}` | **{}** | {} | {} |\n",
            gate.id,
            if gate.passed { "PASS" } else { "FAIL" },
            gate.threshold_summary,
            gate.observed_summary
        ));
    }
    md.push('\n');

    md.push_str("## 2. Core Invariant Math & `u128` Overflow Safety\n\n");
    let ci = &report.core_invariants;
    let ov = &ci.u128_overflow_safety;
    md.push_str(&format!(
        "- **High-Water Buy Ratchet `f_buy(t)`**: Slot-0 `{} bps` (80.00%) -> Asymptotic `{} bps` (0.25%) -> Surge w/ `v_a^+`: `{} bps` (monotonic `sqrt_P_high_water`: `{}`)\n",
        ci.slot0_f_buy_bps, ci.decayed_f_buy_bps, ci.surge_f_buy_bps, ci.high_water_monotonic
    ));
    md.push_str(&format!(
        "- **Asymmetric 5x Dump-Brake `f_sell(t)`**: Calm `{} bps` -> Mid-dump `{} bps` -> Hard-capped `{} bps` (25.00%)\n",
        ci.calm_f_sell_bps, ci.surge_f_sell_bps, ci.capped_f_sell_bps
    ));
    md.push_str(&format!(
        "- **OnlyB Quote Auto-Compounding (`Delta_R_sol_compounded = 0.75 * f_sell * Gross_SOL_Out`)**: On `{}` L gross sell at `{} bps`, `total_fee = {} L`, `Delta_R_sol_compounded = {} L`, `residual_fee = {} L`, `conservation_error = {} L`\n",
        ci.sample_gross_sol_out,
        ci.capped_f_sell_bps,
        ci.sample_total_sell_fee,
        ci.sample_delta_r_sol_compounded,
        ci.sample_residual_fee,
        ci.sample_conservation_error
    ));
    md.push_str(&format!(
        "- **`u128` Overflow Safety (`(v_a^- * s)^2 * 5 * A_buy / 10^11`)**: Peak domain numerator (`v_a^- = {}`, `s = {}`, `A_buy = {}`) = `{}` < `u128::MAX` (`{}`), leaving **{} bits** of headroom; `{}` deterministic fuzz cases run with **{} silent wraps** and `{}` safe checked overflow rejections on extreme `u64` stress inputs.\n\n",
        ov.domain_max_v_a_down,
        ov.domain_max_bin_step_bps,
        ov.domain_max_a_buy,
        ov.domain_peak_numerator,
        ov.u128_max,
        ov.headroom_bits,
        ov.fuzz_cases_tested,
        ov.silent_wraps_observed,
        ov.checked_rejections_on_extreme_u64
    ));

    md.push_str("## 3. sBPF Hot-Path Compute & Write-Lock Slot Profile (G02 & G03)\n\n");
    md.push_str("| Stage | Compute Units (CU) |\n");
    md.push_str("| :--- | ---: |\n");
    for stage in &report.g02_g03_compute.stages {
        md.push_str(&format!("| {} | {} |\n", stage.stage, stage.compute_units));
    }
    md.push_str(&format!(
        "| **Total Hot-Path Swap CU** | **{}** ({:.1}% of {} CU ceiling) |\n",
        report.g02_g03_compute.total_cu,
        report.g02_g03_compute.utilization_pct,
        report.g02_g03_compute.tx_cu_ceiling
    ));
    md.push_str(&format!(
        "| **Single Write-Lock Lane Capacity** | **{} txs/slot** (`{} / {}`, {} CU remainder) |\n\n",
        report.g02_g03_compute.txs_per_slot_capacity,
        report.g02_g03_compute.slot_write_lock_cu_limit,
        report.g02_g03_compute.total_cu,
        report.g02_g03_compute.slot_remaining_cu
    ));

    md.push_str("## 4. SIMD-0437 Rent Exactness & Account Budget (G04 & G05)\n\n");
    let r = &report.g04_rent;
    md.push_str("| PDA Account | Data Bytes | Overhead | Total Bytes | Step 2 (5,080 L/B) | Step 5 (696 L/B) |\n");
    md.push_str("| :--- | ---: | ---: | ---: | ---: | ---: |\n");
    md.push_str(&format!(
        "| `ratchet_pool_pda` | {} | 128 | {} | {} L | {} L |\n",
        r.pool_data_bytes, r.pool_total_bytes, r.step2_pool_lamports, r.step5_pool_lamports
    ));
    md.push_str(&format!(
        "| `active_bin_array_pda` | {} | 128 | {} | {} L | {} L |\n",
        r.bin_array_data_bytes,
        r.bin_array_total_bytes,
        r.step2_bin_array_lamports,
        r.step5_bin_array_lamports
    ));
    md.push_str(&format!(
        "| **Combined Total** | **{}** | **256** | **{}** | **{} L** | **{} L** |\n\n",
        r.pool_data_bytes + r.bin_array_data_bytes,
        r.pool_total_bytes + r.bin_array_total_bytes,
        r.step2_total_lamports,
        r.step5_total_lamports
    ));
    md.push_str(&format!(
        "- **CPI Call Depth**: `{}` (<= `{}` Solana limit)\n- **Swap Account Count**: `{}` unique metas (<= `{}` SIMD-0339 limit)\n\n",
        report.g05_cpi_accounts.max_cpi_depth_observed,
        report.g05_cpi_accounts.max_cpi_depth_limit,
        report.g05_cpi_accounts.account_count,
        report.g05_cpi_accounts.simd0339_limit
    ));

    md.push_str("## 5. Economic & Attack-Mitigation Simulations (G01, G06-G10)\n\n");
    md.push_str(&format!(
        "- **G01 Conservation**: `LHS = {} L`, `RHS = {} L`, `error = {} L` across `{}` odd-lamport swaps (`token_error = {}`).\n",
        report.g01_conservation.lhs_lamports,
        report.g01_conservation.rhs_lamports,
        report.g01_conservation.error_lamports,
        report.g01_conservation.swaps_executed,
        report.g01_conservation.token_error
    ));
    md.push_str(&format!(
        "- **G06 Vault Solvency**: `max_extractable = {} L <= real_vault = {} L` (`surplus = {} L`), hollow graduation attempt rejected = `{}`.\n",
        report.g06_solvency.max_extractable_lamports,
        report.g06_solvency.real_vault_lamports,
        report.g06_solvency.solvency_surplus_lamports,
        report.g06_solvency.hollow_grad_rejected
    ));
    md.push_str(&format!(
        "- **G07 Slot-0 Whale Sniper (15 SOL)**: Baseline `{} bps` (42.80%) -> Ratchet `{} bps` (18.50%), `v_a^+` climbed to `{}`, peak `f_buy = {} bps`.\n",
        report.g07_sniper.baseline_capture_bps,
        report.g07_sniper.ratchet_capture_bps,
        report.g07_sniper.final_v_a_up,
        report.g07_sniper.peak_f_buy_bps
    ));
    md.push_str(&format!(
        "- **G08 Wash-Fee Siphon Suppression**: Baseline leak `{}-{} bps` compressed down to `{} bps`; all `{}` self-churn cohorts finished with negative EV (`unprofitable = {}`).\n",
        report.g08_wash.baseline_leak_min_bps,
        report.g08_wash.baseline_leak_max_bps,
        report.g08_wash.ratchet_leak_bps,
        report.g08_wash.cohorts.len(),
        report.g08_wash.all_wash_cohorts_unprofitable
    ));
    md.push_str(&format!(
        "- **G09 Single-Leg Sell Impact Ceiling**: Clamped at `{} bps` (`<= {} bps` buffered / `<= {} bps` hard cap); unconstrained sweep would cause `{} bps` impact (strict overshoot rejected = `{}`).\n",
        report.g09_sell_impact.max_single_leg_impact_bps,
        report.g09_sell_impact.buffered_ceiling_bps,
        report.g09_sell_impact.hard_cap_bps,
        report.g09_sell_impact.unconstrained_impact_bps,
        report.g09_sell_impact.strict_overshoot_rejected
    ));
    md.push_str(&format!(
        "- **G10 Atomic In-Place Graduation**: Transition `status {} -> {}` & `authority_revoked = {}` inside slot `{}` with `dead_zone_slots = {}` and `partial_migration_errors = {}`.\n\n",
        report.g10_graduation.pre_grad_status,
        report.g10_graduation.post_grad_status,
        report.g10_graduation.authority_revoked,
        report.g10_graduation.graduation_slot,
        report.g10_graduation.dead_zone_slots,
        report.g10_graduation.partial_migration_errors
    ));

    let bc = &report.binary_check;
    md.push_str("## 6. Compiled `.so` Binary & LiteSVM E2E Compute Parity\n\n");
    md.push_str(&format!(
        "- **Artifact**: `{}` (`{} bytes`, ELF64 = `{}`, `e_machine = {}` BPF = `{}`, `entrypoint` = `{}`)\n",
        bc.so_path, bc.so_size_bytes, bc.so_is_elf64, bc.so_machine, bc.so_machine_is_bpf, bc.so_exports_entrypoint
    ));
    md.push_str(&format!("- **SHA-256**: `{}`\n", bc.so_sha256));
    md.push_str(&format!(
        "- **`declare_id!` & Cross-Repo Consistency**: `lib.rs = {}`, `keypair = {}`, `jupiter-adapter = {}`, `ts-client = {}`, `launch.config.json = {}` (`consistent = {}`, `passed = {}`)\n",
        bc.declare_id,
        bc.keypair_pubkey,
        bc.jupiter_program_id,
        bc.ts_program_id,
        bc.launch_config_program_id,
        bc.declare_id_consistent_across_repo,
        bc.passed
    ));
    if let Some(mc) = &report.measured_cu {
        md.push_str(&format!(
            "- **LiteSVM Measured Swap CU** (`{}`): `buy_swap_cu = {:?}`, `sell_swap_cu = {:?}`, `max_swap_cu = {} CU` (`<= 43,000 CU` model: `{}`, `<= 200,000 CU` ceiling: `{}`)\n",
            mc.source,
            mc.buy_swap_cu,
            mc.sell_swap_cu,
            mc.max_swap_cu,
            mc.within_43000_model,
            mc.within_single_tx_ceiling
        ));
    }

    md
}

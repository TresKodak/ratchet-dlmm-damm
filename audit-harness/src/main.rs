use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use ratchet_audit_harness::{render_json_report, render_markdown_report, run_all_10_gates};

fn resolve_output_dir() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        let cargo_toml = cwd.join("Cargo.toml");
        if let Ok(contents) = fs::read_to_string(&cargo_toml) {
            if contents.contains("ratchet_audit_harness") {
                return cwd;
            }
        }
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        let path = Path::new(manifest_dir);
        if path.is_dir() {
            return path.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn main() {
    let report = run_all_10_gates();
    let json_text = render_json_report(&report);
    let md_text = render_markdown_report(&report);

    let out_dir = resolve_output_dir();
    let json_path = out_dir.join("gate_verification_report.json");
    let md_path = out_dir.join("GATE_AUDIT_REPORT.md");

    if let Err(err) = fs::write(&json_path, &json_text) {
        eprintln!("Failed to write {}: {}", json_path.display(), err);
        process::exit(1);
    }
    if let Err(err) = fs::write(&md_path, &md_text) {
        eprintln!("Failed to write {}: {}", md_path.display(), err);
        process::exit(1);
    }

    println!(
        "Ratchet-DLMM-DAMM Audit Harness: {}/{} gates PASS (audit_pass_rate = {:.2})",
        report.passed_gates, report.total_gates, report.audit_pass_rate
    );
    for gate in &report.gates {
        println!(
            "  [{}] {} -> {}",
            if gate.passed { "PASS" } else { "FAIL" },
            gate.id,
            gate.observed_summary
        );
    }
    println!("Generated: {}", json_path.display());
    println!("Generated: {}", md_path.display());

    if report.failed_gates > 0 || !report.core_invariants.all_invariants_passed {
        eprintln!("Audit verification failed.");
        process::exit(1);
    }
}

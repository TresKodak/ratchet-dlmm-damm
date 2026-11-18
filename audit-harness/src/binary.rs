//! Real compiled sBPF `.so` artifact verification and LiteSVM-measured compute-unit parity.
//!
//! Std-only: includes a SHA-256 and a Base58 encoder so no external crates are required.

use std::fs;
use std::path::{Path, PathBuf};

/// ELF `e_machine` values accepted for Solana BPF/SBF programs (`EM_BPF = 247`, `EM_SBF = 263`).
const EM_BPF: u16 = 247;
const EM_SBF: u16 = 263;

/// Result of verifying `target/deploy/ratchet_dlmm_damm.so` against the program source and keypair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryCheck {
    pub so_path: String,
    pub so_present: bool,
    pub so_size_bytes: u64,
    pub so_is_elf64: bool,
    pub so_machine: u16,
    pub so_machine_is_bpf: bool,
    pub so_exports_entrypoint: bool,
    pub so_sha256: String,
    pub declare_id: String,
    pub keypair_pubkey: String,
    pub jupiter_program_id: String,
    pub ts_program_id: String,
    pub launch_config_program_id: String,
    pub declare_id_matches_keypair: bool,
    pub declare_id_consistent_across_repo: bool,
    pub passed: bool,
    pub notes: Vec<String>,
}

/// Compute-unit measurements recorded from a real LiteSVM run of the compiled `.so`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredCu {
    pub source: String,
    pub buy_swap_cu: Option<u64>,
    pub sell_swap_cu: Option<u64>,
    pub max_swap_cu: u64,
    pub within_single_tx_ceiling: bool,
    pub within_43000_model: bool,
}

/// Returns the repository root (`audit-harness/..`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".."))
}

/// Verifies the compiled program artifact. Missing files yield `passed = false` with notes (never panics).
pub fn verify_program_binary(root: &Path) -> BinaryCheck {
    let so_path = root.join("programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so");
    let lib_rs = root.join("programs/ratchet-dlmm-damm/src/lib.rs");
    let keypair_path = root.join("deploy/ratchet_dlmm_damm-keypair.json");
    let mut notes = Vec::new();

    let bytes = fs::read(&so_path).unwrap_or_default();
    let so_present = !bytes.is_empty();
    if !so_present {
        notes.push(format!("missing or empty .so at {}", so_path.display()));
    }
    let so_is_elf64 = bytes.len() >= 20 && bytes[..4] == [0x7f, b'E', b'L', b'F'] && bytes[4] == 2;
    let so_machine = if bytes.len() >= 20 {
        u16::from_le_bytes([bytes[18], bytes[19]])
    } else {
        0
    };
    let so_machine_is_bpf = so_machine == EM_BPF || so_machine == EM_SBF;
    let needle = b"entrypoint";
    let so_exports_entrypoint = bytes.windows(needle.len()).any(|w| w == needle);
    let so_sha256 = if so_present { hex_upper(&sha256(&bytes)) } else { String::new() };

    let declare_id = fs::read_to_string(&lib_rs)
        .ok()
        .and_then(|src| {
            let marker = "declare_id!(\"";
            let start = src.find(marker)? + marker.len();
            let end = src[start..].find('"')? + start;
            Some(src[start..end].to_string())
        })
        .unwrap_or_default();
    if declare_id.is_empty() {
        notes.push("declare_id! not found in programs/ratchet-dlmm-damm/src/lib.rs".to_string());
    }

    let keypair_pubkey = fs::read_to_string(&keypair_path)
        .ok()
        .map(|s| {
            let nums: Vec<u8> = s
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .filter_map(|t| t.trim().parse::<u8>().ok())
                .collect();
            if nums.len() == 64 {
                base58_encode(&nums[32..])
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    if keypair_pubkey.is_empty() {
        notes.push("could not read 64-byte keypair at deploy/ratchet_dlmm_damm-keypair.json".to_string());
    }
    let declare_id_matches_keypair = !declare_id.is_empty() && declare_id == keypair_pubkey;
    if !declare_id.is_empty() && !keypair_pubkey.is_empty() && !declare_id_matches_keypair {
        notes.push(format!("declare_id {declare_id} != keypair pubkey {keypair_pubkey}"));
    }

    let jupiter_program_id = extract_quoted_after(
        &root.join("sdk/jupiter-adapter/src/lib.rs"),
        "RATCHET_PROGRAM_ID_STR: &str = \"",
    );
    let ts_program_id = extract_quoted_after(
        &root.join("sdk/ts-client/src/accounts.ts"),
        "DEFAULT_RATCHET_PROGRAM_ID = \"",
    );
    let example_config_program_id = extract_quoted_after(
        &root.join("deploy/launch.config.example.json"),
        "\"programId\": \"",
    );
    let local_config_path = root.join("deploy/launch.config.json");
    let launch_config_program_id = if local_config_path.exists() {
        extract_quoted_after(&local_config_path, "\"programId\": \"")
    } else {
        example_config_program_id.clone()
    };
    let declare_id_consistent_across_repo = !declare_id.is_empty()
        && jupiter_program_id == declare_id
        && ts_program_id == declare_id
        && example_config_program_id == declare_id
        && launch_config_program_id == declare_id
        && (!keypair_pubkey.is_empty() && declare_id_matches_keypair || keypair_pubkey.is_empty());
    if !declare_id_consistent_across_repo {
        notes.push(format!(
            "program_id mismatch: lib={declare_id}, keypair={keypair_pubkey}, jup={jupiter_program_id}, ts={ts_program_id}, example_cfg={example_config_program_id}, cfg={launch_config_program_id}"
        ));
    }

    let passed = so_present
        && so_is_elf64
        && so_machine_is_bpf
        && so_exports_entrypoint
        && declare_id_matches_keypair
        && declare_id_consistent_across_repo;

    BinaryCheck {
        so_path: "programs/ratchet-dlmm-damm/target/deploy/ratchet_dlmm_damm.so".to_string(),
        so_present,
        so_size_bytes: bytes.len() as u64,
        so_is_elf64,
        so_machine,
        so_machine_is_bpf,
        so_exports_entrypoint,
        so_sha256,
        declare_id,
        keypair_pubkey,
        jupiter_program_id,
        ts_program_id,
        launch_config_program_id,
        declare_id_matches_keypair,
        declare_id_consistent_across_repo,
        passed,
        notes,
    }
}

fn extract_quoted_after(path: &Path, marker: &str) -> String {
    fs::read_to_string(path)
        .ok()
        .and_then(|src| {
            let start = src.find(marker)? + marker.len();
            let end = src[start..].find('"')? + start;
            Some(src[start..end].to_string())
        })
        .unwrap_or_default()
}

/// Extracts `"key": <u64>` from a flat JSON text without a JSON dependency.
fn json_u64(text: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\"");
    let after = &text[text.find(&pat)? + pat.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Loads LiteSVM-measured swap CU from `RATCHET_MEASURED_SWAP_CU` (single max value) or
/// `audit-harness/measured_cu.json` (`{"buy_swap_cu": N, "sell_swap_cu": N}`), if available.
pub fn load_measured_cu(harness_dir: &Path) -> Option<MeasuredCu> {
    let (source, buy, sell) = if let Some(v) = std::env::var("RATCHET_MEASURED_SWAP_CU")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        ("env:RATCHET_MEASURED_SWAP_CU".to_string(), Some(v), None)
    } else {
        let path = harness_dir.join("measured_cu.json");
        let text = fs::read_to_string(&path).ok()?;
        let buy = json_u64(&text, "buy_swap_cu");
        let sell = json_u64(&text, "sell_swap_cu");
        buy.or(sell)?;
        ("audit-harness/measured_cu.json".to_string(), buy, sell)
    };
    let max_swap_cu = buy.into_iter().chain(sell).max()?;
    Some(MeasuredCu {
        source,
        buy_swap_cu: buy,
        sell_swap_cu: sell,
        max_swap_cu,
        within_single_tx_ceiling: max_swap_cu <= crate::layout::SINGLE_TX_CU_CEILING,
        within_43000_model: max_swap_cu <= crate::layout::HOT_PATH_SWAP_CU,
    })
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Base58 (Bitcoin alphabet) encoder used by Solana pubkeys.
pub fn base58_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let zeros = input.iter().take_while(|&&b| b == 0).count();
    let mut digits: Vec<u8> = Vec::new(); // little-endian base58 digits
    for &byte in input {
        let mut carry = byte as u32;
        for d in digits.iter_mut() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = String::new();
    out.extend(std::iter::repeat_n('1', zeros));
    out.extend(digits.iter().rev().map(|&d| ALPHABET[d as usize] as char));
    out
}

/// Minimal std-only SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

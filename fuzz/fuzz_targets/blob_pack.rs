#![no_main]

use libfuzzer_sys::fuzz_target;
use mbx_cache_core::{CacheDigest, fuzz_decode_blob_pack};
use std::sync::OnceLock;

const HEADER_LEN: usize = 1 + 32 + 8;
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }

    // Admit every digest header present in the input. Production still checks
    // that decoded entries were requested; this lets mutations progress deeper
    // into framing, length, file staging, and digest verification.
    let requested = data
        .windows(HEADER_LEN)
        .filter_map(|header| {
            let algorithm = match header[0] {
                1 => "blake3",
                2 => "sha256",
                _ => return None,
            };
            Some(CacheDigest {
                algorithm: algorithm.into(),
                hash: header[1..33]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                size: u64::from_be_bytes(header[33..41].try_into().unwrap()),
            })
        })
        .take(4096)
        .collect::<Vec<_>>();
    let staging = tempfile::tempdir().unwrap();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    });
    let _ = runtime.block_on(fuzz_decode_blob_pack(data, &requested, staging.path()));
});

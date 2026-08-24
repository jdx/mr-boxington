#![no_main]

use libfuzzer_sys::fuzz_target;
use mbx_cache_rustc::RustcDepInfo;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    if let Ok(contents) = std::str::from_utf8(data) {
        let _ = RustcDepInfo::parse(contents);
    }
});

#![no_main]

use libfuzzer_sys::fuzz_target;
use mbx_cache_cc::CcInvocation;
use std::ffi::OsString;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let arguments = text
        .split('\0')
        .take(4096)
        .map(OsString::from)
        .collect::<Vec<_>>();
    let _ = CcInvocation::parse(&arguments);
});

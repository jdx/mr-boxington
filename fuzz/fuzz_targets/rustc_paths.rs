#![no_main]

use libfuzzer_sys::fuzz_target;
use mbx_cache_rustc::{PathMapping, RustcInvocation, normalize_mapped_path};
use std::ffi::OsString;
use std::path::Path;

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
    let _ = RustcInvocation::parse(&arguments);

    let mappings = PathMapping::ordered(&[
        PathMapping::new("/workspace", "workspace"),
        PathMapping::new("/workspace/target", "target"),
    ]);
    let _ = normalize_mapped_path(Path::new(text.as_ref()), Path::new("/workspace"), &mappings);
});

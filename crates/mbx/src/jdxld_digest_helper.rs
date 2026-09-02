//! Internal bridge from jdxld's input paths to the session file-digest ledger.

use eyre::{Context, Result, bail};
use mbx_cache_core::{
    CacheDigest, FileDigestCache, FileDigestScope, FileIdentity, RecordedFileDigest,
};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::ExitCode;

pub const ARG: &str = "__jdxld_digests_v1";
const MAGIC: &[u8; 8] = b"JDXLDG01";

pub fn run() -> ExitCode {
    match resolve_request(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mbx: jdxld digest helper failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn resolve_request(input: &mut impl Read, output: &mut impl Write) -> Result<()> {
    let paths = read_request(input)?;
    let digests = resolve(&paths, crate::session::file_digest_cache())?;
    write_response(output, &digests)
}

fn resolve(paths: &[PathBuf], cache: &dyn FileDigestCache) -> Result<Vec<[u8; 32]>> {
    let identities = paths
        .iter()
        .map(|path| {
            let metadata = std::fs::metadata(path)
                .wrap_err_with(|| format!("failed to inspect `{}`", path.display()))?;
            FileIdentity::describe(path, &metadata)
                .ok_or_else(|| eyre::eyre!("failed to identify `{}`", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let found = cache.find(FileDigestScope::Content, &identities);
    let mut recorded = Vec::new();
    let mut digests = Vec::with_capacity(paths.len());
    for ((path, identity), found) in paths.iter().zip(&identities).zip(found) {
        let digest = match found.filter(|digest| usable_digest(digest, identity.len)) {
            Some(digest) => digest,
            None => {
                let digest = CacheDigest::blake3_file(path)
                    .wrap_err_with(|| format!("failed to hash `{}`", path.display()))?;
                if !identity.still_describes()? {
                    bail!("linker input changed while hashing `{}`", path.display());
                }
                recorded.push(RecordedFileDigest {
                    file: identity.clone(),
                    digest: digest.clone(),
                });
                digest
            }
        };
        digests.push(decode_blake3(&digest)?);
    }
    cache.record(FileDigestScope::Content, recorded);
    Ok(digests)
}

fn usable_digest(digest: &CacheDigest, expected_size: u64) -> bool {
    digest.algorithm == "blake3"
        && digest.size == expected_size
        && digest.hash.len() == 64
        && digest.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode_blake3(digest: &CacheDigest) -> Result<[u8; 32]> {
    if !usable_digest(digest, digest.size) {
        bail!("file-digest ledger returned an invalid BLAKE3 digest");
    }
    let mut bytes = [0; 32];
    for (destination, pair) in bytes.iter_mut().zip(digest.hash.as_bytes().chunks_exact(2)) {
        let text = std::str::from_utf8(pair)?;
        *destination = u8::from_str_radix(text, 16)?;
    }
    Ok(bytes)
}

fn read_request(input: &mut impl Read) -> Result<Vec<PathBuf>> {
    let mut magic = [0; 8];
    input.read_exact(&mut magic)?;
    if &magic != MAGIC {
        bail!("invalid jdxld digest request");
    }
    let count = read_u32(input)? as usize;
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let length = read_u32(input)? as usize;
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        paths.push(PathBuf::from(OsString::from_vec(bytes)));
    }
    let mut trailing = [0];
    if input.read(&mut trailing)? != 0 {
        bail!("jdxld digest request contains trailing data");
    }
    Ok(paths)
}

fn write_response(output: &mut impl Write, digests: &[[u8; 32]]) -> Result<()> {
    output.write_all(MAGIC)?;
    write_u32(
        output,
        digests.len().try_into().context("too many digests")?,
    )?;
    for digest in digests {
        output.write_all(digest)?;
    }
    output.flush()?;
    Ok(())
}

fn read_u32(input: &mut impl Read) -> Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_u32(output: &mut impl Write, value: u32) -> Result<()> {
    output.write_all(&value.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbx_cache_core::NoFileDigestCache;

    #[test]
    fn hashes_a_ledger_miss() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.o");
        std::fs::write(&path, b"linker input").unwrap();
        let digest = resolve(&[path], &NoFileDigestCache).unwrap().pop().unwrap();
        assert_eq!(
            digest,
            decode_blake3(&CacheDigest::blake3(b"linker input")).unwrap()
        );
    }
}

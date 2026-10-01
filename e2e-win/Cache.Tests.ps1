Describe 'Rust compilation cache' {
    BeforeEach {
        $script:OriginalDir = Get-Location
        $script:OriginalCacheDir = [Environment]::GetEnvironmentVariable('MBX_CACHE_DIR', 'Process')
        $script:OriginalGcAuto = [Environment]::GetEnvironmentVariable('MBX_GC_AUTO', 'Process')
        $script:OriginalTargetViews = [Environment]::GetEnvironmentVariable('MBX_TARGET_VIEWS', 'Process')
        $script:OriginalTargetDir = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
        $script:TestRoot = Join-Path $TestDrive ([System.Guid]::NewGuid().ToString())
        New-Item -ItemType Directory -Path $script:TestRoot | Out-Null
        Set-Location $script:TestRoot
        $env:MBX_CACHE_DIR = Join-Path $script:TestRoot 'cache'
        $env:MBX_GC_AUTO = '0'
        $env:MBX_TARGET_VIEWS = '0'
        Remove-Item Env:CARGO_TARGET_DIR -ErrorAction Ignore
    }

    AfterEach {
        Set-Location $script:OriginalDir
        foreach ($item in @(
            @{ Name = 'MBX_CACHE_DIR'; Value = $script:OriginalCacheDir },
            @{ Name = 'MBX_GC_AUTO'; Value = $script:OriginalGcAuto },
            @{ Name = 'MBX_TARGET_VIEWS'; Value = $script:OriginalTargetViews },
            @{ Name = 'CARGO_TARGET_DIR'; Value = $script:OriginalTargetDir }
        )) {
            if ($null -eq $item.Value) {
                Remove-Item "Env:$($item.Name)" -ErrorAction Ignore
            } else {
                [Environment]::SetEnvironmentVariable($item.Name, $item.Value, 'Process')
            }
        }
    }

    It 'caches a library with a native search path' {
        New-Item -ItemType Directory -Path src | Out-Null
        @'
[package]
name = "native-search-fixture"
version = "0.1.0"
edition = "2024"
'@ | Set-Content -Encoding utf8 Cargo.toml
        @'
fn main() {
    println!("cargo:rustc-link-search=native={}", std::env::var("OUT_DIR").unwrap());
}
'@ | Set-Content -Encoding utf8 build.rs
        'pub fn value() -> u8 { 1 }' | Set-Content -Encoding utf8 src\lib.rs

        $lockfile = & cargo generate-lockfile 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $lockfile

        $cold = & mbx build 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $cold
        $cold | Should -Match 'stored locally'

        Remove-Item -Recurse -Force target
        $warm = & mbx build 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $warm
        $warm | Should -Match 'mbx\[cache\]: (?:object cache: )?[1-9][0-9]* hits'
    }

    It 'restores a natively linked executable' {
        New-Item -ItemType Directory -Path src | Out-Null
        @'
[package]
name = "native-link-fixture"
version = "0.1.0"
edition = "2024"
'@ | Set-Content -Encoding utf8 Cargo.toml
        'fn main() { println!("linked"); }' | Set-Content -Encoding utf8 src\main.rs

        $lockfile = & cargo generate-lockfile 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $lockfile
        $cold = & mbx build --release 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $cold

        Remove-Item -Recurse -Force target
        $warm = & mbx build --release 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $warm
        $warm | Should -Match 'mbx\[cache\]: (?:object cache: )?[1-9][0-9]* hits'
        Test-Path target\release\native-link-fixture.exe | Should -BeTrue
    }

    # A stack size in RUSTFLAGS reaches every proc macro, build script, and
    # binary, and used to bypass all of them. A MSVC link is not reproducible,
    # so each fresh proc-macro DLL then hashed differently and every crate
    # compiled against it missed as well.
    It 'restores proc macros and build scripts linked with a stack size' {
        New-Item -ItemType Directory -Path src, macros\src | Out-Null
        @'
[package]
name = "stack-fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
stack-macros = { path = "macros" }
'@ | Set-Content -Encoding utf8 Cargo.toml
        @'
[package]
name = "stack-macros"
version = "0.1.0"
edition = "2024"

[lib]
proc-macro = true
'@ | Set-Content -Encoding utf8 macros\Cargo.toml
        @'
use proc_macro::TokenStream;

#[proc_macro]
pub fn answer(_: TokenStream) -> TokenStream {
    "42u8".parse().unwrap()
}
'@ | Set-Content -Encoding utf8 macros\src\lib.rs
        'fn main() { println!("cargo:rerun-if-changed=build.rs"); }' | Set-Content -Encoding utf8 build.rs
        'fn main() { println!("{}", stack_macros::answer!()); }' | Set-Content -Encoding utf8 src\main.rs

        $original = [Environment]::GetEnvironmentVariable('RUSTFLAGS', 'Process')
        $env:RUSTFLAGS = '-C link-arg=/STACK:8000000'
        try {
            $lockfile = & cargo generate-lockfile 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $lockfile
            $cold = & mbx build 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $cold

            Remove-Item -Recurse -Force target
            $warm = & mbx build 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $warm
            # Both the proc-macro DLL and the build script restore, so
            # nothing is left to compile and nothing bypasses.
            $warm | Should -Match 'mbx\[cache\]: (?:object cache: )?[1-9][0-9]* hits, 0 misses' -Because $warm
            $warm | Should -Not -Match '[1-9][0-9]* bypassed' -Because $warm
            $run = & target\debug\stack-fixture.exe 2>&1 | Out-String
            $run.Trim() | Should -Be '42'
        } finally {
            [Environment]::SetEnvironmentVariable('RUSTFLAGS', $original, 'Process')
        }
    }

    It 'restores an MSVC object compiled through mbx exec' {
        New-Item -ItemType Directory -Path src | Out-Null
        'int answer(void) { return 42; }' | Set-Content -Encoding ascii src\hello.c

        $cold = & mbx exec cl.exe /nologo /Z7 /Brepro /Fohello.obj /c src\hello.c 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $cold
        Test-Path hello.obj | Should -BeTrue

        Remove-Item hello.obj
        $warm = & mbx exec cl.exe /nologo /Z7 /Brepro /Fohello.obj /c src\hello.c 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $warm
        $warm | Should -Match 'mbx\[cache\]: (?:object cache: )?1 hits'
        Test-Path hello.obj | Should -BeTrue
    }
}

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
        $warm | Should -Match 'mbx\[cache\]: [1-9][0-9]* hits'
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
        $warm | Should -Match 'mbx\[cache\]: [1-9][0-9]* hits'
        Test-Path target\release\native-link-fixture.exe | Should -BeTrue
    }

    It 'restores an MSVC object compiled through mbx exec' {
        'int answer(void) { return 42; }' | Set-Content -Encoding ascii hello.c

        $cold = & mbx exec cl.exe /nologo /Z7 /Brepro /Fohello.obj /c hello.c 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $cold
        Test-Path hello.obj | Should -BeTrue

        Remove-Item hello.obj
        $warm = & mbx exec cl.exe /nologo /Z7 /Brepro /Fohello.obj /c hello.c 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 0 -Because $warm
        $warm | Should -Match 'mbx\[cache\]: 1 hit'
        Test-Path hello.obj | Should -BeTrue
    }
}

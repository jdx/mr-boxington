Describe 'Cargo command dispatch' {
    BeforeEach {
        $script:OriginalDir = Get-Location
        $script:OriginalCacheDir = [Environment]::GetEnvironmentVariable('MBX_CACHE_DIR', 'Process')
        $script:OriginalGcAuto = [Environment]::GetEnvironmentVariable('MBX_GC_AUTO', 'Process')
        $script:OriginalTargetDir = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
        $script:TestRoot = Join-Path $TestDrive ([System.Guid]::NewGuid().ToString())
        New-Item -ItemType Directory -Path $script:TestRoot | Out-Null
        Set-Location $script:TestRoot
        $env:MBX_CACHE_DIR = Join-Path $script:TestRoot 'cache'
        $env:MBX_GC_AUTO = '0'
        Remove-Item Env:CARGO_TARGET_DIR -ErrorAction Ignore
    }

    AfterEach {
        Set-Location $script:OriginalDir
        foreach ($item in @(
            @{ Name = 'MBX_CACHE_DIR'; Value = $script:OriginalCacheDir },
            @{ Name = 'MBX_GC_AUTO'; Value = $script:OriginalGcAuto },
            @{ Name = 'CARGO_TARGET_DIR'; Value = $script:OriginalTargetDir }
        )) {
            if ($null -eq $item.Value) {
                Remove-Item "Env:$($item.Name)" -ErrorAction Ignore
            } else {
                [Environment]::SetEnvironmentVariable($item.Name, $item.Value, 'Process')
            }
        }
    }

    It 'forwards cargo new' {
        $output = & mbx new --vcs none new-project 2>&1 | Out-String
        $status = $LASTEXITCODE

        $status | Should -Be 0 -Because $output
        Test-Path -LiteralPath 'new-project\Cargo.toml' | Should -BeTrue
    }

    It 'forwards cargo init' {
        New-Item -ItemType Directory -Path initialized-project | Out-Null
        Set-Location initialized-project

        $output = & mbx init --vcs none 2>&1 | Out-String
        $status = $LASTEXITCODE

        $status | Should -Be 0 -Because $output
        Test-Path -LiteralPath 'Cargo.toml' | Should -BeTrue
    }

    It 'forwards cargo aliases that mbx does not know' {
        New-Item -ItemType Directory -Path .cargo | Out-Null
        @"
[alias]
mbx-probe = "new --vcs none"
"@ | Set-Content -Encoding utf8 .cargo\config.toml

        $output = & mbx mbx-probe alias-project 2>&1 | Out-String
        $status = $LASTEXITCODE

        $status | Should -Be 0 -Because $output
        Test-Path -LiteralPath 'alias-project\Cargo.toml' | Should -BeTrue
    }

    It 'lets cargo reject an unknown command' {
        $output = & mbx command-added-after-mbx --future-flag value 2>&1 | Out-String
        $status = $LASTEXITCODE

        $status | Should -Not -Be 0
        $output | Should -Match 'no such command: .*command-added-after-mbx'
    }

    It 'installs and dispatches the cargo.exe shim' {
        $mbx = (Get-Command mbx).Source
        $originalPath = $env:PATH
        $originalDisable = $env:MBX_DISABLE
        try {
            $output = & $mbx setup 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $output

            $localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
            $shimDir = Join-Path $localAppData 'mbx\bin'
            Test-Path -LiteralPath (Join-Path $shimDir 'cargo.exe') | Should -BeTrue
            Test-Path -LiteralPath (Join-Path $shimDir 'mbx-target') | Should -BeTrue
            $statusOutput = & $mbx setup --status 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $statusOutput

            # A versioned installer may remove the exact binary setup recorded.
            # The shim and status checks should then follow the active mbx on PATH.
            $missingTarget = Join-Path $shimDir 'removed-version\mbx.exe'
            $targetBytes = [Text.Encoding]::Unicode.GetBytes($missingTarget)
            [IO.File]::WriteAllBytes((Join-Path $shimDir 'mbx-target'), $targetBytes)
            $mbxDir = Split-Path -Parent $mbx
            $env:PATH = "$mbxDir;$originalPath"
            $statusOutput = & $mbx setup --status 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $statusOutput

            $env:PATH = "$shimDir;$mbxDir;$originalPath"
            $env:MBX_DISABLE = '1'
            $version = & cargo.exe --version 2>&1 | Out-String
            $LASTEXITCODE | Should -Be 0 -Because $version
            $version | Should -Match '^cargo '
        } finally {
            $env:PATH = $originalPath
            if ($null -eq $originalDisable) {
                Remove-Item Env:MBX_DISABLE -ErrorAction Ignore
            } else {
                $env:MBX_DISABLE = $originalDisable
            }
        }
    }
}

param(
    [string]$TestName
)

# Load Pester before Enter-VsDevShell rewrites the environment: on a profile
# path with non-ASCII characters it can mangle PSModulePath, after which only
# the Pester 3 that ships with Windows is found and no test runs.
Import-Module Pester -MinimumVersion 5.0 -ErrorAction Stop

if (-not (Get-Command cl.exe -ErrorAction Ignore)) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        throw 'Visual Studio Installer could not be found; the Windows cache tests require MSVC'
    }
    $installation = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if (-not $installation) {
        throw 'An installed Visual Studio C++ toolchain could not be found'
    }
    Import-Module (Join-Path $installation 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    Enter-VsDevShell -VsInstallPath $installation -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
}

$config = New-PesterConfiguration
$config.Run.Path = $PSScriptRoot
$config.Run.Exit = $true
$config.TestResult.Enabled = $true

if ($TestName) {
    $config.Filter.FullName = $TestName
}

$env:PATH = "$PSScriptRoot\..\target\mbx-bootstrap;$env:PATH"

Invoke-Pester -Configuration $config

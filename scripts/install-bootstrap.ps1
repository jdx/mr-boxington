# Install the freshly built bootstrap mbx for the Windows `bootstrap` task.
#
# The previous bootstrap binary may still be running, for example the
# `gc --automatic` it starts after a build, and Windows refuses to replace a
# running executable. ReplaceFile can still move it aside, so leftovers are
# removed once nothing runs them.
$ErrorActionPreference = 'Stop'

$directory = 'target/mbx-bootstrap'
$built = 'target/mbx-bootstrap-build/debug/mbx.exe'
$attempts = 5

New-Item -ItemType Directory -Force $directory | Out-Null
Get-ChildItem $directory -Filter '*.old' | ForEach-Object {
    $leftover = $_.FullName
    # Fails while still running, or after another run removed it first. The
    # second case throws even with -ErrorAction SilentlyContinue.
    try {
        Remove-Item -Force $leftover
    } catch {
        Write-Verbose "kept ${leftover}: $_"
    }
}

# .NET resolves relative paths against the process directory, not $PWD.
$root = (Resolve-Path $directory).Path
$installed = Join-Path $root 'mbx.exe'
# A complete copy under a name no other run uses, so a failed copy or an
# overlapping run never leaves a partial or missing mbx.exe.
$staged = Join-Path $root "mbx-$([guid]::NewGuid()).new"

try {
    Copy-Item $built $staged
    foreach ($attempt in 1..$attempts) {
        try {
            # An atomic replace, which Windows refuses while mbx.exe runs.
            [IO.File]::Move($staged, $installed, $true)
            return
        } catch [UnauthorizedAccessException], [IO.IOException] {
            $failure = $_
        } catch {
            $failure = $_
            break
        }
        $aside = Join-Path $root "mbx-$([guid]::NewGuid()).old"
        try {
            # ReplaceFile moves the running executable aside and puts the new
            # one in its place.
            [IO.File]::Replace($staged, $installed, $aside)
            return
        } catch {
            # Another run may have replaced or removed it meanwhile; try again.
            $failure = $_
            # ReplaceFile can fail after moving the old file aside.
            if ((Test-Path $aside) -and -not (Test-Path $installed)) {
                try {
                    [IO.File]::Move($aside, $installed)
                } catch {
                    Write-Warning "could not restore $installed from ${aside}: $_"
                }
            }
        }
    }
    throw $failure
} finally {
    # Nothing is left after a successful install. After a failure, including a
    # copy that stopped partway, remove this run's partial or unused copy.
    if (Test-Path -LiteralPath $staged) {
        try {
            Remove-Item -Force -LiteralPath $staged
        } catch {
            Write-Warning "could not remove ${staged}: $_"
        }
    }
}

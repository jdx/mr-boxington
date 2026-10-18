# Install the freshly built bootstrap mbx for the Windows `bootstrap` task.
#
# The previous bootstrap binary may still be running, for example the
# `gc --automatic` it starts after a build, and Windows refuses to replace a
# running executable. It does allow renaming one, so move it aside when it is
# in use and remove such leftovers once nothing runs them.
$ErrorActionPreference = 'Stop'

$directory = 'target/mbx-bootstrap'
$built = 'target/mbx-bootstrap-build/debug/mbx.exe'
$installed = Join-Path $directory 'mbx.exe'
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

# A complete copy under a name no other run uses, so a failed copy or an
# overlapping run never leaves a partial or missing mbx.exe.
$staged = Join-Path $directory "mbx-$([guid]::NewGuid()).new"
Copy-Item $built $staged

$aside = $null
foreach ($attempt in 1..$attempts) {
    try {
        Move-Item -Force $staged $installed
        return
    } catch [System.IO.IOException] {
        # Replacing a running executable fails this way; other errors do not
        # call for moving the installed binary.
        $failure = $_
    } catch {
        $failure = $_
        break
    }
    if ($attempt -eq $attempts) {
        break
    }
    # Another run may have moved it already, which leaves nothing to rename.
    $aside = Join-Path $directory "mbx-$([guid]::NewGuid()).old"
    Move-Item $installed $aside -ErrorAction SilentlyContinue
}
if ($aside -and (Test-Path $aside) -and -not (Test-Path $installed)) {
    try {
        Move-Item $aside $installed
    } catch {
        Write-Warning "could not restore $installed from ${aside}: $_"
    }
}
Remove-Item -Force -ErrorAction SilentlyContinue $staged
throw $failure

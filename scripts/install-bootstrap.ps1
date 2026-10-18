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

New-Item -ItemType Directory -Force $directory | Out-Null
Get-ChildItem $directory -Filter '*.old' | ForEach-Object {
    # Still running, or another run removed it first.
    try { Remove-Item -Force $_.FullName } catch {}
}

# A complete copy under a name no other run uses, so a failed copy or an
# overlapping run never leaves a partial or missing mbx.exe.
$staged = Join-Path $directory "mbx-$([guid]::NewGuid()).new"
Copy-Item $built $staged

$aside = $null
foreach ($attempt in 1..5) {
    try {
        Move-Item -Force $staged $installed
        return
    } catch {
        $failure = $_
    }
    # Another run may have moved it already, which leaves nothing to rename.
    $aside = Join-Path $directory "mbx-$([guid]::NewGuid()).old"
    Move-Item $installed $aside -ErrorAction SilentlyContinue
}
if ((Test-Path $aside) -and -not (Test-Path $installed)) {
    Move-Item $aside $installed
}
Remove-Item -Force -ErrorAction Ignore $staged
throw $failure

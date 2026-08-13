param(
    [string]$BinaryDir
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$runtime = Join-Path $root "vendor/angle/windows-x86_64"
$expected = @{
    "libEGL.dll" = "41D933629CC0DD90190EAAC5DDFC65B822974C809F8B3E79E5ACA64B836D3B92"
    "libGLESv2.dll" = "ABBA3DB61100CA558BEB1DE2903719A29CCFFB89E54DBCB413D94F7C8EAD9AF7"
}

function Test-AngleDirectory([string]$Directory, [string]$Label) {
    foreach ($entry in $expected.GetEnumerator()) {
        $path = Join-Path $Directory $entry.Key
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Missing ANGLE runtime file in ${Label}: $path"
        }

        $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        if ($actual -ne $entry.Value) {
            throw "ANGLE hash mismatch for $($entry.Key) in ${Label}: expected $($entry.Value), got $actual"
        }

        $bytes = [System.IO.File]::ReadAllBytes($path)
        if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
            throw "ANGLE file is not a PE image: $path"
        }
        $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
        if ($peOffset -lt 0 -or $peOffset + 6 -gt $bytes.Length) {
            throw "ANGLE file has an invalid PE header: $path"
        }
        $machine = [BitConverter]::ToUInt16($bytes, $peOffset + 4)
        if ($machine -ne 0x8664) {
            throw "ANGLE file is not x86-64: $path (machine 0x$($machine.ToString('X4')))"
        }
    }
}

Test-AngleDirectory $runtime "vendored runtime"
if ($BinaryDir) {
    Test-AngleDirectory $BinaryDir "build output"
}

Write-Host "ANGLE runtime hashes and x86-64 PE headers verified."

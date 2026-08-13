param(
    [string]$Definition = (Join-Path (Split-Path -Parent $PSScriptRoot) "wix/main.wxs")
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Definition -PathType Leaf)) {
    throw "WiX definition was not found: $Definition"
}

[xml]$document = Get-Content -LiteralPath $Definition -Raw
$namespaces = New-Object System.Xml.XmlNamespaceManager($document.NameTable)
$namespaces.AddNamespace("wix", "http://schemas.microsoft.com/wix/2006/wi")

$product = $document.SelectSingleNode("/wix:Wix/wix:Product", $namespaces)
if (-not $product) {
    throw "WiX definition does not contain a Product element"
}
if ($product.GetAttribute("Version") -ne '$(var.Version)') {
    throw "WiX Product.Version must use the cargo-wix Version variable"
}

$majorUpgrade = $product.SelectSingleNode("wix:MajorUpgrade", $namespaces)
if (-not $majorUpgrade) {
    throw "WiX definition does not contain a MajorUpgrade element"
}
if ($majorUpgrade.GetAttribute("AllowSameVersionUpgrades") -ne "yes") {
    throw "WiX MajorUpgrade must allow same-version upgrades for rolling nightly MSI packages"
}

Write-Host "WiX same-version major-upgrade policy verified."

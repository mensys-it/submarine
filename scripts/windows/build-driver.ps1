# Builds and signs the split tunnel driver. Requirements: Visual Studio 2022 with
# the "Desktop development with C++" workload, the Windows SDK and the WDK.
#
#   .\scripts\windows\build-driver.ps1                                   # new test certificate
#   .\scripts\windows\build-driver.ps1 -PfxPath c.pfx -PfxPassword ...   # given certificate
#
# Output in dist\driver: submarine-split-tunnel.sys and, for self-signed builds,
# submarine-test-driver.cer, to trust on test machines. The certificate and its
# password may also come from SUBMARINE_DRIVER_PFX and SUBMARINE_DRIVER_PFX_PASSWORD,
# the PFX either as a path or as base64 text (a CI secret).
param(
    [string]$PfxPath = $env:SUBMARINE_DRIVER_PFX,
    [string]$PfxPassword = $env:SUBMARINE_DRIVER_PFX_PASSWORD
)
$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot\..\.."
$project = "$root\third_party\win-split-tunnel\src\submarine-split-tunnel.vcxproj"
$out = "$root\dist\driver"
New-Item -ItemType Directory -Force -Path $out | Out-Null

# 1. build, unsigned: signing is done below with our own certificate
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$msbuild = & $vswhere -latest -requires Microsoft.Component.MSBuild -find "MSBuild\**\Bin\MSBuild.exe" | Select-Object -First 1
if (-not $msbuild) { throw "MSBuild not found: install Visual Studio 2022 and the WDK" }

& $msbuild $project /p:Configuration=Release /p:Platform=x64 /p:SignMode=Off /m /nologo /v:minimal
if ($LASTEXITCODE -ne 0) { throw "driver build failed" }

$sys = Get-ChildItem -Recurse -Path "$root\third_party\win-split-tunnel" -Filter submarine-split-tunnel.sys |
    Where-Object { $_.FullName -match "Release" } | Select-Object -First 1
if (-not $sys) { throw "submarine-split-tunnel.sys not found after build" }
Copy-Item $sys.FullName "$out\submarine-split-tunnel.sys" -Force

# 2. signing certificate: the given PFX, or a new self-signed one exported as .cer
if ($PfxPath) {
    $pfx = if (Test-Path $PfxPath) { $PfxPath } else {
        # a base64 PFX, e.g. from a CI secret
        $tmp = New-TemporaryFile
        [IO.File]::WriteAllBytes($tmp, [Convert]::FromBase64String($PfxPath))
        $tmp.FullName
    }
    $secure = ConvertTo-SecureString $PfxPassword -AsPlainText -Force
    $cert = Import-PfxCertificate -FilePath $pfx -Password $secure -CertStoreLocation Cert:\CurrentUser\My
} else {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Submarine Test Driver" `
        -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(2)
    Export-Certificate -Cert $cert -FilePath "$out\submarine-test-driver.cer" | Out-Null
}

# 3. signature with the newest signtool of the Windows SDK
$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
& $signtool.FullName sign /fd sha256 /sha1 $cert.Thumbprint /v "$out\submarine-split-tunnel.sys"
if ($LASTEXITCODE -ne 0) { throw "signing failed" }
Write-Host "driver: $out\submarine-split-tunnel.sys"

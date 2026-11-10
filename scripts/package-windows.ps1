# Builds a self-contained OpenPixels for 64-bit Windows into artifacts/: a zip, and an
# installer when Inno Setup 6 is installed. Run it on Windows: the Rust engine is built
# for the machine doing the build.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$project = "desktop/OpenPixels.Desktop/OpenPixels.Desktop.csproj"
$version = (Select-String -Path $project -Pattern "<Version>(.*)</Version>").Matches[0].Groups[1].Value
$out = "artifacts/windows-x64"
$app = "$out/OpenPixels"
New-Item -ItemType Directory -Force artifacts | Out-Null
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue

dotnet publish desktop/OpenPixels.Desktop -c Release -r win-x64 --self-contained true -o $app
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Copy-Item LICENSE $app

# HEIC: libheif and its HEVC decoder libde265 (both LGPL, loaded at run time), from
# vcpkg when installed with `vcpkg install "libheif[core,libde265]:x64-windows"`. The
# default features would add the GPL x265 encoder, which is not needed.
$vcpkg = if ($env:OPENPIXELS_VCPKG_ROOT) { $env:OPENPIXELS_VCPKG_ROOT } else { $env:VCPKG_INSTALLATION_ROOT }
$heif = if ($vcpkg) { Join-Path $vcpkg "installed/x64-windows" } else { $null }
if ($heif -and (Test-Path "$heif/bin/heif.dll")) {
  Copy-Item "$heif/bin/heif.dll", "$heif/bin/libde265.dll" $app
  New-Item -ItemType Directory -Force "$app/licenses" | Out-Null
  Copy-Item "$heif/share/libheif/copyright" "$app/licenses/libheif.txt"
  Copy-Item "$heif/share/libde265/copyright" "$app/licenses/libde265.txt"
  Write-Output "bundled libheif for HEIC"
} else {
  Write-Output "libheif not found in vcpkg; this build cannot open HEIC"
}

$zip = "artifacts/OpenPixels-$version-windows-x64.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $app -DestinationPath $zip
Write-Output "$zip"

$iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "${env:ProgramFiles}\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) {
  Write-Output "Inno Setup 6 not found; skipping the installer"
  exit 0
}
& $iscc "/DVersion=$version" "/DSource=$((Resolve-Path $app).Path)" "/DOutput=$((Resolve-Path artifacts).Path)" desktop/packaging/windows/OpenPixels.iss
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Write-Output "artifacts/OpenPixels-$version-windows-x64-setup.exe"

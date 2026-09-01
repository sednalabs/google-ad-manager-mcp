[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\google-ad-manager-mcp\bin"
)

$ErrorActionPreference = "Stop"
$repository = "sednalabs/google-ad-manager-mcp"

if ($Version -eq "latest") {
    $release = Invoke-RestMethod `
        -Uri "https://api.github.com/repos/$repository/releases/latest" `
        -Headers @{ "User-Agent" = "google-ad-manager-mcp-installer" }
    $Version = $release.tag_name
}

if ($Version -notmatch '^v[0-9]') {
    throw "Release version must look like v0.1.1"
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "Only 64-bit Windows is supported"
}

$asset = "google-ad-manager-mcp-$Version-x86_64-pc-windows-msvc.zip"
$baseUrl = "https://github.com/$repository/releases/download/$Version"
$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $archivePath = Join-Path $tempDir $asset
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"
    Invoke-WebRequest -Uri "$baseUrl/$asset" -OutFile $archivePath
    Invoke-WebRequest -Uri "$baseUrl/SHA256SUMS" -OutFile $checksumsPath

    $escapedAsset = [regex]::Escape($asset)
    $checksumLine = Get-Content $checksumsPath | Where-Object {
        $_ -match "^[0-9a-fA-F]{64}\s+\*?$escapedAsset$"
    } | Select-Object -First 1
    if (-not $checksumLine) {
        throw "Checksum for $asset was not found"
    }
    $expected = ($checksumLine -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $archivePath).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum verification failed for $asset"
    }

    $expanded = Join-Path $tempDir "expanded"
    Expand-Archive -Path $archivePath -DestinationPath $expanded
    $binary = Get-ChildItem -Path $expanded -Filter "google-ad-manager-mcp.exe" -Recurse |
        Select-Object -First 1
    if (-not $binary) {
        throw "Release archive did not contain google-ad-manager-mcp.exe"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item $binary.FullName (Join-Path $InstallDir "google-ad-manager-mcp.exe") -Force
    Write-Host "Installed google-ad-manager-mcp $Version to $InstallDir\google-ad-manager-mcp.exe"
    Write-Host "Use this executable as the command in your MCP client configuration."
    if (($env:PATH -split ';') -notcontains $InstallDir) {
        Write-Host "Add $InstallDir to PATH to run google-ad-manager-mcp."
    }
}
finally {
    Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}

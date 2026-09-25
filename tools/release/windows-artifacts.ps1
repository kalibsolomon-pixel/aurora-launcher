param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][ValidateSet('nsis', 'msi', 'both')][string]$Format,
    [Parameter(Mandatory = $true)][ValidateSet('Pack', 'Verify')][string]$Mode,
    [Parameter(Mandatory = $true)][string]$SourceSha,
    [Parameter(Mandatory = $true)][string]$OutputDirectory,
    [string]$RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
)

$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$') { throw 'Invalid launcher version' }
if ($SourceSha -notmatch '^[0-9a-f]{40}$') { throw 'SourceSha must be an exact lowercase commit SHA' }

function Get-MsiProperty([string]$Path, [string]$Name) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $installer.OpenDatabase($Path, 0)
    $view = $database.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$Name'")
    $view.Execute() | Out-Null
    $record = $view.Fetch()
    if ($null -eq $record) { throw "MSI is missing $Name" }
    $value = $record.StringData(1)
    $view.Close() | Out-Null
    return $value.Trim()
}

function Assert-Metadata([string]$Path, [string]$Kind) {
    if ($Kind -eq 'msi') {
        $actualVersion = Get-MsiProperty $Path 'ProductVersion'
        $actualName = Get-MsiProperty $Path 'ProductName'
    } else {
        $info = (Get-Item -LiteralPath $Path).VersionInfo
        $actualVersion = $info.ProductVersion
        $actualName = $info.ProductName
    }
    if ($actualVersion -ne $Version -or $actualName -ne 'Aurora Launcher') {
        throw "Installer metadata mismatch in $(Split-Path $Path -Leaf): version=$actualVersion name=$actualName"
    }
}

$names = [ordered]@{
    nsis = "Aurora Launcher_${Version}_x64-setup.exe"
    msi = "Aurora Launcher_${Version}_x64_en-US.msi"
}
$selected = if ($Format -eq 'both') { @('nsis', 'msi') } else { @($Format) }
$manifestPath = Join-Path $OutputDirectory 'release-assets.json'

if ($Mode -eq 'Pack') {
    $nsisScript = Join-Path $RepositoryRoot 'src-tauri/target/release/nsis/x64/installer.nsi'
    if (-not (Test-Path -LiteralPath $nsisScript -PathType Leaf) -or
        (Get-Content -LiteralPath $nsisScript -Raw) -notmatch 'shortcuts\.nsh') {
        throw 'Generated NSIS installer is missing the shortcut conflict guard'
    }
    $appExe = Join-Path $RepositoryRoot 'src-tauri/target/release/aurora-launcher.exe'
    if (-not (Test-Path -LiteralPath $appExe -PathType Leaf)) { throw 'Release executable is missing' }
    $appInfo = (Get-Item -LiteralPath $appExe).VersionInfo
    if ($appInfo.ProductVersion -ne $Version -or $appInfo.ProductName -ne 'Aurora Launcher') {
        throw 'Release executable product metadata differs from requested version/name'
    }
    if ((Test-Path -LiteralPath $OutputDirectory) -and @(Get-ChildItem -LiteralPath $OutputDirectory -Force).Count -ne 0) {
        throw 'Release output directory is not empty'
    }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    $artifacts = @()
    foreach ($kind in @('nsis', 'msi')) {
        $folder = Join-Path $RepositoryRoot "src-tauri/target/release/bundle/$kind"
        $files = @(Get-ChildItem -LiteralPath $folder -File | Where-Object { if ($kind -eq 'nsis') { $_.Name -like '*-setup.exe' } else { $_.Extension -eq '.msi' } })
        if ($files.Count -ne 1 -or $files[0].Name -cne $names[$kind]) {
            throw "Expected exactly one $kind installer named $($names[$kind])"
        }
        Assert-Metadata $files[0].FullName $kind
        if ($kind -in $selected) {
            $destination = Join-Path $OutputDirectory $files[0].Name
            Copy-Item -LiteralPath $files[0].FullName -Destination $destination
            $item = Get-Item -LiteralPath $destination
            $artifacts += [pscustomobject]@{
                name = $item.Name
                format = $kind
                architecture = 'x64'
                sizeBytes = $item.Length
                sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    }
    [pscustomobject]@{ version = $Version; sourceSha = $SourceSha; artifacts = $artifacts } |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath -Encoding utf8
}

if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw 'Release artifact manifest is missing' }
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.version -cne $Version -or $manifest.sourceSha -cne $SourceSha) { throw 'Release artifact manifest version/source mismatch' }
if (@($manifest.artifacts).Count -ne $selected.Count) { throw 'Release artifact count is wrong' }
$expectedFiles = @('release-assets.json') + @($selected | ForEach-Object { $names[$_] })
$actualFiles = @(Get-ChildItem -LiteralPath $OutputDirectory -File | Select-Object -ExpandProperty Name)
if (@($actualFiles).Count -ne $expectedFiles.Count -or @(Compare-Object $expectedFiles $actualFiles).Count -ne 0) {
    throw 'Release artifact directory has missing or unexpected files'
}
foreach ($kind in $selected) {
    $entry = @($manifest.artifacts | Where-Object { $_.format -ceq $kind })
    if ($entry.Count -ne 1 -or $entry[0].name -cne $names[$kind] -or $entry[0].architecture -cne 'x64') {
        throw "Release manifest has an ambiguous or incorrect $kind entry"
    }
    $path = Join-Path $OutputDirectory $entry[0].name
    Assert-Metadata $path $kind
    $item = Get-Item -LiteralPath $path
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($item.Length -ne $entry[0].sizeBytes -or $hash -cne $entry[0].sha256) {
        throw "Release artifact size/hash mismatch: $($entry[0].name)"
    }
    Write-Output "$($entry[0].name) | $kind | x64 | $($item.Length) bytes | SHA-256 $hash | product $Version"
}

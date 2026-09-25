param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][ValidateSet('nsis', 'msi', 'both')][string]$Format,
    [Parameter(Mandatory = $true)][string]$SourceSha,
    [Parameter(Mandatory = $true)][string]$AssetsDirectory
)

$ErrorActionPreference = 'Stop'
if (-not $env:GH_TOKEN -or -not $env:GITHUB_REPOSITORY) { throw 'GitHub release environment is incomplete' }
& (Join-Path $PSScriptRoot 'windows-artifacts.ps1') -Version $Version -Format $Format -Mode Verify -SourceSha $SourceSha -OutputDirectory $AssetsDirectory
if (-not $?) { throw 'Transferred release artifacts failed verification' }

$manifestPath = Join-Path $AssetsDirectory 'release-assets.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$tag = "v$Version"
$notesPath = Join-Path $env:RUNNER_TEMP 'aurora-launcher-release-notes.md'
$notes = @(
    "Aurora Launcher $Version",
    '',
    'Windows installer artifacts are listed below with SHA-256 checksums. This launcher build includes the reviewed Aurora Client 2.1.2 production manifest for new instances; existing instances remain pinned.',
    '',
    '| File | Architecture | Bytes | SHA-256 |',
    '| --- | --- | ---: | --- |'
)
foreach ($asset in $manifest.artifacts) {
    $notes += "| $($asset.name) | $($asset.architecture) | $($asset.sizeBytes) | ``$($asset.sha256)`` |"
}
$notes | Set-Content -LiteralPath $notesPath -Encoding utf8

& gh release create $tag --draft --target $SourceSha --title "Aurora Launcher $Version" --notes-file $notesPath
if ($LASTEXITCODE -ne 0) { throw 'Could not create the draft release' }
$files = @($manifest.artifacts | ForEach-Object { Join-Path $AssetsDirectory $_.name }) + @($manifestPath)
& gh release upload $tag @files
if ($LASTEXITCODE -ne 0) { throw 'Could not upload all draft assets; release remains a draft' }

$release = & gh api "repos/$env:GITHUB_REPOSITORY/releases/tags/$tag" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or -not $release.draft) { throw 'Draft release verification failed' }
$expected = @($manifest.artifacts | ForEach-Object { [pscustomobject]@{ name = $_.name; size = [long]$_.sizeBytes } }) +
    @([pscustomobject]@{ name = 'release-assets.json'; size = (Get-Item -LiteralPath $manifestPath).Length })
if (@($release.assets).Count -ne $expected.Count) { throw 'Draft asset count differs from the validated build' }
foreach ($asset in $expected) {
    $found = @($release.assets | Where-Object { $_.name -ceq $asset.name })
    if ($found.Count -ne 1 -or [long]$found[0].size -ne $asset.size) {
        throw "Draft asset missing or size mismatch: $($asset.name)"
    }
}

& gh release edit $tag --draft=false
if ($LASTEXITCODE -ne 0) { throw 'Draft passed verification but could not be published' }
$release = & gh api "repos/$env:GITHUB_REPOSITORY/releases/tags/$tag" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $release.draft) { throw 'Published release status could not be verified' }

$publicDirectory = Join-Path $env:RUNNER_TEMP 'aurora-public-redownload'
if ((Test-Path -LiteralPath $publicDirectory) -and @(Get-ChildItem -LiteralPath $publicDirectory -Force).Count -ne 0) {
    throw 'Public redownload directory is not empty'
}
New-Item -ItemType Directory -Path $publicDirectory -Force | Out-Null
foreach ($asset in $manifest.artifacts) {
    $publicAsset = @($release.assets | Where-Object { $_.name -ceq $asset.name })
    if ($publicAsset.Count -ne 1) { throw "Published asset is missing: $($asset.name)" }
    $destination = Join-Path $publicDirectory $asset.name
    $downloaded = $false
    for ($attempt = 1; $attempt -le 5 -and -not $downloaded; $attempt++) {
        try {
            Invoke-WebRequest -Uri $publicAsset[0].browser_download_url -OutFile $destination -MaximumRedirection 10
            $downloaded = $true
        } catch {
            if ($attempt -eq 5) { throw "Public redownload failed for $($asset.name)" }
            Start-Sleep -Seconds 5
        }
    }
    $item = Get-Item -LiteralPath $destination
    $hash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($item.Length -ne $asset.sizeBytes -or $hash -cne $asset.sha256) {
        throw "Public asset byte verification failed: $($asset.name)"
    }
}

$summary = @(
    "## Aurora Launcher $Version",
    '',
    "Source commit: ``$SourceSha``",
    '',
    '| File | Format | Architecture | Bytes | SHA-256 |',
    '| --- | --- | --- | ---: | --- |'
)
foreach ($asset in $manifest.artifacts) {
    $summary += "| $($asset.name) | $($asset.format) | $($asset.architecture) | $($asset.sizeBytes) | ``$($asset.sha256)`` |"
}
$summary += ''
$summary += 'Draft assets and publicly downloaded installer bytes matched the validated build.'
$summary | Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Encoding utf8

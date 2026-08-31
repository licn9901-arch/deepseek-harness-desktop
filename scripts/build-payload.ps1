param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
. (Join-Path $PSScriptRoot 'release-source.ps1')
Assert-DshReleaseWorktreeClean -RepoRoot $repoRoot
$sourceCommit = Get-DshReleaseSourceCommit -RepoRoot $repoRoot
$package = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw | ConvertFrom-Json
$runtimeLock = Get-Content -LiteralPath (Join-Path $repoRoot 'runtime.lock.json') -Raw | ConvertFrom-Json
$reportPath = Join-Path $repoRoot ".release-work\$($package.version)\reports\payload-build-report.json"
$payloadConfig = 'src-tauri/tauri.payload.conf.json'
$timings = [ordered]@{}
$totalWatch = [System.Diagnostics.Stopwatch]::StartNew()

# 执行一个外部构建阶段，保留原始输出并在失败时立即阻断后续打包。
function Invoke-BuildPhase {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Command,
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host "PAYLOAD PHASE START: $Name"
    & $Command @Arguments
    $exitCode = $LASTEXITCODE
    $watch.Stop()
    $timings[$Name] = $watch.ElapsedMilliseconds
    Write-Host "PAYLOAD PHASE END: $Name elapsedMs=$($watch.ElapsedMilliseconds) exitCode=$exitCode"
    if ($exitCode -ne 0) {
        throw "Payload build phase failed: $Name (exit code $exitCode)"
    }
}

Push-Location $repoRoot
try {
    Invoke-BuildPhase -Name 'pluginManagerUi' -Command 'npm.cmd' -Arguments @('run', 'build:plugin-manager')
    Invoke-BuildPhase -Name 'validateIcons' -Command 'npm.cmd' -Arguments @('run', 'validate:icons')
    Invoke-BuildPhase -Name 'runtimeDependencyStage' -Command 'npm.cmd' -Arguments @('run', 'stage:runtime')
    Invoke-BuildPhase -Name 'runtimeVerify' -Command 'npm.cmd' -Arguments @('run', 'verify:runtime')
    Invoke-BuildPhase -Name 'pluginDependencyStage' -Command 'npm.cmd' -Arguments @('run', 'stage:plugins')
    Invoke-BuildPhase -Name 'pluginVerify' -Command 'npm.cmd' -Arguments @('run', 'verify:plugins')
    Invoke-BuildPhase -Name 'payloadTrimBundleZip' -Command 'npm.cmd' -Arguments @('run', 'package:payload')
    Invoke-BuildPhase -Name 'payloadVerify' -Command 'npm.cmd' -Arguments @('run', 'verify:payload')
    Invoke-BuildPhase -Name 'rustTauriCompile' -Command 'npx.cmd' -Arguments @(
        'tauri', 'build', '--no-bundle', '--config', $payloadConfig
    )
    Invoke-BuildPhase -Name 'nsisBundle' -Command 'npx.cmd' -Arguments @(
        'tauri', 'bundle', '--bundles', 'nsis', '--config', $payloadConfig
    )

    $totalWatch.Stop()
    $installerPattern = "*_$($package.version)_x64-setup.exe"
    $installers = @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'src-tauri\target\release\bundle\nsis') `
        -File -Filter $installerPattern)
    if ($installers.Count -ne 1) {
        throw "Expected exactly one payload installer matching $installerPattern, found $($installers.Count)."
    }
    $installer = $installers[0]
    if ($installer.Length -gt 100MB) { throw "Payload installer exceeds 100 MiB: $($installer.Length) bytes." }
    $manifest = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\resources\payload\payload-manifest.json') -Raw | ConvertFrom-Json
    $report = [ordered]@{
        schemaVersion = 3
        generatedAtUtc = [DateTime]::UtcNow.ToString('O')
        desktopVersion = $package.version
        sourceCommit = $sourceCommit
        nodeVersion = $runtimeLock.node.version
        pnpmVersion = $runtimeLock.pnpm.version
        marketVersion = $runtimeLock.market.version
        payloadDigest = $manifest.payloadDigest
        payloadResourceFiles = 4
        totalMs = $totalWatch.ElapsedMilliseconds
        timingsMs = $timings
        installer = if ($null -eq $installer) { $null } else { [ordered]@{
            path = $installer.FullName.Substring($repoRoot.Length).TrimStart('\')
            bytes = $installer.Length
            sha256 = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        } }
    }
    New-Item -ItemType Directory -Force -Path (Split-Path $reportPath -Parent) | Out-Null
    $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $reportPath -Encoding utf8NoBOM
    Write-Host "PAYLOAD BUILD OK: totalMs=$($totalWatch.ElapsedMilliseconds), report=$reportPath"
}
finally {
    Pop-Location
}

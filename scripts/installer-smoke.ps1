param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,
    [string]$InstallRoot,
    [int]$TimeoutSeconds = 180,
    [switch]$SkipMarket,
    [switch]$Payload
)

$ErrorActionPreference = 'Stop'

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
. (Join-Path $PSScriptRoot 'release-installer-isolation.ps1')
$installerDshRoot = New-DshInstallerTestRoot
$installerPath = if ([System.IO.Path]::IsPathRooted($Installer)) {
    [System.IO.Path]::GetFullPath($Installer)
}
else {
    [System.IO.Path]::GetFullPath((Join-Path $projectRoot $Installer))
}
$installRoot = if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    Join-Path $installerDshRoot 'install'
}
else {
    [System.IO.Path]::GetFullPath($InstallRoot)
}
$installedExe = Join-Path $installRoot 'dsh-desktop.exe'
$uninstaller = Join-Path $installRoot 'uninstall.exe'
$bundledNode = Join-Path $installRoot 'node\node.exe'
$bundledCli = Join-Path $installRoot 'host\node_modules\@deepseek-ai\dsh\lib\bin.js'
$bundledMarket = Join-Path $installRoot 'host\node_modules\dshmarket\package.json'
$bundledPnpm = Join-Path $installRoot 'host\node_modules\.bin\pnpm.cmd'
$bundledPnpmToolchains = @(
    (Join-Path $installRoot 'host\toolchains\pnpm-10\pnpm.cmd')
)
Assert-DshInstallerTestRoots -OwnedInstallRoots @($installRoot)
$bundledMarketPolicy = Join-Path $installRoot 'policy\dsh-market.patch.yml'
$bundledRuntimeLicenses = @(
    (Join-Path $installRoot 'node\LICENSE'),
    (Join-Path $installRoot 'host\node_modules\dshmarket\LICENSE'),
    (Join-Path $installRoot 'host\node_modules\pnpm\LICENSE'),
    (Join-Path $installRoot 'host\THIRD_PARTY_NOTICES.md')
)
$bundledPluginLock = Join-Path $installRoot 'plugins\plugins.lock.json'
$bundledPluginDigest = Join-Path $installRoot 'plugins\store.digest'
$payloadResources = @(
    (Join-Path $installRoot 'payload-manifest.json'),
    (Join-Path $installRoot 'node-runtime.zip'),
    (Join-Path $installRoot 'host-runtime.zip'),
    (Join-Path $installRoot 'builtin-plugins.zip')
)
$bundledPlugins = @(
    (Join-Path $installRoot 'plugins\node_modules\@changfenhuang\dsh-genui\lib\assets\mermaid.js'),
    (Join-Path $installRoot 'plugins\node_modules\@changfenhuang\dsh-genui\lib\assets\echarts.js'),
    (Join-Path $installRoot 'plugins\node_modules\@changfenhuang\dsh-genui\SKILL.md'),
    (Join-Path $installRoot 'plugins\node_modules\dsh-better-sidebar\lib\index.js'),
    (Join-Path $installRoot 'plugins\node_modules\@dsh-desktop\settings\lib\index.js'),
    (Join-Path $installRoot 'plugins\node_modules\@dsh-desktop\settings\lib\client.js'),
    (Join-Path $installRoot 'plugins\node_modules\@linxin666\dsh-skins\cordis.patch.yml'),
    (Join-Path $installRoot 'plugins\node_modules\@linxin666\dsh-client-ui-skin-center\lib\index.js'),
    (Join-Path $installRoot 'plugins\node_modules\@vectorize-io\hindsight-coding-agents\dist\dsh.js'),
    (Join-Path $installRoot 'plugins\node_modules\@cubee-slide\skills-mcp-manager\lib\index.js'),
    (Join-Path $installRoot 'plugins\node_modules\@cubee-slide\skills-mcp-manager\lib\client.js')
)
$webIndexCandidates = @(
    (Join-Path $installRoot 'host\node_modules\@deepseek-ai\dsh-web-frontend\dist\index.html'),
    (Join-Path $installRoot 'host\node_modules\@deepseek-ai\dsh\node_modules\@deepseek-ai\dsh-web-frontend\dist\index.html')
)

# 仅管理当前隔离安装目录中的桌面进程，避免影响本机其他 DSH 实例。
function Get-InstalledDesktopProcesses {
    if (-not (Test-Path -LiteralPath $installedExe -PathType Leaf)) {
        return @()
    }
    $expectedPath = [System.IO.Path]::GetFullPath($installedExe)
    return @(Get-Process dsh-desktop -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and
        [System.IO.Path]::GetFullPath($_.Path).Equals(
            $expectedPath,
            [System.StringComparison]::OrdinalIgnoreCase
        )
    })
}

# Tauri NSIS 可能在安装完成后启动应用。先走正式退出链路，超时后也只清理隔离实例及其日志记录的 Host。
function Stop-InstalledDesktopProcesses {
    $ownedProcesses = @(Get-InstalledDesktopProcesses)
    if ($ownedProcesses.Count -eq 0) {
        return
    }

    $ownedIds = @($ownedProcesses | ForEach-Object { $_.Id })
    $managedLog = Join-Path $env:LOCALAPPDATA 'dsh-desktop\dsh-desktop.log'
    $content = if (Test-Path -LiteralPath $managedLog -PathType Leaf) {
        Get-Content -LiteralPath $managedLog -Raw
    }
    else {
        ''
    }
    $desktopIds = @($ownedIds | Where-Object {
        $content -match "pid=$_ source=app .*phase=boot_start"
    })
    if ($desktopIds.Count -gt 0) {
        $quitRequest = Start-Process -FilePath $installedExe -ArgumentList '--quit-existing' -PassThru
        if (-not $quitRequest.WaitForExit(20000)) {
            Stop-Process -Id $quitRequest.Id -Force -ErrorAction SilentlyContinue
        }
    }

    $deadline = (Get-Date).AddSeconds(20)
    do {
        $remaining = @(Get-InstalledDesktopProcesses | Where-Object { $ownedIds -contains $_.Id })
        if ($remaining.Count -eq 0) {
            break
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)

    foreach ($process in $remaining) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }

    if ($content.Length -gt 0) {
        foreach ($ownedId in $ownedIds) {
            $hostMatches = [regex]::Matches(
                $content,
                "pid=$ownedId source=app host started: pid=(\d+)"
            )
            foreach ($hostMatch in $hostMatches) {
                Stop-Process -Id ([int]$hostMatch.Groups[1].Value) -Force -ErrorAction SilentlyContinue
            }
        }
    }
}

if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
    throw "Installer not found: $installerPath"
}
if ((Test-Path -LiteralPath $installedExe -PathType Leaf) -or
    (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
    throw "Refusing to overwrite an existing installation: $installRoot"
}
Assert-DshInstallerTestUserIsClean

$installed = $false
$preseedVerified = $false
$payloadActivationVerified = $false
$previousDshHome = $env:DSH_HOME
$previousLocalAppData = $env:LOCALAPPDATA
$payloadLocalAppData = Join-Path $installerDshRoot 'localappdata'
$payloadRuntimeRoot = Join-Path $payloadLocalAppData 'dsh-desktop\runtime'
$profileSentinel = Join-Path $installerDshRoot '.dsh\installer-smoke-sentinel.txt'
try {
    $env:DSH_HOME = Join-Path $installerDshRoot '.dsh'
    if ($Payload) {
        $env:LOCALAPPDATA = $payloadLocalAppData
    }
    # 自动化使用 NSIS 静默安装；应用生命周期仍由完整桌面冒烟脚本验证。
    # `/D=` 必须是 NSIS 的最后一个参数；隔离目录可避免 smoke 触碰用户已有安装。
    $installArguments = @('/S', '/NS')
    if ($Payload) {
        # 不能使用 `/R*` 参数名；Tauri 默认 NSIS 会把它误识别成安装后启动开关 `/R`。
        $installArguments += "/PAYLOADTESTROOT=$payloadRuntimeRoot"
    }
    else {
        $installArguments += "/DSHHOME=$env:DSH_HOME"
    }
    $installArguments += "/D=$installRoot"
    $installTimeoutSeconds = if ($Payload) { $TimeoutSeconds } else { [Math]::Max($TimeoutSeconds, 900) }
    $installProcess = Start-Process -FilePath $installerPath -ArgumentList $installArguments -PassThru
    if (-not $installProcess.WaitForExit($installTimeoutSeconds * 1000)) {
        Stop-Process -Id $installProcess.Id -Force -ErrorAction SilentlyContinue
        throw "Silent installation did not exit within $installTimeoutSeconds seconds."
    }
    $installed = Test-Path -LiteralPath $installRoot -PathType Container
    if ($installProcess.ExitCode -ne 0) {
        throw "Silent installation failed with exit code $($installProcess.ExitCode)."
    }

    # NSIS 可能把实际复制交给后台子进程；所有关键资源落盘后才能启动应用。
    $installDeadline = (Get-Date).AddSeconds($installTimeoutSeconds)
    do {
        $installed = Test-Path -LiteralPath $installRoot -PathType Container
        $webReady = $webIndexCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
        $commonReady = (Test-Path -LiteralPath $installedExe -PathType Leaf) -and
            (Test-Path -LiteralPath $uninstaller -PathType Leaf)
        if ($Payload) {
            $statePath = Join-Path $payloadRuntimeRoot 'runtime-state.json'
            $stateReady = Test-Path -LiteralPath $statePath -PathType Leaf
            $installReady = $commonReady -and
                (($payloadResources | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }).Count -eq $payloadResources.Count) -and
                $stateReady
            if (Test-Path -LiteralPath (Join-Path $installRoot 'payload-tool.exe')) {
                throw 'Internal payload-tool must not be installed with the desktop application.'
            }
        }
        else {
            $installReady = $commonReady -and
                (Test-Path -LiteralPath $bundledNode -PathType Leaf) -and
                (Test-Path -LiteralPath $bundledCli -PathType Leaf) -and
                (Test-Path -LiteralPath $bundledMarket -PathType Leaf) -and
                (Test-Path -LiteralPath $bundledPnpm -PathType Leaf) -and
                (($bundledPnpmToolchains | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }).Count -eq $bundledPnpmToolchains.Count) -and
                (Test-Path -LiteralPath $bundledMarketPolicy -PathType Leaf) -and
                (($bundledRuntimeLicenses | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }).Count -eq $bundledRuntimeLicenses.Count) -and
                (Test-Path -LiteralPath $bundledPluginLock -PathType Leaf) -and
                (Test-Path -LiteralPath $bundledPluginDigest -PathType Leaf) -and
                (($bundledPlugins | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }).Count -eq $bundledPlugins.Count) -and
                $webReady
        }
        if (-not $installReady) {
            Start-Sleep -Milliseconds 250
        }
    } while (-not $installReady -and (Get-Date) -lt $installDeadline)
    if (-not $installReady) {
        throw "Installation did not finish writing the bundled runtime within $installTimeoutSeconds seconds."
    }

    if ($Payload) {
        # Tauri current-user 安装器可能让外层进程先退出；provision helper 必须完成后才检查 candidate。
        $provisionDeadline = (Get-Date).AddSeconds($TimeoutSeconds)
        do {
            $provisionProcesses = @(Get-InstalledDesktopProcesses)
            if ($provisionProcesses.Count -eq 0) {
                break
            }
            Start-Sleep -Milliseconds 250
        } while ((Get-Date) -lt $provisionDeadline)
        if ($provisionProcesses.Count -gt 0) {
            throw "Payload provision helper did not exit within $TimeoutSeconds seconds."
        }

        $payloadState = Get-Content -LiteralPath (Join-Path $payloadRuntimeRoot 'runtime-state.json') -Raw | ConvertFrom-Json
        if ($null -eq $payloadState.candidate -or $null -ne $payloadState.active) {
            throw 'Payload installer did not register exactly one candidate on clean install.'
        }
        $candidateRoot = Join-Path $payloadRuntimeRoot $payloadState.candidate.payloadDigest
        foreach ($entry in @('node\node.exe', 'host\node_modules\@deepseek-ai\dsh\lib\bin.js', 'plugins\node_modules')) {
            if (-not (Test-Path -LiteralPath (Join-Path $candidateRoot $entry))) {
                throw "Provisioned candidate entry is missing: $entry"
            }
        }
    }
    else {
        $digest = (Get-Content -LiteralPath $bundledPluginDigest -Raw).Trim()
        $preseededStore = Join-Path $env:DSH_HOME "profiles\node_modules\.dsh-desktop\$digest"
        if (-not (Test-Path -LiteralPath (Join-Path $preseededStore 'plugins.lock.json') -PathType Leaf) -or
            -not (Test-Path -LiteralPath (Join-Path $preseededStore 'node_modules\@dsh-desktop\runtime-services\lib\index.js') -PathType Leaf)) {
            throw "Installer did not preseed the managed plugin store: $preseededStore"
        }
        $preseedVerified = $true
    }

    $smokeArguments = @{
        Exe = $installedExe
        TimeoutSeconds = $TimeoutSeconds
        UseBundledRuntime = $true
        UseInstalledWebViewDataDirectory = $true
        DshHome = $env:DSH_HOME
    }
    if (-not $SkipMarket) { $smokeArguments.TestMarket = $true }
    & (Join-Path $PSScriptRoot 'smoke-test.ps1') @smokeArguments
    if ($Payload) {
        $payloadState = Get-Content -LiteralPath (Join-Path $payloadRuntimeRoot 'runtime-state.json') -Raw | ConvertFrom-Json
        if ($null -eq $payloadState.active -or $null -ne $payloadState.candidate) {
            throw 'Candidate was not promoted to active after real Host and plugin readiness.'
        }
        New-Item -ItemType Directory -Force -Path (Split-Path $profileSentinel -Parent) | Out-Null
        Set-Content -LiteralPath $profileSentinel -Value 'must survive uninstall' -Encoding utf8NoBOM
        $payloadActivationVerified = $true
    }
}
finally {
    if ($installed) {
        Stop-InstalledDesktopProcesses
    }
    if ($installed) {
        $uninstallerDeadline = (Get-Date).AddSeconds(60)
        while (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf) -and (Get-Date) -lt $uninstallerDeadline) {
            Start-Sleep -Milliseconds 250
        }
    }
    if ($installed -and (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        $completionPaths = @($installedExe)
        if ($Payload) { $completionPaths += $payloadRuntimeRoot }
        Invoke-DshSilentUninstall `
            -Uninstaller $uninstaller `
            -CompletionPaths $completionPaths
    }
    if ($preseedVerified -and -not (Test-Path -LiteralPath $preseededStore -PathType Container)) {
        throw 'Uninstaller removed the managed plugin cache that must be preserved.'
    }
    if ($payloadActivationVerified -and -not (Test-Path -LiteralPath $profileSentinel -PathType Leaf)) {
        throw 'Uninstaller removed the user DSH profile sentinel.'
    }
    $shellCleanupError = $null
    try {
        Clear-DshInstallerTestUserState -OwnedInstallRoots @($installRoot)
    } catch {
        $shellCleanupError = $_.Exception
    }
    [Environment]::SetEnvironmentVariable('DSH_HOME', $previousDshHome, 'Process')
    [Environment]::SetEnvironmentVariable('LOCALAPPDATA', $previousLocalAppData, 'Process')
    if (Test-Path -LiteralPath $installerDshRoot -PathType Container) {
        Remove-DshInstallerTestDirectory -Root $installerDshRoot
    }
    if ($null -ne $shellCleanupError) { throw $shellCleanupError }
}

if (Test-Path -LiteralPath $installedExe) {
    throw "Installed executable remained after uninstall: $installedExe"
}

Write-Host 'INSTALLER SMOKE OK: install, bundled runtime, lifecycle and uninstall verified.'

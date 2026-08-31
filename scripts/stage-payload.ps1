param(
    [switch]$Force,
    [switch]$SkipBudgetGate,
    [switch]$SkipDebugArchive
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cacheRoot = Join-Path $repoRoot '.runtime-cache\payload'
$stagingRoot = Join-Path $repoRoot '.runtime-cache\payload-staging'
$outputRoot = Join-Path $repoRoot 'src-tauri\resources\payload'
$debugRoot = Join-Path $repoRoot '.deploy-artifacts\runtime-debug-symbols'
$rawRoot = Join-Path $repoRoot 'src-tauri\resources'
$cargoManifest = Join-Path $repoRoot 'src-tauri\Cargo.toml'
$runtimeLock = Get-Content -LiteralPath (Join-Path $repoRoot 'runtime.lock.json') -Raw | ConvertFrom-Json
if ($runtimeLock.schemaVersion -ne 2) {
    throw "Unsupported runtime lock schema: $($runtimeLock.schemaVersion)"
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)][string]$Parent,
        [Parameter(Mandatory = $true)][string]$Child
    )
    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $childFull = [System.IO.Path]::GetFullPath($Child)
    if (-not $childFull.StartsWith($parentFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to mutate path outside $Parent`: $Child"
    }
}

function Invoke-RobocopyTree {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    & robocopy.exe $Source $Destination /MIR /COPY:DAT /DCOPY:DAT /R:1 /W:1 /MT:32 /XJ /SL /NFL /NDL /NJH /NJS /NP
    if ($LASTEXITCODE -gt 7) {
        throw "robocopy failed for $Source with exit code $LASTEXITCODE"
    }
}

function Get-TreeMeasure {
    param([Parameter(Mandatory = $true)][string]$Path)
    $files = @(Get-ChildItem -LiteralPath $Path -File -Recurse -Force -ErrorAction SilentlyContinue)
    $bytes = ($files | Measure-Object Length -Sum).Sum
    if ($null -eq $bytes) { $bytes = 0 }
    [pscustomobject]@{ files = $files.Count; bytes = [int64]$bytes }
}

function Copy-DebugFiles {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    & robocopy.exe $Source $Destination *.pdb *.map /S /COPY:DAT /DCOPY:DAT /R:1 /W:1 /MT:32 /XJ /SL /NFL /NDL /NJH /NJS /NP
    if ($LASTEXITCODE -gt 7) {
        throw "robocopy failed while collecting debug files from $Source with exit code $LASTEXITCODE"
    }
}

function Remove-DevelopmentFiles {
    param([Parameter(Mandatory = $true)][string]$Root)
    $developmentExtensions = @('.pdb', '.map', '.ts', '.tsx', '.d.ts', '.d.cts', '.d.mts')
    Get-ChildItem -LiteralPath $Root -File -Recurse -Force | Where-Object {
        $name = $_.Name.ToLowerInvariant()
        $developmentExtensions | Where-Object { $name.EndsWith($_) }
    } | Remove-Item -Force

    # `doc/docs` 可能是运行时代码命名（例如 yaml/dist/doc），不能只按目录名删除。
    $developmentDirectories = @('test', 'tests', '__tests__', 'example', 'examples', 'benchmark', 'benchmarks')
    Get-ChildItem -LiteralPath $Root -Directory -Recurse -Force | Where-Object {
        $_.Name.ToLowerInvariant() -in $developmentDirectories
    } | Sort-Object FullName -Descending | Remove-Item -Recurse -Force

    Get-ChildItem -LiteralPath $Root -File -Recurse -Force -Filter '*.md' | Where-Object {
        $_.Name -notmatch '^(LICENSE|NOTICE|COPYING)' -and $_.Name -ne 'SKILL.md'
    } | Remove-Item -Force
}

function Reduce-NodePty {
    param([Parameter(Mandatory = $true)][string]$NodeModules)
    $root = Join-Path $NodeModules 'node-pty'
    if (-not (Test-Path -LiteralPath $root)) { return }
    foreach ($directory in @(Get-ChildItem -LiteralPath $root -Directory -Force)) {
        if ($directory.Name -notin 'lib', 'prebuilds') {
            Remove-Item -LiteralPath $directory.FullName -Recurse -Force
        }
    }
    $prebuilds = Join-Path $root 'prebuilds'
    foreach ($directory in @(Get-ChildItem -LiteralPath $prebuilds -Directory -Force -ErrorAction SilentlyContinue)) {
        if ($directory.Name -ne 'win32-x64') {
            Remove-Item -LiteralPath $directory.FullName -Recurse -Force
        }
    }
    Get-ChildItem -LiteralPath (Join-Path $root 'lib') -File -Recurse -Force -Filter '*.test.js' -ErrorAction SilentlyContinue | Remove-Item -Force
    Get-ChildItem -LiteralPath $root -File -Force | Where-Object { $_.Name -notin 'package.json', 'LICENSE' } | Remove-Item -Force
}

function Remove-PluginClientDuplicates {
    param([Parameter(Mandatory = $true)][string]$NodeModules)
    foreach ($relative in @('lucide-react', 'rxjs', 'xterm', '@codemirror', '@lezer', '@xterm')) {
        $target = Join-Path $NodeModules $relative
        if (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }
}

function Remove-NodeIgnoredModuleFormats {
    param([Parameter(Mandatory = $true)][string]$NodeModules)

    # Node 22 按 package main 加载这些 CJS 入口；module/esnext 仅供前端 bundler 使用。
    foreach ($scope in @('@opentelemetry', '@smithy', '@aws-sdk')) {
        $scopeRoot = Join-Path $NodeModules $scope
        foreach ($packageRoot in @(Get-ChildItem -LiteralPath $scopeRoot -Directory -Force -ErrorAction SilentlyContinue)) {
            $packageJson = Join-Path $packageRoot.FullName 'package.json'
            if (-not (Test-Path -LiteralPath $packageJson -PathType Leaf)) { continue }
            $package = Get-Content -LiteralPath $packageJson -Raw | ConvertFrom-Json
            if ($package.main -match '(^|/)(build/src|dist-cjs)(/|$)') {
                foreach ($relative in @('build\esm', 'build\esnext', 'dist-es')) {
                    $target = Join-Path $packageRoot.FullName $relative
                    if (Test-Path -LiteralPath $target -PathType Container) {
                        Remove-Item -LiteralPath $target -Recurse -Force
                    }
                }
            }
        }
    }
}

function Copy-PayloadResources {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    foreach ($name in @('payload-manifest.json', 'node-runtime.zip', 'host-runtime.zip', 'builtin-plugins.zip')) {
        Copy-Item -LiteralPath (Join-Path $Source $name) -Destination (Join-Path $Destination $name) -Force
    }
}

# 为同一 cache key 获取跨进程独占锁；进程退出时 FileStream 句柄会由系统自动释放。
function Enter-PayloadCacheLock {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [int]$TimeoutSeconds = 1800
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ($true) {
        try {
            return [System.IO.File]::Open(
                $Path,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        }
        catch [System.IO.IOException] {
            if ([DateTime]::UtcNow -ge $deadline) {
                throw "Timed out waiting for payload cache lock: $Path"
            }
            Start-Sleep -Milliseconds 250
        }
    }
}

foreach ($required in @(
    (Join-Path $rawRoot 'node\node.exe'),
    (Join-Path $rawRoot 'host\node_modules\@deepseek-ai\dsh\lib\bin.js'),
    (Join-Path $rawRoot 'plugins\node_modules'),
    (Join-Path $repoRoot 'plugins.lock.json')
)) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "Payload input is missing: $required. Run stage:runtime and stage:plugins first."
    }
}

$keyInputs = @(
    'runtime.lock.json',
    'plugins.lock.json',
    'runtime-host\package-lock.json',
    'plugin-runtime\package-lock.json',
    'package-lock.json',
    'src-tauri\Cargo.toml',
    'src-tauri\Cargo.lock',
    'src-tauri\src\payload.rs',
    'src-tauri\examples\payload-tool.rs',
    'rust-toolchain.toml',
    'scripts\stage-runtime.ps1',
    'scripts\patch-directory-picker.ps1',
    'scripts\stage-plugins.ps1',
    'scripts\optimize-plugin-previews.mjs',
    'scripts\prune-plugin-client-dependencies.mjs',
    'scripts\stage-payload.ps1'
)
$keyLines = @(
    'platform=win32-x64',
    "node=$(& node.exe --version)",
    "npm=$(& npm.cmd --version)",
    "esbuild=$(& node.exe -p "require('./node_modules/esbuild/package.json').version")",
    "rust=$(& rustc.exe --version)"
)
foreach ($relative in $keyInputs) {
    $path = Join-Path $repoRoot $relative
    if (-not (Test-Path -LiteralPath $path)) { throw "Cache input is missing: $path" }
    $keyLines += "$relative=$((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant())"
}
$keyText = ($keyLines -join "`n") + "`n"
$keyBytes = [System.Text.Encoding]::UTF8.GetBytes($keyText)
$hasher = [System.Security.Cryptography.SHA256]::Create()
$cacheKey = ([System.BitConverter]::ToString($hasher.ComputeHash($keyBytes))).Replace('-', '').ToLowerInvariant()
$cacheDirectory = Join-Path $cacheRoot $cacheKey
$cachedResources = Join-Path $cacheDirectory 'resources'

New-Item -ItemType Directory -Force -Path $cacheRoot, $stagingRoot, $debugRoot | Out-Null
Assert-ChildPath -Parent (Join-Path $repoRoot '.runtime-cache') -Child $cacheDirectory
Assert-ChildPath -Parent (Join-Path $repoRoot '.runtime-cache') -Child $stagingRoot
$cacheLockPath = Join-Path $cacheRoot "$cacheKey.lock"
$cacheLock = Enter-PayloadCacheLock -Path $cacheLockPath

if (-not $Force -and (Test-Path -LiteralPath (Join-Path $cachedResources 'payload-manifest.json'))) {
    & cargo.exe run --quiet --manifest-path $cargoManifest --example payload-tool -- verify --resources $cachedResources
    if ($LASTEXITCODE -eq 0) {
        Copy-PayloadResources -Source $cachedResources -Destination $outputRoot
        Write-Host "Payload cache hit: $cacheKey"
        $cacheLock.Dispose()
        $cacheLock = $null
        exit 0
    }
}

$totalWatch = [System.Diagnostics.Stopwatch]::StartNew()
$staging = Join-Path $stagingRoot "$cacheKey.staging.$PID"
Assert-ChildPath -Parent $stagingRoot -Child $staging
if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
New-Item -ItemType Directory -Force -Path $staging | Out-Null

try {
    $copyWatch = [System.Diagnostics.Stopwatch]::StartNew()
    $nodeSource = Join-Path $staging 'node\node'
    $hostSource = Join-Path $staging 'host\host'
    $pluginSource = Join-Path $staging 'plugins\plugins'
    New-Item -ItemType Directory -Force -Path $nodeSource | Out-Null
    Copy-Item -LiteralPath (Join-Path $rawRoot 'node\node.exe') -Destination $nodeSource -Force
    foreach ($license in @(Get-ChildItem -LiteralPath (Join-Path $rawRoot 'node') -File -Filter 'LICENSE*')) {
        Copy-Item -LiteralPath $license.FullName -Destination $nodeSource -Force
    }
    Invoke-RobocopyTree -Source (Join-Path $rawRoot 'host') -Destination $hostSource
    Invoke-RobocopyTree -Source (Join-Path $rawRoot 'plugins') -Destination $pluginSource
    Invoke-RobocopyTree -Source (Join-Path $rawRoot 'policy') -Destination (Join-Path $staging 'host\policy')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'plugins.lock.json') -Destination (Join-Path $pluginSource 'plugins.lock.json') -Force
    $copyWatch.Stop()

    $before = Get-TreeMeasure -Path $staging
    $trimWatch = [System.Diagnostics.Stopwatch]::StartNew()
    $debugStaging = Join-Path $debugRoot "$cacheKey.staging.$PID"
    if (Test-Path -LiteralPath $debugStaging) { Remove-Item -LiteralPath $debugStaging -Recurse -Force }
    Copy-DebugFiles -Source $hostSource -Destination (Join-Path $debugStaging 'host')
    Copy-DebugFiles -Source $pluginSource -Destination (Join-Path $debugStaging 'plugins')
    Remove-DevelopmentFiles -Root $hostSource
    Remove-DevelopmentFiles -Root $pluginSource
    Reduce-NodePty -NodeModules (Join-Path $hostSource 'node_modules')
    Reduce-NodePty -NodeModules (Join-Path $pluginSource 'node_modules')
    $sharpWasm = Join-Path $hostSource 'node_modules\@img\sharp-wasm32'
    if (Test-Path -LiteralPath $sharpWasm) { Remove-Item -LiteralPath $sharpWasm -Recurse -Force }
    Remove-PluginClientDuplicates -NodeModules (Join-Path $pluginSource 'node_modules')
    & (Join-Path $nodeSource 'node.exe') (Join-Path $PSScriptRoot 'prune-plugin-client-dependencies.mjs') `
        --lock (Join-Path $repoRoot 'plugin-runtime\package-lock.json') `
        --node-modules (Join-Path $pluginSource 'node_modules') `
        --owner 'dsh-better-sidebar' --dependency 'mermaid'
    if ($LASTEXITCODE -ne 0) { throw 'Plugin client dependency pruning failed.' }
    # Sidebar 的浏览器 bundle 已内联图标实现，删除未被任何其他托管插件引用的完整 react-icons 包。
    & (Join-Path $nodeSource 'node.exe') (Join-Path $PSScriptRoot 'prune-plugin-client-dependencies.mjs') `
        --lock (Join-Path $repoRoot 'plugin-runtime\package-lock.json') `
        --node-modules (Join-Path $pluginSource 'node_modules') `
        --owner 'dsh-better-sidebar' --dependency 'react-icons'
    if ($LASTEXITCODE -ne 0) { throw 'Plugin client dependency pruning failed.' }
    Remove-NodeIgnoredModuleFormats -NodeModules (Join-Path $hostSource 'node_modules')
    & (Join-Path $nodeSource 'node.exe') (Join-Path $PSScriptRoot 'optimize-plugin-previews.mjs') `
        --host-node-modules (Join-Path $hostSource 'node_modules') --plugin-root $pluginSource
    if ($LASTEXITCODE -ne 0) { throw 'Skin preview optimization failed.' }
    $trimWatch.Stop()

    $bundleWatch = [System.Diagnostics.Stopwatch]::StartNew()
    $hostEntry = Join-Path $hostSource 'node_modules\@deepseek-ai\dsh\lib\bin.js'
    $bundledEntry = "$hostEntry.bundle"
    $metafile = Join-Path $staging 'host-metafile.json'
    Push-Location $hostSource
    try {
        # 使用稳定相对入口，避免 esbuild source label 把 staging PID 写进 Host bundle。
        & (Join-Path $repoRoot 'node_modules\.bin\esbuild.cmd') `
            'node_modules/@deepseek-ai/dsh/lib/bin.js' `
            --bundle --platform=node --format=esm --target=node22 --packages=external --log-level=warning `
            --metafile=$metafile --outfile=$bundledEntry
        if ($LASTEXITCODE -ne 0) { throw 'Host esbuild closure failed.' }
    }
    finally {
        Pop-Location
    }
    Move-Item -LiteralPath $bundledEntry -Destination $hostEntry -Force
    $bundleWatch.Stop()

    $loaderWatch = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $hostSource
    try {
        & (Join-Path $nodeSource 'node.exe') -e "require('yaml'); require('@opentelemetry/api'); require('@aws-sdk/client-bedrock-runtime'); require('node-pty'); require('sharp'); console.log('runtime-loader-ok')"
        if ($LASTEXITCODE -ne 0) { throw 'Native loader smoke failed.' }
    } finally {
        Pop-Location
    }
    $loaderWatch.Stop()

    $packageWatch = [System.Diagnostics.Stopwatch]::StartNew()
    $payloadOutput = Join-Path $staging 'resources'
    $desktopVersion = (Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw | ConvertFrom-Json).version
    & cargo.exe run --quiet --manifest-path $cargoManifest --example payload-tool -- package --node (Join-Path $staging 'node') --host (Join-Path $staging 'host') --plugins (Join-Path $staging 'plugins') --output $payloadOutput --desktop-version $desktopVersion --runtime-abi 1 --node-version $runtimeLock.node.version --pnpm-version $runtimeLock.pnpm.version
    if ($LASTEXITCODE -ne 0) { throw 'Payload packaging failed.' }
    $packageWatch.Stop()

    $after = Get-TreeMeasure -Path $staging
    $manifest = Get-Content -LiteralPath (Join-Path $payloadOutput 'payload-manifest.json') -Raw | ConvertFrom-Json
    $payloadFiles = [int64]$manifest.nodeRuntime.fileCount + [int64]$manifest.hostRuntime.fileCount + [int64]$manifest.builtinPlugins.fileCount
    $payloadUnpacked = [int64]$manifest.nodeRuntime.unpackedSize + [int64]$manifest.hostRuntime.unpackedSize + [int64]$manifest.builtinPlugins.unpackedSize
    $payloadCompressed = [int64]$manifest.nodeRuntime.compressedSize + [int64]$manifest.hostRuntime.compressedSize + [int64]$manifest.builtinPlugins.compressedSize
    if (-not $SkipBudgetGate) {
        # 安装器只接收 manifest 与三个 ZIP；展开后的文件数单独记录，不再生成 NSIS File 指令。
        if (4 -gt 4741) { throw 'Payload installer resource file budget exceeded.' }
        if ($payloadFiles -gt 20000) { throw "Expanded payload file safety budget exceeded: $payloadFiles > 20000" }
        if ($payloadUnpacked -gt 300MB) { throw "Payload unpacked budget exceeded: $payloadUnpacked > 300 MiB" }
        if ($payloadCompressed -gt 90MB) { throw "Payload compressed budget exceeded: $payloadCompressed > 90 MiB" }
    }

    $totalWatch.Stop()
    $report = [ordered]@{
        schemaVersion = 1
        cacheKey = $cacheKey
        payloadDigest = $manifest.payloadDigest
        beforeFiles = $before.files
        beforeBytes = $before.bytes
        afterFiles = $payloadFiles
        afterUnpackedBytes = $payloadUnpacked
        compressedBytes = $payloadCompressed
        timingsMs = [ordered]@{
            copy = $copyWatch.ElapsedMilliseconds
            trim = $trimWatch.ElapsedMilliseconds
            bundle = $bundleWatch.ElapsedMilliseconds
            loaderSmoke = $loaderWatch.ElapsedMilliseconds
            zipAndManifest = $packageWatch.ElapsedMilliseconds
            total = $totalWatch.ElapsedMilliseconds
        }
        inputs = $keyLines
    }
    $cacheStaging = "$cacheDirectory.staging.$PID"
    Assert-ChildPath -Parent $cacheRoot -Child $cacheStaging
    if (Test-Path -LiteralPath $cacheStaging) { Remove-Item -LiteralPath $cacheStaging -Recurse -Force }
    New-Item -ItemType Directory -Force -Path (Join-Path $cacheStaging 'resources') | Out-Null
    Copy-PayloadResources -Source $payloadOutput -Destination (Join-Path $cacheStaging 'resources')
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $cacheStaging 'build-report.json') -Encoding utf8NoBOM
    if (Test-Path -LiteralPath $cacheDirectory) { Remove-Item -LiteralPath $cacheDirectory -Recurse -Force }
    Move-Item -LiteralPath $cacheStaging -Destination $cacheDirectory
    Copy-PayloadResources -Source (Join-Path $cacheDirectory 'resources') -Destination $outputRoot

    if ((Test-Path -LiteralPath $debugStaging) -and -not $SkipDebugArchive) {
        $debugZip = Join-Path $debugRoot "runtime-debug-symbols-$cacheKey.zip"
        if (Test-Path -LiteralPath $debugZip) { Remove-Item -LiteralPath $debugZip -Force }
        Compress-Archive -Path (Join-Path $debugStaging '*') -DestinationPath $debugZip -CompressionLevel Optimal
        Remove-Item -LiteralPath $debugStaging -Recurse -Force
    }
    elseif (Test-Path -LiteralPath $debugStaging) {
        Remove-Item -LiteralPath $debugStaging -Recurse -Force
    }
    Write-Host "Payload staged: digest=$($manifest.payloadDigest), files=$payloadFiles, unpacked=$payloadUnpacked, compressed=$payloadCompressed, totalMs=$($totalWatch.ElapsedMilliseconds)"
} finally {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    if ($null -ne $cacheLock) { $cacheLock.Dispose() }
}

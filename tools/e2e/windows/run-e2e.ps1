<#
.SYNOPSIS
  Updates the reference application from version N to N+1 on Windows, end
  to end, for one architecture.

.DESCRIPTION
  Builds the reference app twice per flow (1.0.0 and 1.1.0) for the declared
  architecture, packages and signs version 1.1.0 with a disposable Ed25519
  key, serves the signed feed from loopback, and runs version 1.0.0
  unattended (REFERENCE_APP_E2E_REPORT) until version 1.1.0 is running.

  Flows:
    inno-setup  1.0.0 is installed per user by its Inno Setup installer into
                a non-default directory; the update runs the 1.1.0 installer
                after the app has quit, which replaces the files there and
                relaunches the app.
    portable    1.0.0 is a single executable; the update swaps in the 1.1.0
                executable and restarts into it.

  The installer appends its own lines to the report, so the order in which
  the old version, the installer, and the new version ran is checked from
  one file. The installer also records whether the old executable was still
  in use when it was about to replace files; it must not be, and the old
  version must have exited cleanly by itself rather than been closed by
  Setup.

  Requires PowerShell 7, cargo, Python 3 (for the loopback server), and for
  the inno-setup flow Inno Setup 6.3 or newer (ISCC.exe on PATH, in a
  standard location, or given with -Iscc). Nothing leaves the machine.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('x86_64', 'aarch64')]
    [string] $Arch,

    [ValidateSet('inno-setup', 'portable')]
    [string[]] $Flow = @('inno-setup', 'portable'),

    [string] $WorkDir = (Join-Path ([IO.Path]::GetTempPath()) 'gpui-auto-update-e2e'),

    [int] $Port = 18765,

    [int] $TimeoutSeconds = 240,

    [string] $Iscc
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$OldVersion = '1.0.0'
$NewVersion = '1.1.0'
$AppName = 'GPUI Auto Update Reference App'

# Everything that differs between architectures is declared here, never
# derived from file names.
$Packaging = @{
    x86_64  = @{ Triple = 'x86_64-pc-windows-msvc'; InnoArch = 'x64compatible'; Host = 'AMD64' }
    aarch64 = @{ Triple = 'aarch64-pc-windows-msvc'; InnoArch = 'arm64'; Host = 'ARM64' }
}[$Arch]

function Invoke-Native {
    param([Parameter(Mandatory)] [string] $Program, [string[]] $Arguments = @())
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Program $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Find-Iscc {
    if ($Iscc) { return $Iscc }
    $command = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }
    throw 'ISCC.exe (Inno Setup 6.3 or newer) was not found; pass -Iscc <path>.'
}

function Build-ReferenceApp {
    param(
        [string] $Version,
        [string] $Install,
        [string] $AppId,
        [string] $FeedUrl,
        [string] $PublicKey,
        [string] $Report,
        [string] $Destination
    )
    $variables = @{
        REFERENCE_APP_ID                      = $AppId
        REFERENCE_APP_VERSION                 = $Version
        REFERENCE_APP_FEED_URL                = $FeedUrl
        REFERENCE_APP_PUBLIC_KEY              = $PublicKey
        REFERENCE_APP_ALLOW_INSECURE_HTTP     = '1'
        REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE = '1'
        REFERENCE_APP_WINDOWS_INSTALL         = $Install
        REFERENCE_APP_E2E_REPORT              = $Report
    }
    foreach ($name in $variables.Keys) { Set-Item "env:$name" $variables[$name] }
    try {
        Invoke-Native cargo @('build', '--locked', '-p', 'gpui-auto-update-reference-app', '--target', $Packaging.Triple)
    } finally {
        foreach ($name in $variables.Keys) { Remove-Item "env:$name" -ErrorAction SilentlyContinue }
    }
    $built = Join-Path $Repo "target\$($Packaging.Triple)\debug\reference-app.exe"
    New-Item -ItemType Directory -Force (Split-Path $Destination) | Out-Null
    Copy-Item $built $Destination -Force
    $reported = (& $Destination --version | Out-String).Trim()
    if ($reported -ne $Version) {
        throw "$Destination reports version '$reported', expected $Version"
    }
}

function Publish-Feed {
    param([string] $Artifact, [string] $Flow, [string] $Site, [string] $KeyFile, [string] $PublicKey)
    $feed = Join-Path $Site "$Flow\appcast-windows-$Arch.xml"
    New-Item -ItemType Directory -Force (Split-Path $feed) | Out-Null
    Invoke-Native $script:Cli @(
        'feed', 'native',
        '--os', 'windows', '--arch', $Arch, '--version', $NewVersion,
        '--artifact', $Artifact,
        '--download-url-prefix', "http://127.0.0.1:$Port/$Flow/$NewVersion/",
        '--allow-http',
        '--public-key', $PublicKey, '--key-file', $KeyFile,
        '--output', $feed
    )
    $published = Join-Path $Site "$Flow\$NewVersion"
    New-Item -ItemType Directory -Force $published | Out-Null
    Copy-Item $Artifact $published
}

function Start-FeedServer {
    param([string] $Site)
    $python = (Get-Command 'python' -ErrorAction SilentlyContinue) ?? (Get-Command 'python3')
    $server = Start-Process $python.Source -PassThru -WindowStyle Hidden -ArgumentList @(
        '-m', 'http.server', $Port, '--bind', '127.0.0.1', '--directory', "`"$Site`""
    )
    $deadline = (Get-Date).AddSeconds(30)
    while ($true) {
        try {
            Invoke-WebRequest "http://127.0.0.1:$Port/" -UseBasicParsing -TimeoutSec 2 | Out-Null
            return $server
        } catch {
            if ((Get-Date) -gt $deadline) { throw "the feed server did not start: $_" }
            Start-Sleep -Milliseconds 250
        }
    }
}

function Read-Report {
    param([string] $Report)
    if (Test-Path $Report) { return @(Get-Content $Report) }
    return @()
}

# Waits until the report contains lines starting with each of $Expected, in
# that order. Fails at once on an `error` line.
function Wait-Report {
    param([string] $Report, [string[]] $Expected, [string[]] $Diagnostics = @())
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ($true) {
        $lines = Read-Report $Report
        $next = 0
        foreach ($line in $lines) {
            if ($line -like 'error *') { Fail-Report $Report "the app reported: $line" $Diagnostics }
            if ($next -lt $Expected.Count -and $line.StartsWith($Expected[$next])) { $next++ }
        }
        if ($next -eq $Expected.Count) { return $lines }
        if ((Get-Date) -gt $deadline) {
            Fail-Report $Report "timed out waiting for '$($Expected[$next])'" $Diagnostics
        }
        Start-Sleep -Milliseconds 250
    }
}

function Fail-Report {
    param([string] $Report, [string] $Message, [string[]] $Diagnostics)
    Write-Host "---- $Report"
    Read-Report $Report | ForEach-Object { Write-Host $_ }
    foreach ($file in $Diagnostics) {
        if (Test-Path $file) {
            Write-Host "---- $file"
            Get-Content $file | Select-Object -Last 80 | ForEach-Object { Write-Host $_ }
        }
    }
    throw $Message
}

function Stop-ReferenceApp {
    param([string] $Executable)
    Get-Process 'reference-app' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $Executable } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

function Assert-Version {
    param([string] $Executable, [string] $Expected)
    $reported = (& $Executable --version | Out-String).Trim()
    if ($reported -ne $Expected) {
        throw "$Executable is version '$reported' after the update, expected $Expected"
    }
}

function Test-InnoSetupFlow {
    param([string] $KeyFile, [string] $PublicKey, [string] $Site)
    $iscc = Find-Iscc
    $root = Join-Path $WorkDir 'inno-setup'
    $report = Join-Path $root 'report.log'
    $innoLog = Join-Path $root 'inno-setup.log'
    # A non-default directory, with a space, that the update must keep.
    $installDir = Join-Path $root 'Custom Install Dir'
    $feedUrl = "http://127.0.0.1:$Port/inno-setup/appcast-windows-$Arch.xml"
    $appId = 'dev.gpui-auto-update.reference-app.e2e-inno-setup'

    $installers = @{}
    foreach ($version in @($OldVersion, $NewVersion)) {
        $exe = Join-Path $root "build\$version\reference-app.exe"
        Build-ReferenceApp $version 'inno-setup' $appId $feedUrl $PublicKey $report $exe
        $parts = @($version.Split('-')[0].Split('.')) + @('0')
        Invoke-Native $iscc @(
            '/Q',
            "/DAppVersion=$version",
            "/DNumericVersion=$($parts[0..3] -join '.')",
            "/DFeedArch=$Arch",
            "/DInnoArch=$($Packaging.InnoArch)",
            "/DSourceExe=$exe",
            "/DOutputDir=$(Join-Path $root 'dist')",
            "/DE2EReport=$report",
            (Join-Path $Repo 'apps\reference-app\packaging\windows\reference-app.iss')
        )
        $installers[$version] = Join-Path $root "dist\reference-app-$version-windows-$Arch-setup.exe"
        (Get-Item $installers[$version]).VersionInfo | Format-List FileVersion, ProductVersion | Out-String | Write-Host
    }
    Publish-Feed $installers[$NewVersion] 'inno-setup' $Site $KeyFile $PublicKey

    $setup = Start-Process $installers[$OldVersion] -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NORUN=1',
        "/LOG=`"$(Join-Path $root 'install-1.0.0.log')`"", "/DIR=`"$installDir`""
    )
    if ($setup.ExitCode -ne 0) { throw "installing $OldVersion failed with exit code $($setup.ExitCode)" }
    $exe = Join-Path $installDir 'reference-app.exe'
    Assert-Version $exe $OldVersion
    Remove-Item $report -ErrorAction SilentlyContinue

    $old = Start-Process $exe -PassThru
    # Caching the handle keeps ExitCode readable after the process exits.
    $null = $old.Handle
    try {
        Wait-Report $report @("started $OldVersion", "update-available $NewVersion", 'handoff') @($innoLog) | Out-Null
        if (-not $old.WaitForExit($TimeoutSeconds * 1000)) { throw "$OldVersion did not quit after handing off" }
        if ($old.ExitCode -ne 0) { throw "$OldVersion exited with code $($old.ExitCode) instead of quitting cleanly" }
        $lines = Wait-Report $report @(
            "started $OldVersion",
            "update-available $NewVersion",
            'handoff',
            'installer-replacing-files',
            "installer-done $NewVersion",
            "started $NewVersion",
            'up-to-date'
        ) @($innoLog)
        $replacing = @($lines | Where-Object { $_ -like 'installer-replacing-files *' })
        if ($replacing.Count -ne 1 -or $replacing[0] -notlike '* app-running=no *') {
            Fail-Report $report 'the installer replaced files while the old version was still running' @($innoLog)
        }
        if ($replacing[0] -notlike "* dir=$installDir") {
            Fail-Report $report "the installer did not update $installDir in place" @($innoLog)
        }
        Assert-Version $exe $NewVersion
        $defaultDir = Join-Path $env:LOCALAPPDATA "Programs\$AppName"
        if (Test-Path (Join-Path $defaultDir 'reference-app.exe')) {
            throw "the update created a second installation in $defaultDir"
        }
    } finally {
        Stop-ReferenceApp $exe
        $uninstaller = Join-Path $installDir 'unins000.exe'
        if (Test-Path $uninstaller) {
            Start-Process $uninstaller -Wait -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') | Out-Null
        }
    }
    Write-Host "inno-setup ($Arch): $OldVersion -> $NewVersion passed"
}

function Test-PortableFlow {
    param([string] $KeyFile, [string] $PublicKey, [string] $Site)
    $root = Join-Path $WorkDir 'portable'
    $report = Join-Path $root 'report.log'
    $installDir = Join-Path $root 'Portable App'
    $feedUrl = "http://127.0.0.1:$Port/portable/appcast-windows-$Arch.xml"
    $appId = 'dev.gpui-auto-update.reference-app.e2e-portable'

    $builds = @{}
    foreach ($version in @($OldVersion, $NewVersion)) {
        $builds[$version] = Join-Path $root "build\$version\reference-app.exe"
        Build-ReferenceApp $version 'portable' $appId $feedUrl $PublicKey $report $builds[$version]
    }
    $artifact = Join-Path $root "dist\reference-app-$NewVersion-windows-$Arch.exe"
    New-Item -ItemType Directory -Force (Split-Path $artifact) | Out-Null
    Copy-Item $builds[$NewVersion] $artifact -Force
    Publish-Feed $artifact 'portable' $Site $KeyFile $PublicKey

    New-Item -ItemType Directory -Force $installDir | Out-Null
    $exe = Join-Path $installDir 'reference-app.exe'
    Copy-Item $builds[$OldVersion] $exe -Force
    Remove-Item $report -ErrorAction SilentlyContinue

    $old = Start-Process $exe -PassThru
    # Caching the handle keeps ExitCode readable after the process exits.
    $null = $old.Handle
    try {
        Wait-Report $report @("started $OldVersion", "update-available $NewVersion", 'handoff') | Out-Null
        if (-not $old.WaitForExit($TimeoutSeconds * 1000)) { throw "$OldVersion did not quit after handing off" }
        if ($old.ExitCode -ne 0) { throw "$OldVersion exited with code $($old.ExitCode) instead of quitting cleanly" }
        Wait-Report $report @(
            "started $OldVersion",
            "update-available $NewVersion",
            'handoff',
            "started $NewVersion",
            'up-to-date'
        ) | Out-Null
        Assert-Version $exe $NewVersion
        $leftovers = @(Get-ChildItem $installDir -Filter 'reference-app.exe.previous*')
        if ($leftovers.Count -ne 0) {
            throw "the previous executable was not cleaned up: $($leftovers.Name -join ', ')"
        }
    } finally {
        Stop-ReferenceApp $exe
    }
    Write-Host "portable ($Arch): $OldVersion -> $NewVersion passed"
}

if ($env:PROCESSOR_ARCHITECTURE -ne $Packaging.Host) {
    throw "-Arch $Arch needs a $($Packaging.Host) Windows host; this one is $env:PROCESSOR_ARCHITECTURE"
}
if (Test-Path $WorkDir) { Remove-Item $WorkDir -Recurse -Force }
New-Item -ItemType Directory -Force $WorkDir | Out-Null
$WorkDir = (Resolve-Path $WorkDir).Path

Push-Location $Repo
try {
    Invoke-Native cargo @('build', '--locked', '-p', 'gpui-auto-update-cli', '--target', $Packaging.Triple)
    $script:Cli = Join-Path $Repo "target\$($Packaging.Triple)\debug\gpui-auto-update.exe"

    # A disposable key for this run only.
    $keyFile = Join-Path $WorkDir 'signing-key'
    $publicKey = (& $script:Cli keys generate --output $keyFile | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $publicKey) { throw 'could not generate a signing key' }

    $site = Join-Path $WorkDir 'site'
    New-Item -ItemType Directory -Force $site | Out-Null
    $server = Start-FeedServer $site
    try {
        if ($Flow -contains 'inno-setup') { Test-InnoSetupFlow $keyFile $publicKey $site }
        if ($Flow -contains 'portable') { Test-PortableFlow $keyFile $publicKey $site }
    } finally {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
} finally {
    Pop-Location
}

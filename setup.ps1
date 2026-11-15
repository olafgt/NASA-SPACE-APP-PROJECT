<#
.SYNOPSIS
  Checks (and optionally installs) everything the Astro-Telemetry prototype needs on Windows 10/11.

.DESCRIPTION
  Checks:  Windows build, Bluetooth radio, Visual Studio C++ Build Tools, Rust (MSVC, >= 1.80),
           Python (>= 3.8, Computer 2 only), internet access, project files.
  Installs what is missing, using winget when it exists and a direct download otherwise
  (VS Build Tools and Rust from Microsoft/rustup, Python from python.org).
  Does not touch anything that is already present.
  Everything printed is also written to setup.log next to this script.

.PARAMETER Role
  computer1 = bio-chip simulator (no Python needed), computer2 = Ground Control, both (default).
.PARAMETER CheckOnly
  Report only, install nothing.
.PARAMETER Yes
  Do not ask for confirmation before installing.
.PARAMETER Build
  After the checks, run the unit tests and a release build for the selected role.

.EXAMPLE
  .\setup.bat                          # check + install everything (asks first)
  .\setup.bat -Role computer1 -Build   # Computer 1 only, then compile it
  .\setup.bat -CheckOnly               # just report
#>
[CmdletBinding()]
param(
    [ValidateSet('computer1', 'computer2', 'both')][string]$Role = 'both',
    [switch]$CheckOnly,
    [switch]$Yes,
    [switch]$Build
)

# Windows PowerShell 5.1 turns native stderr into errors under 'Stop', so stay on 'Continue'.
$ErrorActionPreference = 'Continue'
Set-Location -LiteralPath $PSScriptRoot
$LogPath = Join-Path $PSScriptRoot 'setup.log'
$script:Transcribing = $false
try { Start-Transcript -Path $LogPath -Force | Out-Null; $script:Transcribing = $true } catch { }

$needComputer1 = $Role -in 'computer1', 'both'
$needComputer2 = $Role -in 'computer2', 'both'
$MinRustMajor = 1; $MinRustMinor = 80
$MinPyMinor = 8
$WingetAlready = -1978335189   # 0x8A150011: already installed / no applicable update

# ---------------------------------------------------------------- helpers
function Write-Status($tag, $color, $name, $detail) {
    Write-Host ("  [{0}] " -f $tag) -ForegroundColor $color -NoNewline
    Write-Host ("{0,-28}" -f $name) -NoNewline
    Write-Host $detail -ForegroundColor DarkGray
}

function Update-SessionPath {
    $m = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $u = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = "$m;$u;$env:USERPROFILE\.cargo\bin"
}

# Run a native program with its output shown on screen; returns ONLY the exit code.
function Invoke-Native([string]$exe, [string[]]$arguments) {
    Write-Host "  > $exe $($arguments -join ' ')" -ForegroundColor DarkGray
    & $exe @arguments | Out-Host
    return $LASTEXITCODE
}

# Run a program and capture its text output (stdout + stderr). Never throws.
function Get-NativeText([string]$exe, [string[]]$arguments) {
    try { return ((& $exe @arguments 2>&1 | ForEach-Object { "$_" }) -join "`n").Trim() }
    catch { return '' }
}

function Get-RustInfo {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) { return $null }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { return $null }
    $ver = Get-NativeText 'rustc' @('--version')
    $full = Get-NativeText 'rustc' @('-vV')
    if ($ver -match 'rustc (\d+)\.(\d+)\.(\d+)') {
        $maj = [int]$Matches[1]; $min = [int]$Matches[2]
        $ok = ($maj -gt $MinRustMajor) -or ($maj -eq $MinRustMajor -and $min -ge $MinRustMinor)
        return [pscustomobject]@{ Text = $ver; VersionOk = $ok; Msvc = [bool]($full -match 'host:.*msvc') }
    }
    return $null
}

function Get-PythonInfo {
    $candidates = @(
        @{ Exe = 'python';  Pre = @() },
        @{ Exe = 'py';      Pre = @('-3') },
        @{ Exe = 'python3'; Pre = @() }
    )
    foreach ($c in $candidates) {
        if (-not (Get-Command $c.Exe -ErrorAction SilentlyContinue)) { continue }
        $argList = @($c.Pre) + @('--version')
        $out = Get-NativeText $c.Exe $argList
        # The Microsoft Store "python.exe" stub prints a hint instead of "Python 3.x" -> skipped here.
        if ($out -match '^Python (\d+)\.(\d+)\.(\d+)' -and [int]$Matches[1] -eq 3 -and [int]$Matches[2] -ge $MinPyMinor) {
            return [pscustomobject]@{ Exe = $c.Exe; Pre = @($c.Pre); Text = $out }
        }
    }
    return $null
}

function Test-VsBuildTools {
    $pf86 = ${env:ProgramFiles(x86)}
    if (-not $pf86) { $pf86 = $env:ProgramFiles }
    $vw = Join-Path $pf86 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vw)) { return $false }
    $p = Get-NativeText $vw @('-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath')
    return [bool]($p -and (Test-Path -LiteralPath ($p -split "`n")[0].Trim()))
}

function Test-Tcp([string]$hostName, [int]$port, [int]$timeoutMs = 4000) {
    try {
        $c = New-Object System.Net.Sockets.TcpClient
        $r = $c.BeginConnect($hostName, $port, $null, $null)
        $ok = $r.AsyncWaitHandle.WaitOne($timeoutMs, $false) -and $c.Connected
        $c.Close()
        return [bool]$ok
    }
    catch { return $false }
}

function Get-RemoteFile([string]$url, [string]$dest) {
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }
    $old = $ProgressPreference; $ProgressPreference = 'SilentlyContinue'
    try {
        Write-Host "  > downloading $url" -ForegroundColor DarkGray
        Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
        return (Test-Path -LiteralPath $dest)
    }
    catch { Write-Host "  download failed: $($_.Exception.Message)" -ForegroundColor Red; return $false }
    finally { $ProgressPreference = $old }
}

function Install-WithWinget([string]$id, [string]$override) {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { return $false }
    $a = @('install', '--id', $id, '-e', '--accept-package-agreements', '--accept-source-agreements')
    if ($override) { $a += @('--override', $override) }
    $rc = Invoke-Native 'winget' $a
    return ($rc -eq 0 -or $rc -eq $WingetAlready)
}

function Install-VsBuildTools {
    $vsArgs = '--quiet --wait --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    Write-Host '  (a Windows UAC prompt will appear; this can take 5-20 minutes)' -ForegroundColor DarkGray
    if (Install-WithWinget 'Microsoft.VisualStudio.2022.BuildTools' $vsArgs) { if (Test-VsBuildTools) { return $true } }
    Write-Host '  winget did not do it - trying the direct download ...' -ForegroundColor Yellow
    $exe = Join-Path $env:TEMP 'vs_BuildTools.exe'
    if (-not (Get-RemoteFile 'https://aka.ms/vs/17/release/vs_BuildTools.exe' $exe)) { return $false }
    try {
        $p = Start-Process -FilePath $exe -ArgumentList ($vsArgs -split ' ') -Verb RunAs -Wait -PassThru
        Write-Host "  installer exit code $($p.ExitCode)" -ForegroundColor DarkGray
        return ($p.ExitCode -eq 0 -or $p.ExitCode -eq 3010)
    }
    catch { Write-Host "  installer could not be started: $($_.Exception.Message)" -ForegroundColor Red; return $false }
}

function Install-Rust {
    # rustup-init downloaded straight from rustup.rs works with or without winget.
    $exe = Join-Path $env:TEMP 'rustup-init.exe'
    $ok = $false
    if (Get-RemoteFile 'https://win.rustup.rs/x86_64' $exe) {
        $rc = Invoke-Native $exe @('-y', '--default-host', 'x86_64-pc-windows-msvc', '--default-toolchain', 'stable')
        $ok = ($rc -eq 0)
    }
    if (-not $ok) {
        Write-Host '  direct rustup install failed - trying winget ...' -ForegroundColor Yellow
        $ok = Install-WithWinget 'Rustlang.Rustup' $null
        Update-SessionPath
        if ($ok -and (Get-Command rustup -ErrorAction SilentlyContinue)) { [void](Invoke-Native 'rustup' @('default', 'stable-msvc')) }
    }
    Update-SessionPath
    return $ok
}

function Update-Rust {
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { return $false }
    $rc1 = Invoke-Native 'rustup' @('default', 'stable-msvc')
    $rc2 = Invoke-Native 'rustup' @('update', 'stable-msvc')
    return ($rc1 -eq 0 -and $rc2 -eq 0)
}

function Install-Python {
    if (Install-WithWinget 'Python.Python.3.12' $null) { Update-SessionPath; if (Get-PythonInfo) { return $true } }
    Write-Host '  winget did not do it - trying the direct download ...' -ForegroundColor Yellow
    $exe = Join-Path $env:TEMP 'python-3.12.8-amd64.exe'
    if (-not (Get-RemoteFile 'https://www.python.org/ftp/python/3.12.8/python-3.12.8-amd64.exe' $exe)) { return $false }
    $rc = Invoke-Native $exe @('/quiet', 'InstallAllUsers=0', 'PrependPath=1', 'Include_test=0', 'Include_launcher=1')
    Update-SessionPath
    return ($rc -eq 0)
}

# ---------------------------------------------------------------- main
function Invoke-Setup {
    $problems = New-Object System.Collections.ArrayList
    $warnings = New-Object System.Collections.ArrayList
    $missing = [ordered]@{}   # name -> installer function name

    Write-Host ''
    Write-Host "Astro-Telemetry setup  (role: $Role)" -ForegroundColor Cyan
    Write-Host '------------------------------------------------------------'
    Update-SessionPath

    # --- project files (also catches "run from inside the zip")
    foreach ($f in 'Cargo.toml', 'telemetry-protocol\src\lib.rs') {
        if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $f))) {
            [void]$problems.Add("Project file missing: $f. Extract the whole zip first and run setup.bat from the astro-telemetry folder (not from inside the zip).")
        }
    }
    if ($needComputer1 -and -not (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'bio-chip-sim\src\main.rs'))) { [void]$problems.Add('bio-chip-sim\ folder missing') }
    if ($needComputer2) {
        foreach ($f in 'analysis.py', 'ext_analysis.py') {
            if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot "ground-control\python\$f"))) { [void]$problems.Add("ground-control\python\$f missing") }
        }
    }
    if ($problems.Count -gt 0) { return @{ Problems = $problems; Warnings = $warnings } }
    Write-Status 'OK' Green 'Project files' $PSScriptRoot

    # --- Windows
    $build = [Environment]::OSVersion.Version.Build
    if ($build -ge 17134) { Write-Status 'OK' Green 'Windows' "build $build" }
    else { Write-Status 'WARN' Yellow 'Windows' "build $build - BLE peripheral needs Windows 10 1803 (17134)+"; [void]$warnings.Add('Old Windows build') }

    # --- Bluetooth (informational)
    $bt = @()
    if (Get-Command Get-PnpDevice -ErrorAction SilentlyContinue) {
        $bt = @(Get-PnpDevice -Class Bluetooth -Status OK -ErrorAction SilentlyContinue | Where-Object { $_.InstanceId -match '^(USB|PCI|ACPI|ROOT)\\' -and $_.FriendlyName -notmatch 'Enumerator|RFCOMM|Generic|Service|Transport' })
    }
    if ($bt.Count -gt 0) { Write-Status 'OK' Green 'Bluetooth radio' ($bt[0].FriendlyName) }
    else { Write-Status 'WARN' Yellow 'Bluetooth radio' 'none found/enabled - BLE needs one (TCP loopback mode works without)'; [void]$warnings.Add('No Bluetooth radio detected') }

    # --- winget (optional: direct downloads are used when it is missing)
    $haveWinget = [bool](Get-Command winget -ErrorAction SilentlyContinue)
    if ($haveWinget) { Write-Status 'OK' Green 'winget' 'available' }
    else { Write-Status 'INFO' DarkGray 'winget' 'not found - will use direct downloads instead' }

    # --- C++ Build Tools
    if (Test-VsBuildTools) { Write-Status 'OK' Green 'VS C++ Build Tools' 'found' }
    else { Write-Status 'MISSING' Red 'VS C++ Build Tools' 'needed to link Rust programs (~3-6 GB)'; $missing['VS C++ Build Tools'] = 'Install-VsBuildTools' }

    # --- Rust
    $rust = Get-RustInfo
    if ($rust -and $rust.VersionOk -and $rust.Msvc) { Write-Status 'OK' Green 'Rust (MSVC)' $rust.Text }
    elseif ($rust) {
        $why = @(); if (-not $rust.VersionOk) { $why += "needs >= $MinRustMajor.$MinRustMinor" }; if (-not $rust.Msvc) { $why += 'not the MSVC toolchain' }
        Write-Status 'FIX' Yellow 'Rust (MSVC)' ("$($rust.Text) - " + ($why -join ', '))
        $missing['Rust update'] = 'Update-Rust'
    }
    else { Write-Status 'MISSING' Red 'Rust (rustup)' 'not found'; $missing['Rust (rustup)'] = 'Install-Rust' }

    # --- Python (Computer 2 only)
    if ($needComputer2) {
        $py = Get-PythonInfo
        if ($py) { Write-Status 'OK' Green 'Python' ("$($py.Text) via '$($py.Exe) $($py.Pre -join ' ')'".Trim()) }
        else { Write-Status 'MISSING' Red 'Python 3.8+' 'needed for analysis.py (the Store stub does not count)'; $missing['Python 3'] = 'Install-Python' }
    }
    else { Write-Status 'SKIP' DarkGray 'Python' 'not needed on Computer 1' }

    # --- Internet
    $net = Test-Tcp 'index.crates.io' 443
    if ($net) { Write-Status 'OK' Green 'Internet (crates.io)' 'reachable' }
    else { Write-Status 'WARN' Yellow 'Internet (crates.io)' 'unreachable - installs and the first build need internet'; [void]$warnings.Add('crates.io unreachable (proxy or offline?)') }

    # --- install what is missing
    if ($missing.Count -gt 0 -and $CheckOnly) {
        [void]$problems.Add('Missing (CheckOnly, nothing installed): ' + ($missing.Keys -join ', '))
    }
    elseif ($missing.Count -gt 0) {
        Write-Host ''
        Write-Host ('To install: ' + ($missing.Keys -join ', ')) -ForegroundColor Cyan
        $go = $true
        if (-not $Yes) {
            try { $a = Read-Host 'Proceed? [Y/n]'; $go = ($a -eq '' -or $a -match '^[yY]') } catch { $go = $true }
        }
        if (-not $go) { [void]$problems.Add('Installation declined: ' + ($missing.Keys -join ', ')) }
        else {
            foreach ($name in @($missing.Keys)) {
                Write-Host "`nInstalling $name ..." -ForegroundColor Cyan
                $ok = $false
                try { $ok = [bool](& $missing[$name] | Select-Object -Last 1) }
                catch { Write-Host "  error: $($_.Exception.Message)" -ForegroundColor Red }
                Update-SessionPath
                if ($ok) { Write-Host "  $name installer finished." -ForegroundColor Green }
                else { Write-Host "  $name installer FAILED (see output above)." -ForegroundColor Red }
            }
            Write-Host "`nRe-checking ..." -ForegroundColor Cyan
            if (-not (Test-VsBuildTools)) { [void]$problems.Add('VS C++ Build Tools still missing. Reboot, or install "Desktop development with C++" manually from https://visualstudio.microsoft.com/visual-cpp-build-tools/ and re-run.') }
            $rust = Get-RustInfo
            if (-not ($rust -and $rust.VersionOk -and $rust.Msvc)) { [void]$problems.Add('Rust (MSVC >= 1.80) not ready. Close this window, open a NEW one and re-run setup.bat.') }
            if ($needComputer2 -and -not (Get-PythonInfo)) { [void]$problems.Add('Python 3.8+ not found yet. Close this window, open a NEW one and re-run setup.bat (or install from https://www.python.org/downloads/ with "Add to PATH").') }
        }
    }

    # --- verify (+ optional build)
    if ($problems.Count -eq 0) {
        if ($needComputer2) {
            $py = Get-PythonInfo
            if ($py) {
                $argList = @($py.Pre) + @('ground-control\python\analysis.py', '--selftest')
                $out = Get-NativeText $py.Exe $argList
                if ($out -match 'EXTENDED SELFTEST PASSED') { Write-Status 'OK' Green 'analysis engine self-test' 'core + 19 hazard features passed' }
                else { Write-Status 'FAIL' Red 'analysis engine self-test' 'did not pass'; Write-Host $out -ForegroundColor DarkGray; [void]$problems.Add('analysis engine self-test failed') }
            }
        }
        if ($Build -and $problems.Count -eq 0) {
            Write-Host "`nBuilding (first build downloads and compiles crates: several minutes) ..." -ForegroundColor Cyan
            $pkgs = @('telemetry-protocol')
            if ($needComputer1) { $pkgs += 'bio-chip-sim' }
            if ($needComputer2) { $pkgs += 'ground-control' }
            $t = @('test', '--release'); foreach ($p in $pkgs) { $t += @('-p', $p) }
            if ((Invoke-Native 'cargo' $t) -ne 0) { [void]$problems.Add('cargo test failed - see output above / setup.log') }
            else {
                $b = @('build', '--release'); foreach ($p in $pkgs) { $b += @('-p', $p) }
                if ((Invoke-Native 'cargo' $b) -ne 0) { [void]$problems.Add('cargo build failed - see output above / setup.log') }
                else { Write-Status 'OK' Green 'cargo build --release' 'finished (target\release\)' }
            }
        }
    }
    return @{ Problems = $problems; Warnings = $warnings }
}

$rc = 1
try {
    $res = @(Invoke-Setup) | Select-Object -Last 1
    Write-Host ''
    Write-Host '------------------------------------------------------------'
    foreach ($w in $res.Warnings) { Write-Host "  warning: $w" -ForegroundColor Yellow }
    if ($res.Problems.Count -eq 0) {
        Write-Host 'READY.' -ForegroundColor Green
        if ($needComputer1) { Write-Host '  Computer 1:  cargo run --release -p bio-chip-sim -- --mode ble' }
        if ($needComputer2) { Write-Host '  Computer 2:  cargo run --release -p ground-control -- --source ble' }
        Write-Host '  Single PC:   .\scripts\run-single-pc-loopback.ps1'
        $rc = 0
    }
    else {
        foreach ($p in $res.Problems) { Write-Host "  problem: $p" -ForegroundColor Red }
        Write-Host "  (full log: $LogPath)" -ForegroundColor DarkGray
    }
}
catch {
    Write-Host ''
    Write-Host 'UNEXPECTED ERROR in setup.ps1:' -ForegroundColor Red
    Write-Host ($_ | Out-String) -ForegroundColor Red
    Write-Host "  (full log: $LogPath)" -ForegroundColor DarkGray
    $rc = 2
}
finally {
    if ($script:Transcribing) { try { Stop-Transcript | Out-Null } catch { } }
}
exit $rc

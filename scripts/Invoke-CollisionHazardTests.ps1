[CmdletBinding()]
param(
    [ValidateSet('Quick', 'Full', 'Stress')]
    [string]$Profile = 'Quick',

    [string]$Stage = 'All',

    [switch]$ValidateOnly,

    [ValidateRange(2, 100)]
    [int]$Repeat = 20,

    [ValidateRange(15, 1800)]
    [int]$PerTestTimeoutSeconds = 180,

    [ValidateRange(512, 1048576)]
    [int]$MinimumFreeMemoryMb = 2048,

    [ValidateRange(50, 100)]
    [int]$MaximumCpuPercent = 95,

    [ValidateRange(5, 900)]
    [int]$PressureWaitSeconds = 60,

    [ValidateRange(0, 10000)]
    [int]$PauseMilliseconds = 250,

    [string]$OutputDirectory,

    [switch]$StopOnFailure,

    [switch]$ListStages
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:RepoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$script:ManifestPath = Join-Path $PSScriptRoot 'collision-hazard-tests.psd1'
$script:CargoManifest = Join-Path $script:RepoRoot 'src-tauri\Cargo.toml'

function Assert-RepositoryIdentity {
    $packagePath = Join-Path $script:RepoRoot 'package.json'
    $windowsVariable = Get-Variable -Name IsWindows -ErrorAction SilentlyContinue
    $isWindowsHost = if ($windowsVariable) { [bool]$windowsVariable.Value } else { $env:OS -eq 'Windows_NT' }
    if (-not $isWindowsHost) {
        throw 'This hazard suite certifies Windows behavior and refuses to run on a non-Windows host.'
    }
    if (-not (Test-Path -LiteralPath $script:CargoManifest -PathType Leaf)) {
        throw "Wrong repository: missing $script:CargoManifest"
    }
    if (-not (Test-Path -LiteralPath $packagePath -PathType Leaf)) {
        throw "Wrong repository: missing $packagePath"
    }
    $package = Get-Content -Raw -LiteralPath $packagePath | ConvertFrom-Json
    if ($package.name -ne 'perfect-planner-desktop') {
        throw "Wrong repository: expected package perfect-planner-desktop, found '$($package.name)'."
    }
    $gitRoot = (& git -C $script:RepoRoot rev-parse --show-toplevel 2>$null).Trim()
    if (-not $gitRoot -or [IO.Path]::GetFullPath($gitRoot) -ne $script:RepoRoot) {
        throw "Worktree identity mismatch: Git resolved '$gitRoot', expected '$script:RepoRoot'."
    }
}

function Resolve-SafeOutputDirectory {
    param([string]$Requested)

    $artifactRoot = [IO.Path]::GetFullPath((Join-Path $script:RepoRoot 'artifacts\hazard-tests'))
    $candidate = if ([string]::IsNullOrWhiteSpace($Requested)) {
        Join-Path $artifactRoot ("$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ'))-$PID")
    } elseif ([IO.Path]::IsPathRooted($Requested)) {
        [IO.Path]::GetFullPath($Requested)
    } else {
        [IO.Path]::GetFullPath((Join-Path $script:RepoRoot $Requested))
    }

    $rootWithSeparator = $artifactRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not ($candidate + [IO.Path]::DirectorySeparatorChar).StartsWith($rootWithSeparator, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Unsafe output path: '$candidate' must remain below '$artifactRoot'."
    }
    if ($candidate.Equals($artifactRoot, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Unsafe output path: each run requires its own child directory below artifacts\hazard-tests.'
    }

    $relative = $candidate.Substring($script:RepoRoot.Length).TrimStart('\')
    $current = $script:RepoRoot
    foreach ($segment in ($relative -split '\\')) {
        if (-not $segment) { continue }
        $current = Join-Path $current $segment
        if (-not (Test-Path -LiteralPath $current)) { continue }
        $item = Get-Item -Force -LiteralPath $current
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Unsafe output path: existing component '$current' is a reparse point."
        }
    }
    if (Test-Path -LiteralPath $candidate) {
        throw "Output collision: '$candidate' already exists; refusing to overwrite evidence."
    }
    return $candidate
}

function New-ProcessResult {
    param(
        [string]$FileName,
        [string[]]$Arguments,
        [int]$TimeoutSeconds
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FileName
    $startInfo.WorkingDirectory = $script:RepoRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $quotedArguments = foreach ($argument in $Arguments) {
        if ($argument.Contains('"')) {
            throw "Unsafe quote in native process argument: $argument"
        }
        if ($argument -match '\s') { '"' + $argument + '"' } else { $argument }
    }
    $startInfo.Arguments = $quotedArguments -join ' '

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) {
        throw "Failed to start $FileName."
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $peakWorkingSet = [int64]0
    while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        try {
            $process.Refresh()
            $peakWorkingSet = [math]::Max($peakWorkingSet, $process.WorkingSet64)
        } catch {
            # A process can exit between HasExited and Refresh. The exit state is checked below.
        }
        Start-Sleep -Milliseconds 100
    }
    $timedOut = -not $process.HasExited
    if ($timedOut) {
        $killer = [Diagnostics.ProcessStartInfo]::new()
        $killer.FileName = 'taskkill.exe'
        $killer.Arguments = "/PID $($process.Id) /T /F"
        $killer.UseShellExecute = $false
        $killer.CreateNoWindow = $true
        try {
            $killProcess = [Diagnostics.Process]::Start($killer)
            [void]$killProcess.WaitForExit(10000)
        } catch {
            try { $process.Kill() } catch { }
        }
    }
    if (-not $process.HasExited) {
        try { $process.Kill() } catch { }
    }
    if (-not $process.HasExited) {
        throw "Timed-out process $FileName could not be terminated."
    }
    if ($process.HasExited) {
        $process.WaitForExit()
    }
    $stopwatch.Stop()
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()

    [pscustomobject]@{
        ExitCode = if ($timedOut) { -1 } else { $process.ExitCode }
        TimedOut = $timedOut
        DurationMs = $stopwatch.ElapsedMilliseconds
        PeakWorkingSetMb = [math]::Round($peakWorkingSet / 1MB, 1)
        Stdout = $stdout
        Stderr = $stderr
    }
}

function Get-ResourceSnapshot {
    try {
        $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction Stop
        $processors = @(Get-CimInstance -ClassName Win32_Processor -ErrorAction Stop)
        if (-not $os -or $processors.Count -eq 0) {
            throw 'Windows resource sensors returned no data.'
        }
        $cpu = [math]::Round((($processors | Measure-Object -Property LoadPercentage -Average).Average), 1)
        [pscustomobject]@{
            CapturedAt = [DateTime]::UtcNow.ToString('o')
            FreeMemoryMb = [math]::Floor([double]$os.FreePhysicalMemory / 1024)
            CpuPercent = $cpu
        }
    } catch {
        throw "Cannot prove host resource pressure safely: $($_.Exception.Message)"
    }
}

function Wait-ResourceBudget {
    $deadline = [DateTime]::UtcNow.AddSeconds($PressureWaitSeconds)
    do {
        $snapshot = Get-ResourceSnapshot
        if ($snapshot.FreeMemoryMb -ge $MinimumFreeMemoryMb -and $snapshot.CpuPercent -le $MaximumCpuPercent) {
            return $snapshot
        }
        Write-Warning "Resource gate paused: free RAM $($snapshot.FreeMemoryMb) MB; CPU $($snapshot.CpuPercent)%."
        Start-Sleep -Seconds 2
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "Resource pressure did not clear within $PressureWaitSeconds seconds (requires >= $MinimumFreeMemoryMb MB free RAM and <= $MaximumCpuPercent% CPU)."
}

function Get-SelectedStages {
    param([object[]]$Definitions)

    $known = @($Definitions | ForEach-Object { [string]$_.Name })
    $requested = @($Stage -split ',' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    if ($requested.Count -eq 1 -and $requested[0] -eq 'All') {
        return @($Definitions)
    }
    $unknown = @($requested | Where-Object { $_ -notin $known })
    if ($unknown.Count -gt 0) {
        throw "Unknown stage(s): $($unknown -join ', '). Valid stages: $($known -join ', ')."
    }
    return @($Definitions | Where-Object { $_.Name -in $requested })
}

function Get-TestsForStage {
    param([object]$Definition)

    $tests = @($Definition.QuickTests)
    if ($Profile -in @('Full', 'Stress')) {
        $tests += @($Definition.FullTests)
    }
    if ($Profile -eq 'Stress' -and $Definition.ContainsKey('StressTests')) {
        $tests += @($Definition.StressTests)
    }
    return @($tests | Select-Object -Unique)
}

function Get-AttemptsForTest {
    param(
        [object]$Definition,
        [string]$TestName
    )
    if ($Profile -ne 'Stress' -or -not $Definition.ContainsKey('StressTests')) {
        return 1
    }
    if ($TestName -in @($Definition.StressTests)) {
        return $Repeat
    }
    return 1
}

function Assert-TestInventory {
    param([object[]]$SelectedStages)

    $list = New-ProcessResult -FileName 'cargo' -Arguments @(
        'test', '--manifest-path', $script:CargoManifest, '--', '--list'
    ) -TimeoutSeconds $PerTestTimeoutSeconds
    if ($list.TimedOut -or $list.ExitCode -ne 0) {
        throw "Could not enumerate Cargo tests. Exit $($list.ExitCode).`n$($list.Stderr)"
    }
    $available = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($line in ($list.Stdout -split "`r?`n")) {
        if ($line -match '^(?<name>[^:]+(?:::[^:]+)+): test$') {
            [void]$available.Add($Matches.name)
        }
    }

    $configured = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($definition in $SelectedStages) {
        foreach ($test in (Get-TestsForStage $definition)) {
            if (-not $configured.Add([string]$test)) {
                throw "Duplicate test assignment in selected stages: $test"
            }
            if (-not $available.Contains([string]$test)) {
                throw "Configured Cargo hazard test does not exist exactly: $test"
            }
        }
    }
    return $configured.Count
}

function ConvertTo-SafeFileName {
    param([string]$Value)
    return ($Value -replace '[^A-Za-z0-9_.-]', '_')
}

function Get-SourceFingerprint {
    $head = (& git -C $script:RepoRoot rev-parse HEAD 2>$null).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $head) {
        throw 'Cannot fingerprint source: git rev-parse failed.'
    }
    $diff = (& git -C $script:RepoRoot diff --binary HEAD -- 2>$null) -join "`n"
    if ($LASTEXITCODE -ne 0) {
        throw 'Cannot fingerprint source: git diff failed.'
    }
    $untracked = @(& git -C $script:RepoRoot ls-files --others --exclude-standard 2>$null | Sort-Object)
    if ($LASTEXITCODE -ne 0) {
        throw 'Cannot fingerprint source: untracked-file inventory failed.'
    }

    $builder = [Text.StringBuilder]::new()
    [void]$builder.AppendLine($head)
    [void]$builder.AppendLine($diff)
    foreach ($relativePath in $untracked) {
        $absolutePath = Join-Path $script:RepoRoot $relativePath
        if (-not (Test-Path -LiteralPath $absolutePath -PathType Leaf)) {
            throw "Cannot fingerprint source: untracked file disappeared: $relativePath"
        }
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $absolutePath).Hash.ToLowerInvariant()
        [void]$builder.AppendLine("$relativePath=$hash")
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($builder.ToString())
        return ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

try {
    Assert-RepositoryIdentity
    if (-not (Test-Path -LiteralPath $script:ManifestPath -PathType Leaf)) {
        throw "Hazard manifest missing: $script:ManifestPath"
    }
    $manifest = Import-PowerShellDataFile -LiteralPath $script:ManifestPath
    if ($manifest.SchemaVersion -ne 1 -or -not $manifest.Stages) {
        throw 'Unsupported or empty hazard manifest.'
    }

    if ($ListStages) {
        foreach ($definition in $manifest.Stages) {
            [pscustomobject]@{
                Stage = $definition.Name
                Step = $definition.Step
                Quick = @($definition.QuickTests).Count
                Full = @($definition.QuickTests).Count + @($definition.FullTests).Count
                StressRepeated = if ($definition.ContainsKey('StressTests')) { @($definition.StressTests).Count } else { 0 }
                Hazards = ($definition.Hazards -join '; ')
            }
        }
        exit 0
    }

    $selectedStages = @(Get-SelectedStages $manifest.Stages)
    $safeOutput = Resolve-SafeOutputDirectory $OutputDirectory
    $sourceFingerprintBefore = Get-SourceFingerprint
    $testCount = Assert-TestInventory $selectedStages
    try {
        $initialPressure = Wait-ResourceBudget
    } catch {
        [Console]::Error.WriteLine($_.Exception.Message)
        exit 3
    }
    if ($ValidateOnly) {
        Write-Host "VALID: $testCount exact hazard tests across $($selectedStages.Count) stage(s)."
        Write-Host "RESOURCE GATE: $($initialPressure.FreeMemoryMb) MB free RAM, $($initialPressure.CpuPercent)% CPU."
        Write-Host "SOURCE FINGERPRINT: $sourceFingerprintBefore"
        Write-Host "OUTPUT: $safeOutput"
        exit 0
    }

    [void](New-Item -ItemType Directory -Path $safeOutput)
    $results = [Collections.Generic.List[object]]::new()
    $abort = $false

    foreach ($definition in $selectedStages) {
        if ($abort) { break }
        foreach ($test in (Get-TestsForStage $definition)) {
            if ($abort) { break }
            $attempts = Get-AttemptsForTest -Definition $definition -TestName $test
            for ($attempt = 1; $attempt -le $attempts; $attempt++) {
                try {
                    $pressure = Wait-ResourceBudget
                } catch {
                    $results.Add([pscustomobject]@{
                        Stage = $definition.Name; Test = $test; Attempt = $attempt
                        Status = 'PRESSURE_ABORT'; ExitCode = 3; DurationMs = 0
                        PeakWorkingSetMb = 0; Pressure = $null; Log = $null
                        Error = $_.Exception.Message
                    })
                    $abort = $true
                    break
                }

                $label = ConvertTo-SafeFileName "$($definition.Name)-$attempt-$test"
                $logPath = Join-Path $safeOutput "$label.log"
                Write-Host "[$($definition.Name)] attempt $attempt/$attempts $test"
                $run = New-ProcessResult -FileName 'cargo' -Arguments @(
                    'test', '--manifest-path', $script:CargoManifest, $test,
                    '--', '--exact', '--nocapture', '--test-threads=1'
                ) -TimeoutSeconds $PerTestTimeoutSeconds
                $status = if ($run.TimedOut) { 'TIMEOUT' } elseif ($run.ExitCode -eq 0) { 'PASS' } else { 'FAIL' }
                $log = @(
                    "stage=$($definition.Name)"
                    "step=$($definition.Step)"
                    "test=$test"
                    "attempt=$attempt/$attempts"
                    "status=$status"
                    "exit=$($run.ExitCode)"
                    "durationMs=$($run.DurationMs)"
                    "peakWorkingSetMb=$($run.PeakWorkingSetMb)"
                    "freeMemoryMbBefore=$($pressure.FreeMemoryMb)"
                    "cpuPercentBefore=$($pressure.CpuPercent)"
                    '--- stdout ---'
                    $run.Stdout
                    '--- stderr ---'
                    $run.Stderr
                ) -join [Environment]::NewLine
                Set-Content -LiteralPath $logPath -Value $log -Encoding utf8
                $results.Add([pscustomobject]@{
                    Stage = $definition.Name; Test = $test; Attempt = $attempt
                    Status = $status; ExitCode = $run.ExitCode; DurationMs = $run.DurationMs
                    PeakWorkingSetMb = $run.PeakWorkingSetMb; Pressure = $pressure
                    Log = $logPath.Substring($script:RepoRoot.Length).TrimStart('\'); Error = $null
                })

                if ($status -ne 'PASS' -and $StopOnFailure) {
                    $abort = $true
                    break
                }
                if ($PauseMilliseconds -gt 0) {
                    Start-Sleep -Milliseconds $PauseMilliseconds
                }
            }
        }
    }

    $finishedAt = [DateTime]::UtcNow
    $sourceFingerprintAfter = Get-SourceFingerprint
    $sourceStable = $sourceFingerprintBefore -eq $sourceFingerprintAfter
    $summary = [ordered]@{
        schemaVersion = 1
        profile = $Profile
        stages = @($selectedStages | ForEach-Object { $_.Name })
        startedAt = $initialPressure.CapturedAt
        finishedAt = $finishedAt.ToString('o')
        resourcePolicy = [ordered]@{
            minimumFreeMemoryMb = $MinimumFreeMemoryMb
            maximumCpuPercent = $MaximumCpuPercent
            pressureWaitSeconds = $PressureWaitSeconds
            perTestTimeoutSeconds = $PerTestTimeoutSeconds
            pauseMilliseconds = $PauseMilliseconds
        }
        source = [ordered]@{
            before = $sourceFingerprintBefore
            after = $sourceFingerprintAfter
            stable = $sourceStable
        }
        counts = [ordered]@{
            total = $results.Count
            passed = @($results | Where-Object Status -eq 'PASS').Count
            failed = @($results | Where-Object Status -eq 'FAIL').Count
            timedOut = @($results | Where-Object Status -eq 'TIMEOUT').Count
            pressureAborted = @($results | Where-Object Status -eq 'PRESSURE_ABORT').Count
            sourceDrift = if ($sourceStable) { 0 } else { 1 }
        }
        results = @($results)
    }
    $summaryPath = Join-Path $safeOutput 'summary.json'
    $summary | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $summaryPath -Encoding utf8
    Write-Host "SUMMARY: $summaryPath"
    Write-Host "PASS $($summary.counts.passed)/$($summary.counts.total); FAIL $($summary.counts.failed); TIMEOUT $($summary.counts.timedOut); PRESSURE_ABORT $($summary.counts.pressureAborted); SOURCE_DRIFT $($summary.counts.sourceDrift)"

    if (-not $sourceStable) { exit 5 }
    if ($summary.counts.pressureAborted -gt 0) { exit 3 }
    if ($summary.counts.timedOut -gt 0) { exit 4 }
    if ($summary.counts.failed -gt 0) { exit 1 }
    exit 0
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 2
}

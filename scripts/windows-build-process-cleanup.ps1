Set-StrictMode -Version Latest

# The native helper is loaded without performing its CLI Inspect/Stop action.
# One original Job handle survives root exit in the admitting parent. No
# snapshot-only or PID-only cleanup can certify all descendant retirement.
& (Join-Path $PSScriptRoot 'windows-build-process-owner.ps1') -Action Inspect -ProcessId $PID -LoadOnly
if (-not (Get-Variable QCBuildChildJobs -Scope Script -ErrorAction SilentlyContinue)) { $script:QCBuildChildJobs = @{} }

function Read-QcBuildOwnerDocument {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        # Software envelope, not a limit on native process metadata.
        $bytes = New-Object byte[] 8193
        $used = 0
        while ($used -lt $bytes.Length) {
            $count = $stream.Read($bytes, $used, $bytes.Length - $used)
            if ($count -eq 0) { break }
            $used += $count
        }
        if ($used -gt 8192) { throw 'Build owner document exceeded its 8 KiB software envelope.' }
        return ([Text.Encoding]::UTF8.GetString($bytes, 0, $used).TrimStart([char]0xFEFF) | ConvertFrom-Json -ErrorAction Stop)
    } finally { $stream.Dispose() }
}

function New-QcBuildAdmissionJob {
    param([string]$Name, [int]$ProcessId)
    return [QcOwnedProcess.BuildJob]::CreateAdmittedChildJob($Name, $ProcessId)
}

function Write-QcBuildProcessOwner {
    param([Parameter(Mandatory = $true)][string]$Path, [int]$AdmissionTimeoutSeconds = 30)
    if ($AdmissionTimeoutSeconds -le 0) { throw 'Build admission timeout must be positive.' }
    $key = [IO.Path]::GetFullPath($Path)
    if ($script:QCBuildChildJobs.ContainsKey($key)) { throw 'This child already owns a build admission Job for this record.' }
    if ($script:QCBuildChildJobs.Count -ne 0) { throw 'This child host already owns its one build admission Job.' }
    $name = 'Local\QC-build-owner-' + $PID + '-' + [guid]::NewGuid().ToString('N')
    $job = New-QcBuildAdmissionJob -Name $name -ProcessId $PID
    # Retain before publishing or waiting. The child also holds a Job handle:
    # kill-on-close applies only to the LAST handle. Parent cleanup must
    # explicitly terminate and acknowledge the original members.
    $script:QCBuildChildJobs[$key] = $job
    $identity = $job.Root
    $directory = Split-Path -Parent $key
    if ($directory) { New-Item -ItemType Directory -Force -Path $directory | Out-Null }
    $document = [ordered]@{
        kind = 'qc-build-owner-admission'
        schemaVersion = 2
        ownerFile = $key
        processId = $identity.processId
        parentProcessId = $identity.parentProcessId
        createdAtFileTime = [string]$identity.createdAtFileTime
        imagePath = $identity.imagePath
        startedAtUtc = [datetime]::FromFileTimeUtc($identity.createdAtFileTime).ToString('o')
        jobName = $name
    }
    $bytes = [Text.Encoding]::UTF8.GetBytes(($document | ConvertTo-Json -Compress))
    if ($bytes.Length -gt 8192) { throw 'Build admission metadata exceeded its 8 KiB software envelope.' }
    # Original Job IPC, not only a mutable filesystem record, binds the native
    # identity to the exact Start-Job object retained by this parent.
    Write-Output ([pscustomobject]$document)
    $stream = [IO.File]::Open($key, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
    } finally { $stream.Dispose() }
    $deadline = [datetime]::UtcNow.AddSeconds($AdmissionTimeoutSeconds)
    while ([datetime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath "$key.cancel" -PathType Leaf) {
            throw 'Build command admission cancelled before dispatch.'
        }
        if (Test-Path -LiteralPath "$key.admitted" -PathType Leaf) {
            try {
                $ack = Read-QcBuildOwnerDocument "$key.admitted"
                if ([string]$ack.jobName -eq $name -and [int]$ack.parentProcessId -eq $identity.parentProcessId) { return }
            } catch { }
        }
        Start-Sleep -Milliseconds 25
    }
    # No build command has been dispatched. Keep the original until the child
    # host exits; caller receives failure, never an unowned dispatch permit.
    throw "Build dispatch refused: parent did not retain its original Job before the admission deadline. Owner record: $key"
}

function Reserve-QcBuildProcessOwner {
    param([Parameter(Mandatory = $true)][string]$OwnerFile)
    [QcOwnedProcess.BuildJob]::Reserve([IO.Path]::GetFullPath($OwnerFile))
    Write-Host "Build owner slot reserved before startup: $OwnerFile"
}

function Register-QcBuildPowerShellJob {
    param([Parameter(Mandatory = $true)][string]$OwnerFile, [Parameter(Mandatory = $true)]$Job)
    [QcOwnedProcess.BuildJob]::AttachScriptJob([IO.Path]::GetFullPath($OwnerFile), $Job)
}

function Mark-QcBuildDispatchStarted {
    param([Parameter(Mandatory = $true)][string]$OwnerFile)
    [QcOwnedProcess.BuildJob]::MarkDispatchStarted([IO.Path]::GetFullPath($OwnerFile))
}
function Release-QcUndispatchedBuildReservation {
    param([Parameter(Mandatory = $true)][string]$OwnerFile)
    [QcOwnedProcess.BuildJob]::ReleaseUndispatched([IO.Path]::GetFullPath($OwnerFile))
}

function Capture-QcBuildProcessOwner {
    param([Parameter(Mandatory = $true)][string]$OwnerFile, [int]$AdmissionTimeoutSeconds = 30,
          [scriptblock]$BeforeAcknowledge)
    $key = [IO.Path]::GetFullPath($OwnerFile)
    if ($null -ne [QcOwnedProcess.BuildJob]::Find($key)) { throw 'This original native Job is already captured; repeated admission cannot imply a missing acknowledgment succeeded.' }
    $originalJob = [QcOwnedProcess.BuildJob]::OriginalScriptJob($key)
    if ($null -eq $originalJob) { throw 'The original PowerShell Job was not retained before native admission.' }
    $deadline = [datetime]::UtcNow.AddSeconds($AdmissionTimeoutSeconds)
    $document = $null
    while ([datetime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $key -PathType Leaf) {
            try {
                $candidate = Read-QcBuildOwnerDocument $key
                $frames = @($originalJob.ChildJobs[0].Output | Where-Object {
                    $null -ne $_.PSObject.Properties['kind'] -and [string]$_.kind -eq 'qc-build-owner-admission'
                })
                if ($frames.Count -gt 1) { throw 'Original build Job emitted duplicate admission identities.' }
                if ($frames.Count -eq 1) {
                    $frame = $frames[0]
                    foreach ($field in @('schemaVersion', 'processId', 'parentProcessId', 'createdAtFileTime', 'imagePath', 'jobName', 'ownerFile')) {
                        if ($null -eq $frame.PSObject.Properties[$field] -or $null -eq $candidate.PSObject.Properties[$field] -or [string]$frame.$field -cne [string]$candidate.$field) { throw 'Build owner file differs from its original Job IPC identity.' }
                    }
                    if (-not [string]::Equals([string]$frame.ownerFile, $key, [StringComparison]::OrdinalIgnoreCase)) { throw 'Original Job admission belongs to another owner path.' }
                    $document = $candidate
                    break
                }
            } catch { }
        }
        Start-Sleep -Milliseconds 25
    }
    if ($null -eq $document -or [int]$document.schemaVersion -ne 2) { throw "Build Job admission record missing or unsupported: $key" }
    if ([int]$document.parentProcessId -ne $PID) { throw 'Build owner is not an original direct child of this admitting process.' }
    [QcOwnedProcess.BuildJob]::CaptureAndRetain($key, [string]$document.jobName, [int]$document.processId, [long]$document.createdAtFileTime, [int]$document.parentProcessId, [string]$document.imagePath)
    # Retain before acknowledgment, including a failure publishing the ack.
    $owner = [QcOwnedProcess.BuildJob]::Find($key)
    # An enclosing supervisor may retain this same original Job before the
    # worker is allowed to launch its command. Failure keeps dispatch closed
    # and leaves the original admission available for its owner's cleanup.
    if ($null -ne $BeforeAcknowledge) { & $BeforeAcknowledge $owner }
    $stream = [IO.File]::Open("$key.admitted", [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes((@{ jobName = $owner.Name; parentProcessId = $PID } | ConvertTo-Json -Compress))
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
    } finally { $stream.Dispose() }
}

function ConvertTo-QcOwnerStartTimeUtc {
    param([Parameter(Mandatory = $true)][object]$StartedAtUtc)
    if ($StartedAtUtc -is [datetime]) { return $StartedAtUtc.ToUniversalTime() }
    return [datetime]::Parse([string]$StartedAtUtc, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind).ToUniversalTime()
}

function Invoke-QcBuildJobRetirement {
    param($Owner, [int]$TimeoutMilliseconds)
    $Owner.StopAndWait($TimeoutMilliseconds)
}

function Stop-QcOwnedProcessTree {
    param([Parameter(Mandatory = $true)][string]$OwnerFile, [int]$TimeoutSeconds = 30)
    $key = [IO.Path]::GetFullPath($OwnerFile)
    $owner = [QcOwnedProcess.BuildJob]::Find($key)
    if ($null -eq $owner) {
        Write-Warning "Build cleanup is unacknowledged: no retained original Job owns ${key}; legacy PID snapshots are not whole-tree proof. Record retained."
        return $false
    }
    try {
        if ($TimeoutSeconds -le 0 -or $TimeoutSeconds -gt 120) { throw 'Build cleanup deadline must be 1..120 seconds.' }
        if (-not $owner.Acknowledged) { Invoke-QcBuildJobRetirement -Owner $owner -TimeoutMilliseconds ($TimeoutSeconds * 1000) }
        if (-not $owner.Acknowledged) { throw 'Original build Job did not acknowledge termination.' }
        # Keep the process-global proof even after handles close, until its
        # owned record bytes retire too. A failed close leaves records intact.
        $owner.Dispose()
        foreach ($path in @("$key.admitted", $key)) {
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force -ErrorAction Stop }
        }
        # Native exit/bytes are acknowledged, but the original PowerShell Job
        # and its admission slot remain until its repository object retires.
        return $true
    } catch {
        Write-Warning "Build cleanup not fully acknowledged; ownership or cleanup attribution retained for ${key}: $($_.Exception.Message)"
        return $false
    }
}

function Remove-QcBuildPowerShellJob {
    param([Parameter(Mandatory = $true)][string]$OwnerFile)
    $key = [IO.Path]::GetFullPath($OwnerFile)
    $owner = [QcOwnedProcess.BuildJob]::Find($key)
    $original = [QcOwnedProcess.BuildJob]::OriginalScriptJob($key)
    if ($null -eq $owner -or $null -eq $original -or -not $owner.Acknowledged) {
        throw "Original native and PowerShell retirement is not proven for $key."
    }
    if ([QcOwnedProcess.BuildJob]::IsScriptJobRemoved($key)) { return }
    if ($original.State -notin @('Completed','Failed','Stopped')) {
        throw "Original PowerShell Job $($original.Id) is nonterminal; repository removal refused."
    }
    # Remove only the object we admitted. Get-Job enumerates unrelated source
    # adapters (including ScheduledJob persistence) and is not our authority.
    # No Force, no nonterminal removal, no success latch on a throwing remove.
    Remove-Job -Job $original -ErrorAction Stop
    [QcOwnedProcess.BuildJob]::AcknowledgeScriptJobRemoval($key, $original)
}

function Complete-QcBuildPowerShellJobRetirement {
    param([Parameter(Mandatory = $true)][string]$OwnerFile)
    $key = [IO.Path]::GetFullPath($OwnerFile)
    $owner = [QcOwnedProcess.BuildJob]::Find($key)
    $original = [QcOwnedProcess.BuildJob]::OriginalScriptJob($key)
    if ($null -eq $owner -or $null -eq $original -or -not $owner.Acknowledged) {
        throw "Original native and PowerShell retirement is not proven for $key."
    }
    if ($original.State -notin @('Completed','Failed','Stopped')) {
        throw "Original PowerShell Job $($original.Id) has not reached a terminal state; attribution retained at $key."
    }
    if (-not [QcOwnedProcess.BuildJob]::IsScriptJobRemoved($key)) {
        throw "Exact original PowerShell repository removal is not acknowledged; attribution retained at $key."
    }
    [QcOwnedProcess.BuildJob]::Retire($key, $owner)
}

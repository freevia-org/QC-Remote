# Generic Windows producer host. PowerShell7 is a validation prerequisite only.
# No private release policy, repository layout, credentials or evidence inputs.
param()
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'Standalone Windows native validation requires PowerShell 7 or newer before producer dispatch.' }

# One sole raw pipe reader. Console.In is SyncTextReader: its Async method
# can synchronously block before returning a Task, so it must not gate ready.
# PeekNamedPipe plus reads <= observed availability needs no input worker or
# background Task. The same original reader handles request, go and cancel.
if (-not ('QcOwnedProducer.PipeInput' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
namespace QcOwnedProducer {
    public sealed class PipeInput : IDisposable {
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern IntPtr GetStdHandle(int kind);
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern bool PeekNamedPipe(IntPtr pipe, IntPtr buffer, uint size,
            IntPtr read, out uint available, IntPtr left);
        private readonly IntPtr handle;
        private readonly Stream stream;
        private readonly MemoryStream pending = new MemoryStream();
        private readonly byte[] buffer = new byte[4096];
        private readonly UTF8Encoding utf8 = new UTF8Encoding(false, true);
        private int offset;
        private int length;
        private bool eof;
        public PipeInput() {
            handle = GetStdHandle(-10);
            if (handle == IntPtr.Zero || handle == new IntPtr(-1)) {
                throw new Win32Exception(Marshal.GetLastWin32Error());
            }
            stream = Console.OpenStandardInput();
        }
        public bool TryLine(int maximum, out string line, out bool ended) {
            line = null;
            ended = false;
            while (true) {
                if (offset == length) {
                    offset = length = 0;
                    if (!eof) {
                        uint available;
                        if (!PeekNamedPipe(handle, IntPtr.Zero, 0, IntPtr.Zero, out available, IntPtr.Zero)) {
                            int error = Marshal.GetLastWin32Error();
                            if (error == 109 || error == 233) eof = true;
                            else throw new Win32Exception(error);
                        } else if (available != 0) {
                            // This is the sole reader; no competing consumer can
                            // remove the peeked bytes before the synchronous read.
                            length = stream.Read(buffer, 0, (int)Math.Min(available, (uint)buffer.Length));
                            if (length == 0) eof = true;
                        }
                    }
                    if (length == 0) {
                        if (eof) {
                            if (pending.Length != 0) throw new InvalidDataException("Truncated producer protocol line.");
                            ended = true;
                        }
                        return false;
                    }
                }
                byte value = buffer[offset++];
                if (value == 10) {
                    line = utf8.GetString(pending.ToArray()).TrimEnd('\r');
                    pending.SetLength(0);
                    pending.Position = 0;
                    return true;
                }
                if (pending.Length >= maximum) throw new InvalidDataException("Producer protocol line exceeds its software byte envelope.");
                pending.WriteByte(value);
            }
        }
        public void Dispose() {
            stream.Dispose();
            pending.Dispose();
        }
    }
}
'@
}
$protocolInput = [QcOwnedProducer.PipeInput]::new()
$retainedLease = $null
try {
$requestText = $null
$ended = $false
$bootstrapClock = [Diagnostics.Stopwatch]::StartNew()
while (-not $protocolInput.TryLine(131072, [ref]$requestText, [ref]$ended)) {
    if ($ended) { throw 'Producer request pipe closed before dispatch.' }
    if ($bootstrapClock.ElapsedMilliseconds -ge 45000) { throw 'Producer request deadline expired before dispatch.' }
    Start-Sleep -Milliseconds 10
}
# JsonDocument preserves literal argv/environment strings (including ISO dates)
# rather than PowerShell's automatic DateTime conversion.
$document = [Text.Json.JsonDocument]::Parse($requestText)
try {
    $value = $document.RootElement
    $arguments = $value.GetProperty('arguments')
    if ($arguments.ValueKind -ne [Text.Json.JsonValueKind]::Array -or $arguments.GetArrayLength() -gt 4096) { throw 'Invalid producer argv.' }
    $argv = @($arguments.EnumerateArray() | ForEach-Object { $_.GetString() })
    $environment = [ordered]@{}
    foreach ($property in $value.GetProperty('environment').EnumerateObject()) {
        if ($property.Value.ValueKind -eq [Text.Json.JsonValueKind]::Null) { $environment[$property.Name] = $null }
        else { $environment[$property.Name] = $property.Value.GetString() }
    }
    $policyElement = [Text.Json.JsonElement]::new()
    $completionPolicy = if ($value.TryGetProperty('completionPolicy', [ref]$policyElement)) { $policyElement.GetString() } else { 'bounded' }
    $observerElement = [Text.Json.JsonElement]::new()
    $observeOriginalJob = if ($value.TryGetProperty('observeOriginalJob', [ref]$observerElement)) { $observerElement.GetBoolean() } else { $false }
    $directoryElement = [Text.Json.JsonElement]::new()
    $createOwnerDirectory = if ($value.TryGetProperty('createOwnerDirectory', [ref]$directoryElement)) { $directoryElement.GetBoolean() } else { $false }
    $stateElement = [Text.Json.JsonElement]::new()
    $createNativeTestState = if ($value.TryGetProperty('createNativeTestState', [ref]$stateElement)) { $stateElement.GetBoolean() } else { $false }
    if ($createNativeTestState -and -not $createOwnerDirectory) { throw 'Native test state requires host-created staging.' }
    $leaseElement = [Text.Json.JsonElement]::new()
    $originalLease = if ($value.TryGetProperty('originalLease', [ref]$leaseElement)) {
        [pscustomobject]@{processId=$leaseElement.GetProperty('processId').GetInt32();
            createdAtFileTime=$leaseElement.GetProperty('createdAtFileTime').GetString();
            handleValue=$leaseElement.GetProperty('handleValue').GetString();path=$leaseElement.GetProperty('path').GetString()}
    } else { $null }
    $request = [pscustomobject]@{
        schemaVersion=$value.GetProperty('schemaVersion').GetInt32()
        id=$value.GetProperty('id').GetString()
        command=$value.GetProperty('command').GetString()
        arguments=$argv
        cwd=$value.GetProperty('cwd').GetString()
        environment=[pscustomobject]$environment
        ownerDirectory=$value.GetProperty('ownerDirectory').GetString()
        completionPolicy=$completionPolicy
        observeOriginalJob=$observeOriginalJob
        createOwnerDirectory=$createOwnerDirectory
        createNativeTestState=$createNativeTestState
        originalLease=$originalLease
        timeoutMilliseconds=$(if ($value.GetProperty('timeoutMilliseconds').ValueKind -ne [Text.Json.JsonValueKind]::Null) { $value.GetProperty('timeoutMilliseconds').GetInt64() } else { $null })
        maxOutputBytes=$(if ($value.GetProperty('maxOutputBytes').ValueKind -ne [Text.Json.JsonValueKind]::Null) { $value.GetProperty('maxOutputBytes').GetInt64() } else { $null })
    }
} finally { $document.Dispose() }
if ($request.schemaVersion -ne 1 -or $request.id -notmatch '^[A-Za-z0-9_-]{1,128}$' -or
    [string]::IsNullOrWhiteSpace([string]$request.command) -or
    -not [IO.Path]::IsPathRooted([string]$request.cwd) -or
    -not [IO.Path]::IsPathRooted([string]$request.ownerDirectory)) { throw 'Invalid generic producer request.' }
if ($request.arguments -isnot [Array] -or $request.arguments.Count -gt 4096) { throw 'Invalid producer argv.' }
foreach ($argument in $request.arguments) { if ($argument -isnot [string] -or $argument.Contains([char]0)) { throw 'Invalid producer argument.' } }
$timeoutMs = [long]$request.timeoutMilliseconds
$outputLimit = [long]$request.maxOutputBytes
$followsCaller = $request.completionPolicy -ceq 'caller-lifetime'
if ($request.completionPolicy -cnotin @('bounded','caller-lifetime')) { throw 'Invalid producer completion policy.' }
if ($followsCaller) {
    if ($null -ne $request.timeoutMilliseconds -or $null -ne $request.maxOutputBytes) { throw 'Caller-lifetime producers cannot declare bounded execution/output limits.' }
} elseif ($timeoutMs -le 0 -or $timeoutMs -gt 7200000 -or $outputLimit -le 0 -or $outputLimit -gt 67108864) { throw 'Invalid producer software budget.' }

function Write-ProducerFrame($Value) {
    [Console]::Out.WriteLine(($Value | ConvertTo-Json -Depth 8 -Compress))
    [Console]::Out.Flush()
}
$library = Join-Path $PSScriptRoot 'windows-build-process-cleanup.ps1'
. $library
if ($null -ne $request.originalLease) {
    $lease = $request.originalLease
    $retainedLease = [QcOwnedProcess.Native]::DuplicateParentLease([int]$lease.processId,
        [long]::Parse($lease.createdAtFileTime, [Globalization.CultureInfo]::InvariantCulture),
        [long]::Parse($lease.handleValue, [Globalization.CultureInfo]::InvariantCulture), [string]$lease.path)
}
$owner = Join-Path ([string]$request.ownerDirectory) 'producer-owner.json'
$job = $null
$reserved = $false
$marked = $false
$acknowledged = $false
$exitCode = $null
$failure = $null
$producerDirectoryCreated = $false
$originalsAcknowledged = $false
$clock = [Diagnostics.Stopwatch]::StartNew()
Write-ProducerFrame @{schemaVersion=1;id=$request.id;kind='ready';leaseRetained=($null -ne $retainedLease)}
# Until the retained Node caller explicitly sends go, no native child can start.
$go = $null
$ended = $false
$goClock = [Diagnostics.Stopwatch]::StartNew()
while (-not $protocolInput.TryLine(64, [ref]$go, [ref]$ended)) {
    if ($ended) { throw 'Producer bootstrap input closed before dispatch.' }
    if ($goClock.ElapsedMilliseconds -ge 30000) { throw 'Producer bootstrap admission timed out before dispatch.' }
    Start-Sleep -Milliseconds 10
}
if ($go -cne 'go') { throw 'Producer bootstrap admission refused before dispatch.' }
function Forward-ProducerOutput($Original) {
    foreach ($frame in @(Receive-Job -Job $Original -ErrorAction Stop)) {
        if ($frame.kind -eq 'stdout' -or $frame.kind -eq 'stderr') {
            Write-ProducerFrame @{schemaVersion=1;id=$request.id;kind=$frame.kind;text=$frame.text}
        } elseif ($frame.kind -eq 'producer-exit') { Set-Variable -Name exitCode -Value ([int]$frame.code) -Scope 1 }
        elseif ($frame.kind -eq 'producer-error') { Set-Variable -Name failure -Value ([string]$frame.message) -Scope 1 }
    }
}
function Request-ProducerPowerShellStop {
    # This is a signal to OUR admitted child script, not native exit proof.
    # Its exclusive owner directory remains retained until the final ACK.
    $cancelPath = "$owner.cancel"
    if (-not [IO.File]::Exists($cancelPath)) {
        $signal = [IO.File]::Open($cancelPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        try { $signal.Flush() } finally { $signal.Dispose() }
    }
}
function Complete-ProducerRetirement {
    $retirementClock = [Diagnostics.Stopwatch]::StartNew()
    $cooperativeFailure = $null
    if ($null -ne $job -and $job.State -notin @('Completed','Failed','Stopped')) {
        try {
            # Let the child abandon pending inherited output reads and finish
            # its own script BEFORE killing its remoting transport. The native
            # Job and exact repository object remain retained throughout.
            Request-ProducerPowerShellStop
            Wait-Job -Job $job -Timeout 3 | Out-Null
        } catch { $cooperativeFailure = [string]$_.Exception.Message }
    }
    $retirementWarnings = @()
    if (-not (Stop-QcOwnedProcessTree -OwnerFile $owner -TimeoutSeconds 3 -WarningVariable retirementWarnings 3>$null)) {
        throw ('Native producer retirement was not acknowledged. ' + ($retirementWarnings -join ' '))
    }
    if ($null -eq $job) { throw 'Original PowerShell producer is unavailable; admission retained.' }
    # One six-second observation budget for cooperation/native retirement/
    # original settlement, not a fresh three-second wait after every phase.
    $waitSeconds = if ($job.State -in @('Completed','Failed','Stopped')) { 1 } else {
        [Math]::Min(3, [Math]::Floor((6000 - $retirementClock.ElapsedMilliseconds) / 1000))
    }
    if ($waitSeconds -le 0 -or $null -eq (Wait-Job -Job $job -Timeout ([int]$waitSeconds))) {
        throw "Original PowerShell producer did not settle; attribution retained. Cooperative signal: $cooperativeFailure"
    }
    # Retry may follow a failed Complete after successful removal. Never
    # manufacture an affirmative ACK from native termination alone.
    Remove-QcBuildPowerShellJob -OwnerFile $owner
    Complete-QcBuildPowerShellJobRetirement -OwnerFile $owner
}
function Remove-ProducerCreatedDirectory {
    if (-not $producerDirectoryCreated) { return }
    # Only this host's exclusive creation is deletion authority. An ordinary
    # caller-provided existing directory never enters this path.
    $path = [IO.Path]::GetFullPath([string]$request.ownerDirectory)
    if (Test-Path -LiteralPath $path) {
        $ancestor = $path
        while ($ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction Stop
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Producer staging ancestry changed before retirement.' }
            $ancestor = [IO.Path]::GetDirectoryName($ancestor)
        }
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
    }
}
try {
    if ($request.createOwnerDirectory) {
        $path = [IO.Path]::GetFullPath([string]$request.ownerDirectory)
        if (-not [IO.Path]::IsPathFullyQualified([string]$request.ownerDirectory)) { throw 'Producer-created staging must be fully qualified.' }
        $ancestor = [IO.Path]::GetDirectoryName($path)
        while ($ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction Stop
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Producer-created staging requires plain existing ancestors.' }
            $ancestor = [IO.Path]::GetDirectoryName($ancestor)
        }
        New-Item -ItemType Directory -Path $path -ErrorAction Stop | Out-Null
        $producerDirectoryCreated = $true
        if ($request.createNativeTestState) {
            $testState = Join-Path $path 'native-test-state'
            New-Item -ItemType Directory -Path $testState -ErrorAction Stop | Out-Null
            foreach ($property in @($request.environment.PSObject.Properties)) {
                if ($property.Name -ieq 'QC_REMOTE_TEST_STATE_ROOT') { $request.environment.PSObject.Properties.Remove($property.Name) }
            }
            $request.environment | Add-Member -NotePropertyName QC_REMOTE_TEST_STATE_ROOT -NotePropertyValue $testState
        }
    }
    Reserve-QcBuildProcessOwner $owner 6>$null | Out-Null
    $reserved = $true
    Mark-QcBuildDispatchStarted $owner
    $marked = $true
    $job = Start-Job -ScriptBlock {
        param($Library, $Owner, $Request)
        $ErrorActionPreference = 'Stop'
        . $Library
        # Preserve the admission IPC frame for capture before launching anything.
        Write-QcBuildProcessOwner -Path $Owner -AdmissionTimeoutSeconds 30
        $process = $null
        try {
            $info = [Diagnostics.ProcessStartInfo]::new()
            $info.FileName = [string]$Request.command
            $info.WorkingDirectory = [string]$Request.cwd
            $info.UseShellExecute = $false
            $info.CreateNoWindow = $true
            $info.RedirectStandardOutput = $true
            $info.RedirectStandardError = $true
            # The command specification has no stdin payload. The original
            # host's input belongs only to go/cancel protocol, never the child.
            $info.RedirectStandardInput = $true
            foreach ($argument in $Request.arguments) { [void]$info.ArgumentList.Add([string]$argument) }
            $info.Environment.Clear()
            foreach ($property in $Request.environment.PSObject.Properties) {
                if ($null -ne $property.Value) { $info.Environment[$property.Name] = [string]$property.Value }
            }
            # Executable lookup uses this private child host's PATH too.
            if ($null -ne $Request.environment.PSObject.Properties['PATH']) { $env:PATH = [string]$Request.environment.PATH }
            $resolved = Get-Command $info.FileName -CommandType Application -ErrorAction Stop | Select-Object -First 1
            $info.FileName = [string]$resolved.Source
            $process = [Diagnostics.Process]::Start($info)
            if ($null -eq $process) { throw 'Producer could not start.' }
            $process.StandardInput.Close()
            $readers = @($process.StandardOutput, $process.StandardError)
            $buffers = @([char[]]::new(4096), [char[]]::new(4096))
            $pending = @($readers[0].ReadAsync($buffers[0],0,4096), $readers[1].ReadAsync($buffers[1],0,4096))
            $done = @($false,$false)
            $used = @([long]0,[long]0)
            $watch = [Diagnostics.Stopwatch]::StartNew()
            $exitDrain = $null
            while (-not ($done[0] -and $done[1] -and $process.HasExited)) {
                # Do not await a pending read held open by a grandchild after
                # cancellation. Parent still owns and retires the native Job.
                if ([IO.File]::Exists("$Owner.cancel")) { throw 'Producer child acknowledged cooperative cancellation.' }
                if ($Request.completionPolicy -ne 'caller-lifetime' -and $watch.ElapsedMilliseconds -ge [long]$Request.timeoutMilliseconds) { throw 'Producer exceeded its execution/output deadline.' }
                # A root that has exited cannot legitimately keep this caller
                # waiting forever for output inherited by a surviving child.
                if ($process.HasExited -and $null -eq $exitDrain) { $exitDrain = [Diagnostics.Stopwatch]::StartNew() }
                if ($null -ne $exitDrain -and $exitDrain.ElapsedMilliseconds -ge 5000) { throw "Producer exited with code $($process.ExitCode), but inherited output remained open after five seconds." }
                for ($index=0; $index -lt 2; $index++) {
                    if ($done[$index] -or -not $pending[$index].IsCompleted) { continue }
                    $count = $pending[$index].GetAwaiter().GetResult()
                    if ($count -eq 0) { $done[$index] = $true; continue }
                    $text = [string]::new($buffers[$index],0,$count)
                    $used[$index] += [Text.Encoding]::UTF8.GetByteCount($text)
                    if ($Request.completionPolicy -ne 'caller-lifetime' -and $used[$index] -gt [long]$Request.maxOutputBytes) { throw 'Producer exceeded its software output envelope.' }
                    [pscustomobject]@{ kind=(@('stdout','stderr')[$index]); text=$text }
                    $pending[$index] = $readers[$index].ReadAsync($buffers[$index],0,4096)
                }
                Start-Sleep -Milliseconds 10
            }
            [pscustomobject]@{kind='producer-exit';code=$process.ExitCode}
        } catch {
            [pscustomobject]@{kind='producer-error';message=([string]$_.Exception.Message).Substring(0,[Math]::Min(2048,([string]$_.Exception.Message).Length))}
        } finally {
            if ($null -ne $process) { $process.Dispose() }
            # Parent owns native Job retirement even when the primary has exited.
        }
    } -ArgumentList $library,$owner,$request
    Register-QcBuildPowerShellJob -OwnerFile $owner -Job $job
    $beforeAcknowledgment = $null
    if ($request.observeOriginalJob) {
        $beforeAcknowledgment = {
            param($Original)
            Write-ProducerFrame @{schemaVersion=1;id=$request.id;kind='ownership';
                jobName=$Original.Name;processId=$Original.Root.processId;
                createdAtFileTime=[string]$Original.Root.createdAtFileTime;
                parentProcessId=$Original.Root.parentProcessId;imagePath=$Original.Root.imagePath}
            $observerClock = [Diagnostics.Stopwatch]::StartNew()
            $observerLine = $null
            $observerEnded = $false
            while (-not $protocolInput.TryLine(64, [ref]$observerLine, [ref]$observerEnded)) {
                if ($observerEnded) { throw 'Original Job observer closed before command admission.' }
                if ($observerClock.ElapsedMilliseconds -ge 15000) { throw 'Original Job observer did not acknowledge command admission.' }
                Start-Sleep -Milliseconds 10
            }
            if ($observerLine -cne 'owned') { throw 'Original Job observer refused command admission.' }
        }
    }
    Capture-QcBuildProcessOwner -OwnerFile $owner -AdmissionTimeoutSeconds 30 -BeforeAcknowledge $beforeAcknowledgment
    while ($true) {
        $control = $null
        $ended = $false
        if ($protocolInput.TryLine(64, [ref]$control, [ref]$ended) -or $ended) {
            throw 'Producer cancelled by its original parent or input EOF.'
        }
        if (-not $followsCaller -and $clock.ElapsedMilliseconds -ge $timeoutMs) { throw 'Producer host deadline expired.' }
        Forward-ProducerOutput $job
        if ($job.State -in @('Completed','Failed','Stopped')) {
            # Drain again after terminal observation; state may have advanced
            # immediately after the preceding Receive-Job snapshot.
            Forward-ProducerOutput $job
            if ($null -eq $exitCode -and $null -eq $failure) { $failure = 'Producer ended without a complete exit result.' }
            break
        }
        Start-Sleep -Milliseconds 10
    }
} catch { $failure = [string]$_.Exception.Message }
finally {
    if ($reserved -and -not $marked) {
        Release-QcUndispatchedBuildReservation $owner
        $acknowledged = $true
    } elseif ($marked) {
        try {
            Complete-ProducerRetirement
            $acknowledged = $true
        } catch {
            $failure = @($failure, [string]$_.Exception.Message) -join ' Cleanup: '
        }
    } else { $acknowledged = $true }
}
$originalsAcknowledged = $acknowledged
if ($acknowledged) {
    try { Remove-ProducerCreatedDirectory } catch { $acknowledged = $false; $failure = @($failure, [string]$_.Exception.Message) -join ' Cleanup: ' }
}
try { Write-ProducerFrame @{schemaVersion=1;id=$request.id;kind='retirement';acknowledged=$acknowledged;exitCode=$exitCode;error=$failure;ownerDirectory=$request.ownerDirectory} } catch {
    # Broken parent output cannot skip the original retirement retry below.
}
if (-not $acknowledged) {
    # Keep this exact admitting process and its original registry alive. The
    # Node caller retains it and the directory, bounded by its own admission.
    # No parent-death/last-handle kill certification or hard kernel deadline.
    while ($true) {
        Start-Sleep -Milliseconds 250
        try {
            if (-not $originalsAcknowledged) { Complete-ProducerRetirement; $originalsAcknowledged = $true }
            Remove-ProducerCreatedDirectory
            try { Write-ProducerFrame @{schemaVersion=1;id=$request.id;kind='retirement';acknowledged=$true;exitCode=$exitCode;error=$failure;ownerDirectory=$request.ownerDirectory} } catch {}
            break
        } catch { }
    }
}
if ($null -ne $failure -or $exitCode -ne 0) { exit 1 }

} finally {
    try { if ($null -ne $retainedLease) { $retainedLease.Dispose() } }
    finally { $protocolInput.Dispose() }
}

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$HelperPath
)

$ErrorActionPreference = 'Stop'
& $HelperPath -Action Inspect -ProcessId $PID -LoadOnly

function Assert-Equal($Expected, $Actual, [string]$Message) {
    if ($Expected -cne $Actual) {
        throw "$Message (expected '$Expected'; got '$Actual')"
    }
}

function Assert-Throws([scriptblock]$Action, [string]$Message) {
    $threw = $false
    try { & $Action }
    catch [ArgumentException] { $threw = $true }
    if (-not $threw) { throw $Message }
}

$name = 'QC_REMOTE_ENV_EXACT_TEST_' + [Guid]::NewGuid().ToString('N')
$oldValue = [QcOwnedProcess.Native]::ReadProcessEnvironmentExact($name)
if ($null -ne $oldValue) { throw 'Generated environment test name unexpectedly exists.' }

try {
    [QcOwnedProcess.Native]::SetProcessEnvironmentExact($name, $true, '')
    Assert-Equal '' ([QcOwnedProcess.Native]::ReadProcessEnvironmentExact($name)) 'Present-empty value was not preserved'

    [QcOwnedProcess.Native]::SetProcessEnvironmentExact($name, $true, 'value with spaces & symbols=%')
    Assert-Equal 'value with spaces & symbols=%' ([QcOwnedProcess.Native]::ReadProcessEnvironmentExact($name)) 'Non-empty value was not preserved exactly'

    [QcOwnedProcess.Native]::SetProcessEnvironmentExact($name, $false, $null)
    if ($null -ne [QcOwnedProcess.Native]::ReadProcessEnvironmentExact($name)) {
        throw 'Removed environment variable was not reported as absent.'
    }

    Assert-Throws { [QcOwnedProcess.Native]::ReadProcessEnvironmentExact('') } 'Empty name was accepted.'
    Assert-Throws { [QcOwnedProcess.Native]::ReadProcessEnvironmentExact('bad=name') } "Name containing '=' was accepted."
    Assert-Throws { [QcOwnedProcess.Native]::SetProcessEnvironmentExact('bad=name', $true, 'x') } "Setter accepted name containing '='."
    Assert-Throws { [QcOwnedProcess.Native]::SetProcessEnvironmentExact($name, $true, "bad`0value") } 'Value containing NUL was accepted.'
} finally {
    [QcOwnedProcess.Native]::SetProcessEnvironmentExact($name, ($null -ne $oldValue), $oldValue)
}

Write-Output 'Exact process environment read/write tests passed.'

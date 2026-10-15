param([string]$AdmissionFile,[int]$ExpectedParentProcessId,[string]$ExpectedParentImagePath)
$ErrorActionPreference='Stop'
function Assert-QcOriginalJobAdmission {
    param([Parameter(Mandatory=$true)][string]$Path,[int]$ParentProcessId,[string]$ParentImagePath)
    & (Join-Path $PSScriptRoot 'windows-build-process-owner.ps1') -Action Inspect -ProcessId $PID -LoadOnly
    # Canonical absolute form is also available under Windows PowerShell 5's
    # .NET Framework, unlike Path.IsPathFullyQualified. Drive-relative/root-
    # relative spellings cannot compare equal to their fully resolved path.
    if(-not [IO.Path]::IsPathRooted($Path) -or -not [string]::Equals([IO.Path]::GetFullPath($Path),$Path,[StringComparison]::OrdinalIgnoreCase)){
        throw 'Original Job admission must be a canonical absolute file'
    }
    $stream=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
    try{
        if($stream.Length -gt 4096){throw 'Original Job admission exceeded its envelope'}
        $reader=[IO.StreamReader]::new($stream,[Text.UTF8Encoding]::new($false,$true))
        try{$admission=$reader.ReadToEnd()|ConvertFrom-Json -ErrorAction Stop}finally{$reader.Dispose()}
    }finally{$stream.Dispose()}
    if($admission.schemaVersion -ne 1 -or $admission.jobName -cnotmatch '^Local\\QC-build-owner-[1-9][0-9]*-[0-9a-f]{32}$'){
        throw 'Original Job admission is invalid'
    }
    $original=[QcOwnedProcess.BuildJob]::Open([string]$admission.jobName,[int]$admission.processId,
        [long]::Parse([string]$admission.createdAtFileTime,[Globalization.CultureInfo]::InvariantCulture),
        [int]$admission.parentProcessId,[string]$admission.imagePath)
    try{
        $original.ValidateMember($PID)
        if($ParentProcessId -gt 0){
            $self=[QcOwnedProcess.Native]::Inspect($PID)
            if($self.parentProcessId -ne $ParentProcessId){throw 'Admission checker is not a child of the supplied original worker'}
            $parent=[QcOwnedProcess.Native]::Inspect($ParentProcessId)
            if($parent.state -cne 'present' -or -not [string]::Equals($parent.imagePath,$ParentImagePath,[StringComparison]::OrdinalIgnoreCase)){
                throw 'Original worker image does not match its admission checker parent'
            }
            $original.ValidateMember($ParentProcessId)
        }
    }finally{$original.Dispose()}
}
if($MyInvocation.InvocationName -ne '.'){
    if([string]::IsNullOrWhiteSpace($AdmissionFile) -or $ExpectedParentProcessId -le 0 -or [string]::IsNullOrWhiteSpace($ExpectedParentImagePath)){
        throw 'Original admission file and exact worker parent are required'
    }
    Assert-QcOriginalJobAdmission -Path $AdmissionFile -ParentProcessId $ExpectedParentProcessId -ParentImagePath $ExpectedParentImagePath
}

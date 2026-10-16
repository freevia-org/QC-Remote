[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Inspect', 'Stop')][string]$Action,
    [Parameter(Mandatory = $true)][int]$ProcessId,
    [long]$ExpectedCreatedAtFileTime,
    [int]$ExpectedParentProcessId,
    [string]$ExpectedImagePath,
    [switch]$LoadOnly
)

$ErrorActionPreference = 'Stop'
if (-not ('QcOwnedProcess.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

namespace QcOwnedProcess {
    public sealed class Snapshot {
        public string state { get; set; }
        public int processId { get; set; }
        public int parentProcessId { get; set; }
        public long createdAtFileTime { get; set; }
        public string imagePath { get; set; }
    }

    public static class Native {
        const uint PROCESS_TERMINATE = 0x0001;
        const uint PROCESS_QUERY_INFORMATION = 0x0400;
        const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
        const int ProcessBasicInformation = 0;
        const uint WAIT_OBJECT_0 = 0;

        [StructLayout(LayoutKind.Sequential)]
        struct FILETIME { public uint Low; public uint High; }

        [StructLayout(LayoutKind.Sequential)]
        struct PROCESS_BASIC_INFORMATION {
            public IntPtr Reserved1;
            public IntPtr PebBaseAddress;
            public IntPtr Reserved2_0;
            public IntPtr Reserved2_1;
            public IntPtr UniqueProcessId;
            public IntPtr InheritedFromUniqueProcessId;
        }

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool CloseHandle(IntPtr handle);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool GetProcessTimes(IntPtr process, out FILETIME creation, out FILETIME exit, out FILETIME kernel, out FILETIME user);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern bool QueryFullProcessImageName(IntPtr process, int flags, StringBuilder path, ref int size);
        [DllImport("ntdll.dll")]
        static extern int NtQueryInformationProcess(IntPtr process, int infoClass, ref PROCESS_BASIC_INFORMATION info, int length, out int returned);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool TerminateProcess(IntPtr process, uint exitCode);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll", EntryPoint = "SetEnvironmentVariableW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern bool SetEnvironmentVariableExact(string name, string value);
        [DllImport("kernel32.dll", EntryPoint = "GetEnvironmentVariableW", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern uint GetEnvironmentVariableExact(string name, StringBuilder value, uint capacity);
        [DllImport("kernel32.dll", EntryPoint = "SetLastError", ExactSpelling = true)]
        static extern void ClearNativeLastError(uint error);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool DuplicateHandle(IntPtr sourceProcess, IntPtr source, IntPtr targetProcess,
            out IntPtr duplicate, uint desiredAccess, bool inherit, uint options);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint GetFileType(IntPtr file);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern uint GetFinalPathNameByHandle(IntPtr file, StringBuilder path, uint size, uint flags);

        static long FileTime(FILETIME value) { return ((long)value.High << 32) | value.Low; }
        static void Check(bool ok, string operation) {
            if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error(), operation);
        }

        // Retain the SAME exclusive open, not a later open by pathname. The
        // source must still be this host's original live parent, with its exact
        // creation identity. A recycled PID/handle or a different file refuses
        // before command admission. The duplicate is never inherited further.
        public static Microsoft.Win32.SafeHandles.SafeFileHandle DuplicateParentLease(
            int parentId, long parentCreated, long handleValue, string expectedPath) {
            var basic = new PROCESS_BASIC_INFORMATION(); int returned;
            int status = NtQueryInformationProcess(new IntPtr(-1), ProcessBasicInformation,
                ref basic, Marshal.SizeOf(typeof(PROCESS_BASIC_INFORMATION)), out returned);
            if (status != 0 || basic.InheritedFromUniqueProcessId.ToInt64() != parentId)
                throw new InvalidOperationException("Lease source is not this host's original parent.");
            IntPtr parent = OpenProcess(0x0040 | PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, false, parentId);
            if (parent == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcess(lease parent)");
            try {
                FILETIME created, exited, kernel, user;
                Check(GetProcessTimes(parent, out created, out exited, out kernel, out user), "GetProcessTimes(lease parent)");
                if (FileTime(created) != parentCreated || WaitForSingleObject(parent, 0) != 258)
                    throw new InvalidOperationException("Original lease parent changed or exited.");
                IntPtr duplicate;
                Check(DuplicateHandle(parent, new IntPtr(handleValue), new IntPtr(-1), out duplicate,
                    0, false, 2), "DuplicateHandle(original lease)");
                var safe = new Microsoft.Win32.SafeHandles.SafeFileHandle(duplicate, true);
                try {
                    if (GetFileType(duplicate) != 1) throw new InvalidOperationException("Lease is not a disk file.");
                    var path = new StringBuilder(32768);
                    uint used = GetFinalPathNameByHandle(duplicate, path, (uint)path.Capacity, 0);
                    if (used == 0 || used >= path.Capacity) throw new InvalidOperationException("Original lease file path unavailable.");
                    string actual = path.ToString();
                    if (actual.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)) actual = @"\\" + actual.Substring(8);
                    else if (actual.StartsWith(@"\\?\", StringComparison.Ordinal)) actual = actual.Substring(4);
                    if (!String.Equals(System.IO.Path.GetFullPath(actual), System.IO.Path.GetFullPath(expectedPath), StringComparison.OrdinalIgnoreCase))
                        throw new InvalidOperationException("Original lease handle names a different file.");
                    return safe;
                } catch { safe.Dispose(); throw; }
            } finally { CloseHandle(parent); }
        }

        // PS5/.NET Framework treats empty managed environment values as
        // deletion. Select native NULL in C#, not PowerShell's null->string
        // conversion, so absent and inherited present-empty stay distinct.
        public static void SetProcessEnvironmentExact(string name, bool present, string value) {
            if (String.IsNullOrEmpty(name) || name.IndexOf('=') >= 0 || name.IndexOf('\0') >= 0)
                throw new ArgumentException("Invalid process environment name.");
            if (present && (value == null || value.IndexOf('\0') >= 0))
                throw new ArgumentException("A present process environment value must be a string without NUL.");
            if (!SetEnvironmentVariableExact(name, present ? value : null)) {
                int error = Marshal.GetLastWin32Error();
                if (!present && error == 203) return; // Already absent is exact deletion.
                throw new Win32Exception(error, "SetEnvironmentVariableW");
            }
        }

        // Direct OS observation, independent of framework/provider environment
        // enumeration. NULL means absent; String.Empty means present-empty.
        // Win32's documented per-variable maximum is 32,767 characters.
        public static string ReadProcessEnvironmentExact(string name) {
            if (String.IsNullOrEmpty(name) || name.IndexOf('=') >= 0 || name.IndexOf('\0') >= 0)
                throw new ArgumentException("Invalid process environment name.");
            var value = new StringBuilder(32768);
            ClearNativeLastError(0);
            uint length = GetEnvironmentVariableExact(name, value, (uint)value.Capacity);
            int error = Marshal.GetLastWin32Error();
            if (length == 0) {
                if (error == 203) return null;
                if (error != 0) throw new Win32Exception(error, "GetEnvironmentVariableW");
                return String.Empty;
            }
            if (length >= value.Capacity) throw new InvalidOperationException("Process environment variable exceeds the Win32 per-variable envelope.");
            return value.ToString();
        }

        public static Snapshot Inspect(int processId) {
            IntPtr handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION, false, processId);
            if (handle == IntPtr.Zero) {
                int error = Marshal.GetLastWin32Error();
                if (error == 87) return new Snapshot { state = "missing", processId = processId };
                throw new Win32Exception(error, "OpenProcess(query)");
            }
            try {
                FILETIME created, exited, kernel, user;
                Check(GetProcessTimes(handle, out created, out exited, out kernel, out user), "GetProcessTimes");
                var name = new StringBuilder(32768);
                int capacity = name.Capacity;
                Check(QueryFullProcessImageName(handle, 0, name, ref capacity), "QueryFullProcessImageName");
                var basic = new PROCESS_BASIC_INFORMATION();
                int returned;
                int status = NtQueryInformationProcess(handle, ProcessBasicInformation, ref basic, Marshal.SizeOf(typeof(PROCESS_BASIC_INFORMATION)), out returned);
                if (status != 0) throw new InvalidOperationException("NtQueryInformationProcess failed with NTSTATUS 0x" + status.ToString("x8"));
                return new Snapshot {
                    state = "present",
                    processId = processId,
                    parentProcessId = unchecked((int)basic.InheritedFromUniqueProcessId.ToInt64()),
                    createdAtFileTime = FileTime(created),
                    imagePath = name.ToString()
                };
            } finally { CloseHandle(handle); }
        }

        public static void Stop(int processId, long expectedCreatedAtFileTime, int expectedParentProcessId, string expectedImagePath) {
            IntPtr handle = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, false, processId);
            if (handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcess(terminate)");
            try {
                FILETIME created, exited, kernel, user;
                Check(GetProcessTimes(handle, out created, out exited, out kernel, out user), "GetProcessTimes");
                var name = new StringBuilder(32768);
                int capacity = name.Capacity;
                Check(QueryFullProcessImageName(handle, 0, name, ref capacity), "QueryFullProcessImageName");
                var basic = new PROCESS_BASIC_INFORMATION();
                int returned;
                int status = NtQueryInformationProcess(handle, ProcessBasicInformation, ref basic, Marshal.SizeOf(typeof(PROCESS_BASIC_INFORMATION)), out returned);
                if (status != 0) throw new InvalidOperationException("NtQueryInformationProcess failed with NTSTATUS 0x" + status.ToString("x8"));
                int parent = unchecked((int)basic.InheritedFromUniqueProcessId.ToInt64());
                if (FileTime(created) != expectedCreatedAtFileTime || parent != expectedParentProcessId
                    || !String.Equals(name.ToString(), expectedImagePath, StringComparison.OrdinalIgnoreCase)) {
                    throw new InvalidOperationException("Process identity changed; refusing to stop it.");
                }
                Check(TerminateProcess(handle, 1), "TerminateProcess");
                if (WaitForSingleObject(handle, 5000) != WAIT_OBJECT_0)
                    throw new TimeoutException("Owned process did not exit within five seconds.");
            } finally { CloseHandle(handle); }
        }
    }
    // Build dispatch is gated until the parent owns this original Job and
    // process handle. Handles, not a later PID lookup, survive root exit.
    public sealed class BuildJob : IDisposable {
        [StructLayout(LayoutKind.Sequential)]
        struct BasicLimit {
            public long ProcessTime, JobTime;
            public uint Flags;
            public UIntPtr MinWorkingSet, MaxWorkingSet;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass, SchedulingClass;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct IoCounters { public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes; }
        [StructLayout(LayoutKind.Sequential)]
        struct ExtendedLimit {
            public BasicLimit Basic;
            public IoCounters Io;
            public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
        }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern IntPtr CreateJobObject(IntPtr attributes, string name);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern IntPtr OpenJobObject(uint access, bool inherit, string name);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool SetInformationJobObject(IntPtr job, int info, ref ExtendedLimit limits, int size);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool QueryInformationJobObject(IntPtr job, int info, IntPtr buffer, uint size, out uint returned);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool member);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool TerminateJobObject(IntPtr job, uint exitCode);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool CloseHandle(IntPtr handle);
        sealed class Member {
            public IntPtr Handle;
            public Snapshot Identity;
        }
        IntPtr job;
        readonly object lifecycleGate = new object();
        static readonly object RegistryGate = new object();
        static readonly System.Collections.Generic.Dictionary<string, BuildJob> Retained = new System.Collections.Generic.Dictionary<string, BuildJob>(StringComparer.OrdinalIgnoreCase);
        static readonly System.Collections.Generic.Dictionary<string, object> ScriptJobs = new System.Collections.Generic.Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
        static readonly System.Collections.Generic.HashSet<string> ScriptJobsRemoved = new System.Collections.Generic.HashSet<string>(StringComparer.OrdinalIgnoreCase);
        static readonly System.Collections.Generic.HashSet<string> Capturing = new System.Collections.Generic.HashSet<string>(StringComparer.OrdinalIgnoreCase);
        static readonly System.Collections.Generic.HashSet<string> DispatchStarted = new System.Collections.Generic.HashSet<string>(StringComparer.OrdinalIgnoreCase);
        static readonly object ChildGate = new object();
        static BuildJob childOriginal;
        readonly System.Collections.Generic.Dictionary<int, Member> members = new System.Collections.Generic.Dictionary<int, Member>();
        public Snapshot Root { get; private set; }
        public string Name { get; private set; }
        public bool Acknowledged { get; private set; }
        public static int RetainedCount { get { lock (RegistryGate) { return Retained.Count; } } }
        bool quiesced;
        public static BuildJob Find(string key) {
            lock (RegistryGate) { BuildJob owner; return Retained.TryGetValue(key, out owner) ? owner : null; }
        }
        public static bool HasAdmission(string key) { lock (RegistryGate) { return Retained.ContainsKey(key); } }
        public static object OriginalScriptJob(string key) {
            lock (RegistryGate) { object original; return ScriptJobs.TryGetValue(key, out original) ? original : null; }
        }
        public static void Reserve(string key) {
            lock (RegistryGate) {
                if (Retained.ContainsKey(key)) throw new InvalidOperationException("Build owner admission already exists or is in progress.");
                if (Retained.Count >= 16) throw new InvalidOperationException("Build owner admission headroom exhausted (16 originals).");
                Retained.Add(key, null); // BEFORE Start-Job, including unknown startup failures.
            }
        }
        public static void AttachScriptJob(string key, object original) {
            // A throwing Start-Job may already have created a native host.
            // Only an explicit BEFORE-call transition proves no dispatch.
            lock (RegistryGate) {
                if (!Retained.ContainsKey(key) || !DispatchStarted.Contains(key) || original == null) throw new InvalidOperationException("Build Job was not reserved and marked before startup.");
                if (ScriptJobs.ContainsKey(key)) throw new InvalidOperationException("An original PowerShell Job is already retained for this admission.");
                ScriptJobs.Add(key, original);
            }
        }
        public static void MarkDispatchStarted(string key) {
            lock (RegistryGate) {
                if (!Retained.ContainsKey(key) || !DispatchStarted.Add(key)) throw new InvalidOperationException("Build dispatch was not reserved or already started.");
            }
        }
        public static void ReleaseUndispatched(string key) {
            lock (RegistryGate) {
                if (!Retained.ContainsKey(key)) return;
                if (DispatchStarted.Contains(key) || ScriptJobs.ContainsKey(key) || Capturing.Contains(key) || Retained[key] != null)
                    throw new InvalidOperationException("Build startup may have dispatched; original retirement remains required.");
                Retained.Remove(key);
            }
        }
        public static void CaptureAndRetain(string key, string name, int processId, long createdAt, int parent, string image) {
            lock (RegistryGate) {
                if (!Retained.ContainsKey(key) || !ScriptJobs.ContainsKey(key)) throw new InvalidOperationException("Original build startup was not reserved and retained.");
                if (Retained[key] != null || !Capturing.Add(key)) throw new InvalidOperationException("Original native build capture already exists or is in progress.");
            }
            try {
                BuildJob owner = Open(name, processId, createdAt, parent, image);
                lock (RegistryGate) { Retained[key] = owner; }
            } finally { lock (RegistryGate) { Capturing.Remove(key); } }
        }
        public static bool IsScriptJobRemoved(string key) {
            lock (RegistryGate) { return ScriptJobsRemoved.Contains(key); }
        }
        public static void AcknowledgeScriptJobRemoval(string key, object original) {
            lock (RegistryGate) {
                object held;
                BuildJob owner;
                if (!ScriptJobs.TryGetValue(key, out held) || !Object.ReferenceEquals(held, original) ||
                    !Retained.TryGetValue(key, out owner) || owner == null || !owner.Acknowledged)
                    throw new InvalidOperationException("Exact original native and PowerShell ownership must precede repository removal acknowledgement.");
                ScriptJobsRemoved.Add(key);
            }
        }
        public static void Retire(string key, BuildJob owner) {
            if (!owner.Acknowledged) throw new InvalidOperationException("Cannot release an unacknowledged original build Job.");
            lock (RegistryGate) {
                if (!ScriptJobsRemoved.Contains(key)) throw new InvalidOperationException("Original PowerShell repository removal is not acknowledged.");
            }
            owner.Dispose();
            lock (RegistryGate) {
                BuildJob current;
                if (Retained.TryGetValue(key, out current) && Object.ReferenceEquals(owner, current)) {
                    Retained.Remove(key);
                    ScriptJobs.Remove(key);
                    ScriptJobsRemoved.Remove(key);
                    DispatchStarted.Remove(key);
                }
            }
        }
        public static BuildJob CreateAdmittedChildJob(string name, int processId) {
            lock (ChildGate) {
                if (childOriginal != null) throw new InvalidOperationException("This child already owns its original build admission Job.");
                childOriginal = CreateForCurrentProcess(name, processId);
                return childOriginal;
            }
        }
        static void Check(bool ok, string operation) {
            if (!ok) throw new Win32Exception(Marshal.GetLastWin32Error(), operation);
        }
        Member Capture(int processId, bool requireMember, bool retirementOnly = false) {
            if (members.ContainsKey(processId)) return members[processId];
            if (members.Count >= 4096) throw new InvalidOperationException("Retained build process-handle budget exceeded; cleanup remains unacknowledged.");
            uint access = 0x00100000 | 0x0400 | 0x1000;
            if (!requireMember) access |= 0x0100 | 1; // SET_QUOTA | TERMINATE for assignment
            IntPtr handle = OpenProcess(access, false, processId);
            if (handle == IntPtr.Zero) {
                int error = Marshal.GetLastWin32Error();
                if (error == 87) return null;
                throw new Win32Exception(error, "OpenProcess(build member)");
            }
            try {
                bool member;
                Check(IsProcessInJob(handle, job, out member), "IsProcessInJob(build member)");
                if (requireMember && !member) throw new InvalidOperationException("Captured process does not belong to the build Job.");
                // The held handle keeps the original process object/PID alive
                // while the shared exact-identity Inspector opens it.
                Snapshot identity = null;
                if (!retirementOnly) {
                    identity = Native.Inspect(processId);
                    if (identity.state != "present") throw new InvalidOperationException("Build member identity was unavailable.");
                }
                // Retirement needs membership and the exact held process
                // object's exit signal, not mutable image metadata. Windows
                // may reject image queries while exit is still in progress.
                // Admission above still requires the full identity snapshot.
                var owner = new Member { Handle = handle, Identity = identity };
                members.Add(processId, owner);
                handle = IntPtr.Zero;
                return owner;
            } finally { if (handle != IntPtr.Zero) CloseHandle(handle); }
        }
        public static BuildJob CreateForCurrentProcess(string name, int processId) {
            var owner = new BuildJob { Name = name };
            try {
                owner.job = CreateJobObject(IntPtr.Zero, name);
                int error = Marshal.GetLastWin32Error();
                Check(owner.job != IntPtr.Zero, "CreateJobObject(build)");
                if (error == 183) throw new InvalidOperationException("Build Job name already exists; refusing adoption.");
                var limits = new ExtendedLimit();
                limits.Basic.Flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                Check(SetInformationJobObject(owner.job, 9, ref limits, Marshal.SizeOf(typeof(ExtendedLimit))), "SetInformationJobObject(build)");
                var root = owner.Capture(processId, false);
                if (root == null) throw new InvalidOperationException("Build owner no longer exists.");
                Check(AssignProcessToJobObject(owner.job, root.Handle), "AssignProcessToJobObject(build)");
                owner.Root = root.Identity;
                return owner;
            } catch { owner.Dispose(); throw; }
        }
        public static BuildJob Open(string name, int processId, long createdAt, int parent, string image) {
            var owner = new BuildJob { Name = name };
            try {
                owner.job = OpenJobObject(2 | 4 | 8, false, name); // SET_ATTRIBUTES | QUERY | TERMINATE
                Check(owner.job != IntPtr.Zero, "OpenJobObject(build)");
                var root = owner.Capture(processId, true);
                if (root == null || root.Identity.createdAtFileTime != createdAt || root.Identity.parentProcessId != parent
                    || !String.Equals(root.Identity.imagePath, image, StringComparison.OrdinalIgnoreCase))
                    throw new InvalidOperationException("Build root identity changed; refusing admission.");
                owner.Root = root.Identity;
                return owner;
            } catch { owner.Dispose(); throw; }
        }
        public void ValidateMember(int processId) {
            lock (lifecycleGate) {
                if (job == IntPtr.Zero) throw new ObjectDisposedException("BuildJob");
                if (Capture(processId, true) == null) throw new InvalidOperationException("Original build member no longer exists.");
            }
        }
        int[] ProcessIds() {
            // Software headroom for one build, not an OS process limit.
            const int capacity = 4096;
            int bytes = 8 + capacity * IntPtr.Size;
            IntPtr buffer = Marshal.AllocHGlobal(bytes);
            try {
                uint returned;
                Check(QueryInformationJobObject(job, 3, buffer, (uint)bytes, out returned), "QueryInformationJobObject(build members)");
                int assigned = Marshal.ReadInt32(buffer, 0), listed = Marshal.ReadInt32(buffer, 4);
                if (assigned < 0 || listed < 0 || assigned > capacity || listed > capacity || listed < assigned)
                    throw new InvalidOperationException("Build Job member inventory exceeded its complete-list software budget.");
                var ids = new int[listed];
                for (int i = 0; i < listed; ++i) {
                    ids[i] = checked((int)Marshal.ReadIntPtr(buffer, 8 + i * IntPtr.Size).ToInt64());
                    if (ids[i] <= 0) throw new InvalidOperationException("Invalid build Job member identity.");
                }
                return ids;
            } finally { Marshal.FreeHGlobal(buffer); }
        }
        public void Quiesce() {
            lock (lifecycleGate) {
                if (job == IntPtr.Zero) throw new ObjectDisposedException("BuildJob");
                if (quiesced) return;
                // Cleanup only. While any member can create processes, active
                // count >=1, so another association exceeds this limit. At
                // count zero no member remains able to create a descendant.
                // Never change this limit during an admitted build's work.
                var limits = new ExtendedLimit();
                limits.Basic.Flags = 0x2000 | 8;
                limits.Basic.ActiveProcessLimit = 1;
                Check(SetInformationJobObject(job, 9, ref limits, Marshal.SizeOf(typeof(ExtendedLimit))), "SetInformationJobObject(build cleanup admission)");
                quiesced = true;
            }
        }
        public void StopAndWait(int timeoutMilliseconds) {
            lock (lifecycleGate) {
            if (job == IntPtr.Zero) throw new ObjectDisposedException("BuildJob");
            var clock = System.Diagnostics.Stopwatch.StartNew();
            Exception inventoryFailure = null;
            try { Quiesce(); foreach (int id in ProcessIds()) Capture(id, true, true); }
            catch (Exception error) { inventoryFailure = error; }
            Check(TerminateJobObject(job, 1), "TerminateJobObject(build)");
            // Trusted admission established this original Job's authority.
            // An incomplete inventory must not suppress an attempted stop,
            // but can never be relabeled as acknowledged completion.
            if (inventoryFailure != null) throw new InvalidOperationException("Build Job was stopped but member retirement remains unacknowledged: " + inventoryFailure.Message, inventoryFailure);
            while (clock.ElapsedMilliseconds < timeoutMilliseconds) {
                int[] ids = ProcessIds();
                foreach (int id in ids) Capture(id, true, true);
                if (ids.Length > 0) Check(TerminateJobObject(job, 1), "TerminateJobObject(build newly observed members)");
                bool signalled = true;
                foreach (Member member in members.Values) {
                    uint state = WaitForSingleObject(member.Handle, 0);
                    if (state == 258) signalled = false;
                    else if (state != 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "WaitForSingleObject(build member)");
                }
                // Empty accounting alone is never accepted as process exit.
                if (ids.Length == 0 && signalled) { Acknowledged = true; return; }
                System.Threading.Thread.Sleep(10);
            }
            throw new TimeoutException("Build Job termination was not acknowledged by all retained original process handles.");
            }
        }
        public void Dispose() {
            lock (lifecycleGate) {
                var failures = new System.Collections.Generic.List<string>();
                foreach (Member member in members.Values) {
                    if (member.Handle == IntPtr.Zero) continue;
                    if (CloseHandle(member.Handle)) member.Handle = IntPtr.Zero;
                    else failures.Add("CloseHandle(build member): " + Marshal.GetLastWin32Error());
                }
                if (job != IntPtr.Zero) {
                    if (CloseHandle(job)) job = IntPtr.Zero;
                    else failures.Add("CloseHandle(build Job): " + Marshal.GetLastWin32Error());
                }
                if (failures.Count > 0) throw new InvalidOperationException(String.Join("; ", failures.ToArray()));
                members.Clear();
            }
        }
    }
}
'@
}

if ($LoadOnly) { return }
if ($Action -eq 'Inspect') {
    $snapshot = [QcOwnedProcess.Native]::Inspect($ProcessId)
    [pscustomobject]@{
        state = $snapshot.state
        processId = $snapshot.processId
        parentProcessId = $snapshot.parentProcessId
        createdAtFileTime = [string]$snapshot.createdAtFileTime
        imagePath = $snapshot.imagePath
    } | ConvertTo-Json -Compress
    exit 0
}

if (-not $ExpectedImagePath -or $ExpectedCreatedAtFileTime -le 0 -or $ExpectedParentProcessId -le 0) {
    throw 'Expected PID, creation time, parent PID and image path are required for termination.'
}
[QcOwnedProcess.Native]::Stop($ProcessId, $ExpectedCreatedAtFileTime, $ExpectedParentProcessId, $ExpectedImagePath)
Write-Output '{"stopped":true}'

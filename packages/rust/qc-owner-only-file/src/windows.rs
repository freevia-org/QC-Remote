//! Writing a file that carries an explicit, non-inherited DACL naming this
//! Windows user and nobody else.
//!
//! # What this is for, and what it is not
//!
//! The application publishes a per-launch bearer token for local control.
//! Multiple cross-runtime clients must read the same descriptor, so it cannot
//! depend on a platform credential API unavailable to those clients.
//!
//! That is not merely an implementation gap. **Any unlock path a legitimate
//! same-user reader can take, a hostile same-user process can take
//! identically**, because the two are indistinguishable to Windows: they carry
//! the same token, open the same files, call the same APIs. DPAPI user scope
//! unwraps for both. `CredRead` succeeds for both. A loopback exchange
//! endpoint answers both. So no arrangement of this file can stop a process
//! running as this user from obtaining the token, and this module does not
//! claim to.
//!
//! A process that can write the descriptor is inside the same trust boundary
//! and can redirect readers. This module therefore protects publication from
//! other accounts; it does not claim to isolate mutually hostile processes
//! running as the same account.
//!
//! # What an owner-only DACL does buy
//!
//! Under the inherited ACL the descriptor was readable by `BUILTIN\Administrators`
//! and `NT AUTHORITY\SYSTEM` as well as by this user, and it inherited whatever
//! the parent directory happened to grant. An explicit protected DACL removes
//! both of those:
//!
//! * a service or scheduled task running as `SYSTEM` or as a local administrator
//!   that opens the path normally is refused;
//! * `%APPDATA%` redirected onto a file server no longer hands the token to
//!   whatever the share's inherited ACL grants - a domain group, Backup
//!   Operators, an indexing account;
//! * a later change to the ACL of `%LOCALAPPDATA%` or of `%APPDATA%\Claude`
//!   cannot widen this file, because it no longer inherits.
//!
//! # What it does not buy
//!
//! * **Nothing against the threat that matters.** A process running as this
//!   user still reads the token, and still takes full control of the hardware.
//! * A privileged process that *wants* the file still gets it: `SeBackupPrivilege`
//!   bypasses the DACL outright, and an administrator can take ownership.
//!   The win above is against privileged code that opens the path naively, not
//!   against privileged code that is trying.
//! * Nothing offline: a disk image, a shadow copy, or a memory dump of the app.
//! * Nothing about integrity - see the forged-descriptor note above.
//!
//! # Why an unprotectable file is refused rather than published
//!
//! A filesystem that cannot hold a DACL (FAT/exFAT, or an SMB server without
//! ACL support) silently ignores the security descriptor passed to
//! `CreateFileW`. So the DACL is read back off the open handle before a single
//! byte of the token is written, and the file is deleted and the publication
//! failed if it did not take. Writing the credential anyway, having announced
//! that it is protected, is the one outcome worse than not writing it: the
//! caller's error is visible in the LOCAL panel, whereas a silent downgrade is
//! visible nowhere.

use std::{
    fs::{self, File},
    io::Write as _,
    mem,
    os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _},
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    },
    Security::{
        AclSizeInformation, AddAccessAllowedAce,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        EqualSid, GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl,
        GetTokenInformation, InitializeAcl, InitializeSecurityDescriptor,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, TokenUser, ACCESS_ALLOWED_ACE,
        ACL, ACL_REVISION, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        SECURITY_DESCRIPTOR, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, CREATE_NEW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL,
    },
    System::{
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

/// Destructively replaces a private staging path with a file readable only by
/// this user, writes `contents`, and closes it.
///
/// The file is created empty, the DACL is verified on the open handle, and only
/// then is anything written - so the credential never exists on disk under an
/// inherited ACL, not even for the length of one syscall. A pre-existing file at
/// `path` is removed first: `CreateFileW` applies a security descriptor only to
/// a file it actually creates, so opening an existing one would keep whatever
/// DACL that file already had.
pub fn write_staged(path: &Path, contents: &[u8]) -> Result<(), String> {
    write_staged_verified_by(path, contents, names_only)
}

/// Create or validate an owner-private directory. Existing paths are opened
/// without following reparse points and are never modified or removed. Returns
/// `true` only when this call created the directory.
pub fn create_owner_only_directory(path: &Path) -> Result<bool, String> {
    let user = CurrentUser::look_up()?;
    let mut dacl = OwnerOnlyDacl::grant_full_access(user.sid())?;
    // SAFETY: all-zero is a valid uninitialised SECURITY_DESCRIPTOR value.
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { mem::zeroed() };
    let descriptor_pointer = owner_only_descriptor(&mut dacl, &mut descriptor)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor_pointer,
        bInheritHandle: 0,
    };
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated and `attributes` plus its descriptor/DACL
    // remain alive for the duration of the call. CreateDirectoryW never opens
    // or changes an existing path.
    let created = if unsafe { CreateDirectoryW(wide.as_ptr(), &raw const attributes) } != 0 {
        true
    } else {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            false
        } else {
            return Err(format!(
                "Could not create owner-only directory {}: {error}",
                path.display()
            ));
        }
    };

    match open_and_verify_directory(path, user.sid(), created) {
        Ok((_directory, true)) => Ok(created),
        Ok((directory, false)) => {
            if created {
                delete_open_empty_directory(&directory);
            }
            Err(format!(
                "Directory {} is not protected to this user",
                path.display()
            ))
        }
        Err(error) => Err(error),
    }
}

/// Verify the final path component through a handle opened with
/// `FILE_FLAG_OPEN_REPARSE_POINT`, checking both the DACL and actual owner SID.
pub fn is_owner_only_directory(path: &Path) -> Result<bool, String> {
    let user = CurrentUser::look_up()?;
    match open_and_verify_directory(path, user.sid(), false) {
        Ok((_directory, valid)) => Ok(valid),
        Err(error) => Err(error),
    }
}

fn open_and_verify_directory(
    path: &Path,
    expected_owner: PSID,
    include_delete: bool,
) -> Result<(File, bool), String> {
    use std::os::windows::fs::MetadataExt as _;

    let mut access = READ_CONTROL | FILE_READ_ATTRIBUTES;
    if include_delete {
        use windows_sys::Win32::Storage::FileSystem::DELETE;
        access |= DELETE;
    }
    let wide = wide_path(path);
    // SAFETY: `wide` is a NUL-terminated UTF-16 path. OPEN_REPARSE_POINT opens
    // the final reparse object itself, not its target; BACKUP_SEMANTICS permits
    // opening a directory handle.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        return Err(format!(
            "Could not inspect owner-only directory {}: {error}",
            path.display()
        ));
    }
    // SAFETY: CreateFileW returned an owned handle which File closes exactly once.
    let directory = unsafe { File::from_raw_handle(handle.cast()) };
    let metadata = directory
        .metadata()
        .map_err(|error| format!("Could not inspect directory {}: {error}", path.display()))?;
    let attributes = metadata.file_attributes();
    if !metadata.is_dir()
        || attributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Ok((directory, false));
    }
    Ok((
        directory,
        owner_and_dacl_are_private(handle, expected_owner)?,
    ))
}

fn owner_and_dacl_are_private(handle: HANDLE, expected_owner: PSID) -> Result<bool, String> {
    let mut owner: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `handle` is live for the caller's duration. All out-pointers are
    // writable locals, and the returned descriptor owns the owner/DACL buffers.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "Could not verify owner-only directory security: {}",
            std::io::Error::from_raw_os_error(status as i32)
        ));
    }
    let result = if owner.is_null() {
        Ok(false)
    } else {
        // SAFETY: both SIDs are valid for EqualSid: `owner` is within the live
        // descriptor returned above; expected_owner belongs to CurrentUser.
        let matches_owner = unsafe { EqualSid(owner, expected_owner) } != 0;
        if matches_owner {
            inspect_dacl(descriptor, dacl, expected_owner)
        } else {
            Ok(false)
        }
    };
    // SAFETY: descriptor is the LocalAlloc'd value returned by GetSecurityInfo.
    unsafe { LocalFree(descriptor) };
    result
}

fn delete_open_empty_directory(directory: &File) {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `directory` owns a live handle opened with DELETE access. The
    // disposition structure remains valid for this call. Deletion is by handle,
    // never by a possibly replaced path, and succeeds only while the directory
    // is empty.
    unsafe {
        SetFileInformationByHandle(
            directory.as_raw_handle().cast(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            mem::size_of_val(&disposition) as u32,
        );
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn owner_only_descriptor(
    dacl: &mut OwnerOnlyDacl,
    descriptor: &mut SECURITY_DESCRIPTOR,
) -> Result<PSECURITY_DESCRIPTOR, String> {
    let descriptor_pointer: PSECURITY_DESCRIPTOR = (&raw mut *descriptor).cast();
    // SAFETY: descriptor points to the live descriptor local, and dacl owns
    // aligned, initialized ACL storage that outlives each caller's API call.
    if unsafe { InitializeSecurityDescriptor(descriptor_pointer, SECURITY_DESCRIPTOR_REVISION) }
        == 0
    {
        return Err(last_error("initialize a security descriptor"));
    }
    // SAFETY: both locals remain alive until the caller's CreateFileW or
    // CreateDirectoryW call has copied the security descriptor.
    if unsafe { SetSecurityDescriptorDacl(descriptor_pointer, 1, dacl.as_mut_ptr(), 0) } == 0 {
        return Err(last_error("attach the owner-only DACL"));
    }
    // SAFETY: the descriptor is initialized above; this prevents inherited ACEs
    // from widening the one-user DACL.
    if unsafe {
        SetSecurityDescriptorControl(descriptor_pointer, SE_DACL_PROTECTED, SE_DACL_PROTECTED)
    } == 0
    {
        return Err(last_error("mark the DACL protected"));
    }
    Ok(descriptor_pointer)
}

/// The body of [`write_staged`], with the DACL read-back passed in.
///
/// The parameter exists so a test can drive the refusal branch: the filesystem
/// that silently discards a security descriptor is the one this code is most
/// careful about and the one a test cannot stand up, so the branch that decides
/// what to do about it is reached from a verifier that says no instead.
fn write_staged_verified_by(
    path: &Path,
    contents: &[u8],
    verify: fn(HANDLE, PSID) -> Result<bool, String>,
) -> Result<(), String> {
    let user = CurrentUser::look_up()?;
    let mut dacl = OwnerOnlyDacl::grant_full_access(user.sid())?;
    // SAFETY: SECURITY_DESCRIPTOR holds only integers and raw pointers, for which all-zero
    // bytes are a valid value (null pointers). It is initialised properly just below.
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { mem::zeroed() };
    let descriptor_pointer = owner_only_descriptor(&mut dacl, &mut descriptor)?;
    // SAFETY: `descriptor` was initialised above. `dacl` owns a DWORD-aligned buffer that holds a
    // fully initialised ACL. The descriptor keeps this pointer without copying it, so `dacl`
    // must outlive every use of `descriptor`: both are locals, `descriptor` is declared after
    // `dacl`, and its last use is the CreateFileW call below.
    // Without this the system merges the parent directory's inheritable ACEs
    // into the DACL supplied here, which is exactly the Administrators and
    // SYSTEM access this call exists to drop.
    // SAFETY: same initialised `descriptor`; SE_DACL_PROTECTED is a valid control bit to set
    // on an absolute security descriptor.
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor_pointer,
        bInheritHandle: 0,
    };

    // This is a private staging name owned by this process; removing a leftover
    // is how `CREATE_NEW` is guaranteed to be the creating open.
    let _ = fs::remove_file(path);
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call. `attributes` is a
    // live SECURITY_ATTRIBUTES whose descriptor points into `descriptor` and, through it, at
    // `dacl`; all of them stay alive until after the call, and CreateFileW applies the
    // descriptor while creating the file without retaining any of these pointers. The
    // template-file argument is null.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            // Readers may open it; nothing else may write it while it is being
            // staged.
            FILE_SHARE_READ,
            &raw const attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Could not create {} for this user only: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `CreateFileW` returned a handle it does not own any more, and the
    // `File` now closes it exactly once on drop, including on the error paths
    // below.
    let mut file = unsafe { File::from_raw_handle(handle.cast()) };

    match verify(handle, user.sid()) {
        Ok(true) => {}
        Ok(false) => {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(format!(
                "{} cannot hold a per-user DACL, so a full-control credential was not written to it",
                path.display()
            ));
        }
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(error);
        }
    }

    if let Err(error) = file.write_all(contents) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(format!("Could not write {}: {error}", path.display()));
    }
    Ok(())
}

/// Whether the file at `path` grants this user and nobody else, with
/// inheritance switched off.
///
/// Publication verifies the handle it already holds rather
/// than reopening by name. It exists so the tests assert the DACL a descriptor
/// actually ends up with, rather than trusting that [`write`] was reached.
pub fn is_owner_only(path: &Path) -> Result<bool, String> {
    use std::os::windows::io::AsRawHandle as _;

    let user = CurrentUser::look_up()?;
    // `File::open` asks for `FILE_GENERIC_READ`, which includes `READ_CONTROL`:
    // enough for `GetSecurityInfo` to return the DACL.
    let file =
        File::open(path).map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    names_only(file.as_raw_handle().cast(), user.sid())
}

/// Reads the DACL back off an open handle and reports whether it is exactly one
/// full-access ACE for `expected`, with inheritance blocked.
///
/// Read back rather than assumed, because a filesystem without ACL support
/// accepts the security descriptor at creation and discards it.
fn names_only(handle: HANDLE, expected: PSID) -> Result<bool, String> {
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `handle` is an open file handle that the caller keeps alive for the whole call
    // (write_staged_verified_by holds `file`, is_owner_only holds its `File`). `dacl` and
    // `descriptor` are writable locals; the other out-parameters are null, which is allowed
    // because only DACL_SECURITY_INFORMATION is requested. On success `descriptor` owns the
    // LocalAlloc'd block that `dacl` points into.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        // A volume with no ACL support answers here rather than at creation.
        return Ok(false);
    }
    let verdict = inspect_dacl(descriptor, dacl, expected);
    // `GetSecurityInfo` allocates the security descriptor - and the `dacl`
    // pointer into it - with `LocalAlloc`.
    // SAFETY: reached only after GetSecurityInfo succeeded, so `descriptor` is the LocalAlloc'd
    // block it returned, and nothing else frees it. `dacl` points into that block and is not
    // used again, and `verdict` holds no pointer into it.
    unsafe { LocalFree(descriptor) };
    verdict
}

/// The ACL comparison itself, kept apart from the allocation so every exit
/// still reaches the `LocalFree` above.
fn inspect_dacl(
    descriptor: PSECURITY_DESCRIPTOR,
    dacl: *const ACL,
    expected: PSID,
) -> Result<bool, String> {
    let mut control: u16 = 0;
    let mut revision: u32 = 0;
    // SAFETY: `descriptor` is the live security descriptor GetSecurityInfo returned (the caller
    // frees it only after this function returns); `control` and `revision` are writable locals
    // of the types the API fills.
    if unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) } == 0
    {
        return Err(last_error("read the security descriptor control bits"));
    }
    if control & SE_DACL_PROTECTED == 0 {
        // The parent's inheritable ACEs are still in force, whatever else the
        // DACL says.
        return Ok(false);
    }
    if dacl.is_null() {
        // A NULL DACL grants everyone everything. It is the one case where a
        // present-and-empty check would report success on a wide-open file.
        return Ok(false);
    }
    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: `dacl` is non-null (checked above) and points into the live descriptor. `size` is a
    // writable ACL_SIZE_INFORMATION, its size_of is passed, and that is the structure the
    // `AclSizeInformation` class fills.
    if unsafe {
        GetAclInformation(
            dacl,
            (&raw mut size).cast(),
            mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last_error("measure the DACL"));
    }
    if size.AceCount != 1 {
        return Ok(false);
    }
    let mut ace: *mut core::ffi::c_void = ptr::null_mut();
    // SAFETY: `dacl` is valid as above and index 0 is in range, because AceCount was checked to
    // be exactly 1. `ace` is a writable pointer slot.
    if unsafe { GetAce(dacl, 0, &raw mut ace) } == 0 {
        return Err(last_error("read the single ACE"));
    }
    let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: `ace` came from GetAce for index 0, so it points at an ACE inside the live ACL.
    // Every ACE starts with an ACE_HEADER, so reading the header through the
    // ACCESS_ALLOWED_ACE view is valid whatever the ACE type is.
    let header = unsafe { (*ace).Header };
    if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
        return Ok(false);
    }
    // SAFETY: the type check above established an ACCESS_ALLOWED_ACE, so `Mask` lies inside the
    // ACE; ACEs are DWORD aligned within an ACL, as is the struct.
    if unsafe { (*ace).Mask } != FILE_ALL_ACCESS {
        return Ok(false);
    }
    // SAFETY: only the address of `SidStart` is taken, nothing is read here. The SID starts at
    // that field and extends past the struct as part of the ACE, which is why the pointer is
    // derived from the ACE pointer rather than from a reference to the field.
    let ace_sid: PSID = unsafe { (&raw const (*ace).SidStart).cast_mut().cast() };
    // SAFETY: `ace_sid` points at the SID embedded in the checked ACE, and `expected` at the SID
    // in the CurrentUser buffer that the caller keeps alive. EqualSid only reads both.
    Ok(unsafe { EqualSid(ace_sid, expected) } != 0)
}

/// This process's own user SID, kept alive in the buffer `GetTokenInformation`
/// filled, because the `TOKEN_USER` it starts with points into that buffer.
struct CurrentUser {
    /// `u64` elements only for their alignment: `TOKEN_USER` holds a pointer.
    buffer: Vec<u64>,
}

impl CurrentUser {
    fn look_up() -> Result<Self, String> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing. `token` is a
        // writable local that receives an owned token handle on success, and every path below
        // closes it.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(last_error("open this process's access token"));
        }
        let mut needed: u32 = 0;
        // The first call always fails; it is how the buffer is sized.
        // SAFETY: the documented sizing call: a null buffer with length 0 is allowed, the call
        // fails with ERROR_INSUFFICIENT_BUFFER and writes the required size to `needed`.
        // `token` is the open handle from above.
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &raw mut needed);
        }
        if needed == 0 {
            let error = last_error("size this user's SID");
            // SAFETY: `token` was opened above and is closed here on a path that returns, so once.
            unsafe { CloseHandle(token) };
            return Err(error);
        }
        let words = (needed as usize).div_ceil(mem::size_of::<u64>());
        let mut buffer = vec![0_u64; words];
        // SAFETY: `buffer` holds at least `needed` bytes (rounded up to whole u64 words), is 8-byte
        // aligned and exclusively borrowed here, so the API may write up to `needed` bytes into
        // it. `token` is open and the length out-parameter is a writable local.
        let filled = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &raw mut needed,
            )
        };
        // SAFETY: `token` is still open here; the error path above returned after closing it, so no
        // path closes it twice.
        unsafe { CloseHandle(token) };
        if filled == 0 {
            return Err(last_error("read this user's SID"));
        }
        Ok(Self { buffer })
    }

    fn sid(&self) -> PSID {
        // SAFETY: a CurrentUser exists only after GetTokenInformation(TokenUser) filled `buffer`,
        // which is 8-byte aligned (Vec<u64>) and at least TOKEN_USER sized. The pointer is only
        // read, and the SID it names lives in the same buffer, so it is valid while `self` is.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// An ACL holding one access-allowed ACE, in a buffer aligned the way the ACL
/// functions require.
struct OwnerOnlyDacl {
    /// `u32` elements because an `ACL` and every ACE inside it are DWORD
    /// aligned; a `Vec<u8>` would only be byte aligned.
    buffer: Vec<u32>,
}

impl OwnerOnlyDacl {
    fn grant_full_access(sid: PSID) -> Result<Self, String> {
        // SAFETY: `sid` must be a valid SID. The only caller passes CurrentUser::sid, whose buffer
        // outlives this call.
        let sid_length = unsafe { GetLengthSid(sid) } as usize;
        if sid_length == 0 {
            return Err(last_error("measure this user's SID"));
        }
        // `ACCESS_ALLOWED_ACE` ends with `SidStart`, the first DWORD of the SID
        // itself, so that DWORD must not be counted twice.
        let bytes = mem::size_of::<ACL>() + mem::size_of::<ACCESS_ALLOWED_ACE>()
            - mem::size_of::<u32>()
            + sid_length;
        let mut dacl = Self {
            buffer: vec![0_u32; bytes.div_ceil(mem::size_of::<u32>())],
        };
        // SAFETY: `dacl.buffer` holds at least `bytes` bytes (rounded up to whole u32 words) and is
        // zeroed, writable and DWORD aligned, which is what InitializeAcl needs for an ACL of
        // `bytes` bytes.
        if unsafe { InitializeAcl(dacl.as_mut_ptr(), bytes as u32, ACL_REVISION) } == 0 {
            return Err(last_error("initialize an ACL"));
        }
        // SAFETY: the ACL was just initialised with room for one ACE of this SID (see the `bytes`
        // computation above), and `sid` is valid for the call as in the length query.
        if unsafe { AddAccessAllowedAce(dacl.as_mut_ptr(), ACL_REVISION, FILE_ALL_ACCESS, sid) }
            == 0
        {
            return Err(last_error("grant this user access to the descriptor"));
        }
        Ok(dacl)
    }

    fn as_mut_ptr(&mut self) -> *mut ACL {
        self.buffer.as_mut_ptr().cast()
    }
}

fn last_error(action: &str) -> String {
    format!(
        "Could not {action} for the owner-only file: {}",
        std::io::Error::last_os_error()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_file(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "qc-remote-owner-only-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch directory");
        directory.join("local-mcp.json")
    }

    fn scratch_directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "qc-owner-only-directory-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    #[test]
    fn owner_only_directory_is_created_and_verified() {
        let parent = scratch_directory("created");
        let path = parent.join("private");
        assert!(create_owner_only_directory(&path).expect("create protected directory"));
        assert!(is_owner_only_directory(&path).expect("verify protected directory"));
        // Existing secure directories are accepted without replacement.
        assert!(!create_owner_only_directory(&path).expect("accept existing protected directory"));
        fs::remove_dir_all(parent).expect("clean scratch directory");
    }

    #[test]
    fn owner_only_directory_rejects_preexisting_insecure_directory_without_mutating_it() {
        let parent = scratch_directory("insecure");
        let path = parent.join("shared");
        fs::create_dir(&path).expect("create inherited-ACL directory");
        assert!(!is_owner_only_directory(&path).expect("inspect inherited ACL"));
        assert!(create_owner_only_directory(&path).is_err());
        assert!(path.is_dir(), "the existing directory must not be removed");
        assert!(!is_owner_only_directory(&path).expect("reinspect inherited ACL"));
        fs::remove_dir_all(parent).expect("clean scratch directory");
    }

    #[test]
    fn owner_only_directory_rejects_reparse_point() {
        use std::os::windows::fs::symlink_dir;
        let parent = scratch_directory("reparse");
        let target = parent.join("target");
        let link = parent.join("link");
        fs::create_dir(&target).expect("create symlink target");
        if symlink_dir(&target, &link).is_err() {
            // Symlink creation may be unavailable when Developer Mode or the
            // required privilege is disabled; the other Windows cases remain.
            fs::remove_dir_all(parent).expect("clean scratch directory");
            return;
        }
        assert!(!is_owner_only_directory(&link).expect("inspect reparse point"));
        assert!(create_owner_only_directory(&link).is_err());
        assert!(target.is_dir(), "the reparse target must remain untouched");
        fs::remove_dir_all(parent).expect("clean scratch directory");
    }

    #[test]
    fn a_separate_process_running_as_this_user_still_reads_the_file() {
        // The load-bearing claim of this whole change: tightening the DACL must
        // not cost an external reader. A same-process `fs::read` does not prove
        // that a separate cross-runtime client can open the descriptor.
        let path = scratch_file("cross-process");
        write_staged(&path, b"{\"bearerToken\":\"secret\"}").expect("owner-only staging write");

        let read_back = std::process::Command::new("node")
            .arg("-e")
            .arg("process.stdout.write(require('node:fs').readFileSync(process.argv[1],'utf8'))")
            .arg(&path)
            .output()
            .expect("node is required to verify that readers still reach the descriptor");
        let _ = fs::remove_dir_all(path.parent().expect("scratch parent"));

        assert!(
            read_back.status.success(),
            "a separate process as this user could not open the descriptor: {}",
            String::from_utf8_lossy(&read_back.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&read_back.stdout),
            "{\"bearerToken\":\"secret\"}",
            "the reader got something other than what was published"
        );
    }

    #[test]
    fn a_filesystem_that_discards_the_dacl_gets_no_credential_at_all() {
        // FAT, exFAT and an SMB server without ACL support accept a security
        // descriptor at creation and throw it away without an error. The only
        // acceptable answer is to write nothing and fail the publication: a
        // silent downgrade would leave a full-control token in the one class of
        // location that cannot protect it, under a claim that it is protected.
        let path = scratch_file("unprotectable");

        let refused =
            write_staged_verified_by(&path, b"{\"bearerToken\":\"secret\"}", |_, _| Ok(false));

        let left_behind = path.exists();
        let _ = fs::remove_dir_all(path.parent().expect("scratch parent"));
        let message = refused.expect_err("an unprotectable file must fail the publication");
        assert!(
            message.contains("cannot hold a per-user DACL"),
            "the failure has to name the reason the caller reports: {message}"
        );
        assert!(
            !left_behind,
            "the credential was left on disk by the path that reported refusing to write it"
        );
    }

    #[test]
    fn writing_replaces_a_file_that_was_left_behind_with_a_looser_dacl() {
        // `CreateFileW` applies a security descriptor only to a file it
        // creates. A staged name a previous run left behind would otherwise
        // keep its inherited DACL and take the token with it.
        let path = scratch_file("stale");
        fs::write(&path, b"stale").expect("leftover stage file");
        assert!(
            !is_owner_only(&path).expect("inspect the leftover"),
            "the leftover was already owner-only, so this test proves nothing"
        );

        write_staged(&path, b"{\"bearerToken\":\"secret\"}").expect("owner-only staging write");

        let owner_only = is_owner_only(&path).expect("inspect the DACL");
        let contents = fs::read(&path).expect("read back");
        let _ = fs::remove_dir_all(path.parent().expect("scratch parent"));

        assert!(
            owner_only,
            "a leftover staged file kept its inherited DACL and the credential went into it"
        );
        assert_eq!(contents, b"{\"bearerToken\":\"secret\"}");
    }
}

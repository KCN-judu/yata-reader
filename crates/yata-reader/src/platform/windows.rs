//! The Win32 calls the desktop channel needs, and nothing else, behind safe functions
//! (ADR-0002). This is the one module that may use `unsafe`.
//!
//! The functions are declared here by hand rather than taken from a bindings crate, so the list
//! below is exactly what the reader can call: listing processes, querying a process with limited
//! rights, opening one with [`READ_ACCESS`], querying and copying its memory, closing handles,
//! asking whether this process is elevated, and finding the local application data folder. No
//! function that writes to, allocates in, or runs code in another process is declared, so none
//! can be called (`reader-security.md`, R2).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::io;
use std::path::PathBuf;

use crate::layout::limits::POINTER_END;
use crate::layout::memory::{Region, RegionKind};

type Handle = *mut c_void;
type Bool = i32;

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const PROCESS_VM_READ: u32 = 0x0010;

/// The only access the reader asks of the game: query with limited rights, and read memory (R2).
pub const READ_ACCESS: u32 = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ;

const TH32CS_SNAPPROCESS: u32 = 0x2;
const TOKEN_QUERY: u32 = 0x8;
const TOKEN_ELEVATION: u32 = 20;
const MEM_COMMIT: u32 = 0x1000;
const MEM_IMAGE: u32 = 0x100_0000;
const MEM_MAPPED: u32 = 0x4_0000;
const PAGE_GUARD: u32 = 0x100;
/// `PAGE_READONLY`, `PAGE_READWRITE`, `PAGE_WRITECOPY`, and their executable forms.
const READABLE: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
const WRITABLE: u32 = 0x04 | 0x08 | 0x40 | 0x80;
const IMAGE_FILE_MACHINE_UNKNOWN: u16 = 0;
/// 100-nanosecond intervals from 1601-01-01 to 1970-01-01.
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    process_id: u32,
    default_heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_process_id: u32,
    priority: i32,
    flags: u32,
    exe_file: [u16; 260],
}

#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `FOLDERID_LocalAppData`.
const LOCAL_APP_DATA: Guid = Guid {
    data1: 0xF1B3_2785,
    data2: 0x6FBA,
    data3: 0x4FCF,
    data4: [0x9D, 0x55, 0x7B, 0x8E, 0x7F, 0x15, 0x70, 0x91],
};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(desired_access: u32, inherit_handle: Bool, process_id: u32) -> Handle;
    fn CloseHandle(object: Handle) -> Bool;
    fn ReadProcessMemory(
        process: Handle,
        base_address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        bytes_read: *mut usize,
    ) -> Bool;
    fn VirtualQueryEx(
        process: Handle,
        address: *const c_void,
        buffer: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry32W) -> Bool;
    fn IsWow64Process2(
        process: Handle,
        process_machine: *mut u16,
        native_machine: *mut u16,
    ) -> Bool;
    fn GetProcessTimes(
        process: Handle,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> Bool;
    fn ProcessIdToSessionId(process_id: u32, session_id: *mut u32) -> Bool;
    safe fn GetCurrentProcess() -> Handle;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, desired_access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> Bool;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        folder: *const Guid,
        flags: u32,
        token: Handle,
        path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(memory: *mut c_void);
}

/// A handle this module opened, closed when dropped. Held as an integer, so it can move between
/// threads; it is only ever turned back into a handle to pass to the functions above.
struct Owned(usize);

impl Owned {
    fn open(access: u32, pid: u32) -> io::Result<Owned> {
        // SAFETY: OpenProcess takes plain values and returns a new handle or null.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Owned(h as usize))
        }
    }

    fn get(&self) -> Handle {
        self.0 as Handle
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by the system to this module and is closed once.
        unsafe { CloseHandle(self.get()) };
    }
}

/// One entry of the process list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub image_name: String,
}

/// Every process on the system, from a snapshot; no process is opened.
pub fn processes() -> io::Result<Vec<ProcessEntry>> {
    // SAFETY: CreateToolhelp32Snapshot takes plain values and returns a new handle or
    // INVALID_HANDLE_VALUE.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot as isize == -1 {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Owned(snapshot as usize);
    let mut entry = ProcessEntry32W {
        size: size_of::<ProcessEntry32W>() as u32,
        usage: 0,
        process_id: 0,
        default_heap_id: 0,
        module_id: 0,
        threads: 0,
        parent_process_id: 0,
        priority: 0,
        flags: 0,
        exe_file: [0; 260],
    };
    let mut out = Vec::new();
    // SAFETY: the snapshot is open and `entry` is a writable PROCESSENTRY32W whose size field is
    // set, as the function requires.
    let mut more = unsafe { Process32FirstW(snapshot.get(), &raw mut entry) } != 0;
    while more {
        let len = entry.exe_file.iter().position(|&c| c == 0).unwrap_or(260);
        out.push(ProcessEntry {
            pid: entry.process_id,
            parent_pid: entry.parent_process_id,
            image_name: String::from_utf16_lossy(&entry.exe_file[..len]),
        });
        // SAFETY: as for Process32FirstW.
        more = unsafe { Process32NextW(snapshot.get(), &raw mut entry) } != 0;
    }
    Ok(out)
}

/// The pointer width a process runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitness {
    Bits32,
    Bits64,
}

/// What can be learnt about a process with limited query rights alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessFacts {
    pub bitness: Option<Bitness>,
    pub created_unix_ms: Option<u64>,
    pub session_id: Option<u32>,
}

/// Facts about a process, as far as `PROCESS_QUERY_LIMITED_INFORMATION` gives them.
pub fn facts(pid: u32) -> ProcessFacts {
    let mut f = ProcessFacts::default();
    let mut session = 0u32;
    // SAFETY: `session` is a writable u32.
    if unsafe { ProcessIdToSessionId(pid, &raw mut session) } != 0 {
        f.session_id = Some(session);
    }
    let Ok(h) = Owned::open(PROCESS_QUERY_LIMITED_INFORMATION, pid) else {
        return f;
    };
    let (mut process_machine, mut native_machine) = (0u16, 0u16);
    // SAFETY: the handle is open with the limited query right the function needs, and both
    // outputs are writable u16s.
    if unsafe { IsWow64Process2(h.get(), &raw mut process_machine, &raw mut native_machine) } != 0 {
        // A process not under WOW64 has the host's width; this reader is built for 64-bit hosts.
        f.bitness = Some(if process_machine == IMAGE_FILE_MACHINE_UNKNOWN {
            Bitness::Bits64
        } else {
            Bitness::Bits32
        });
    }
    let mut created = FileTime::default();
    let (mut exited, mut kernel, mut user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: the handle is open with the limited query right, and the four outputs are
    // writable FILETIMEs.
    let ok = unsafe {
        GetProcessTimes(
            h.get(),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok != 0 {
        let FileTime { low, high } = created;
        let ticks = (u64::from(high) << 32) | u64::from(low);
        f.created_unix_ms = ticks.checked_sub(FILETIME_UNIX_EPOCH).map(|t| t / 10_000);
    }
    f
}

/// Whether this process runs elevated.
pub fn is_elevated() -> io::Result<bool> {
    let mut token: Handle = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of this process is always valid, and `token` is writable.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Owned(token as usize);
    let mut elevated = 0u32;
    let mut returned = 0u32;
    // SAFETY: the token is open for query, and `elevated` is a writable TOKEN_ELEVATION (one
    // u32) whose size is passed.
    let ok = unsafe {
        GetTokenInformation(
            token.get(),
            TOKEN_ELEVATION,
            (&raw mut elevated).cast(),
            4,
            &raw mut returned,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(elevated != 0)
}

/// A process opened with [`READ_ACCESS`] and nothing more.
pub struct Process {
    handle: Owned,
}

/// Open a process for reading. The error is the operating system's, for the caller to classify.
pub fn open(pid: u32) -> io::Result<Process> {
    Owned::open(READ_ACCESS, pid).map(|handle| Process { handle })
}

impl Process {
    /// The committed, readable, unguarded regions of the process, in address order.
    pub fn regions(&self) -> Vec<Region> {
        let mut out = Vec::new();
        let mut address = 0u64;
        while address < POINTER_END {
            let mut info = MemoryBasicInformation {
                base_address: std::ptr::null_mut(),
                allocation_base: std::ptr::null_mut(),
                allocation_protect: 0,
                partition_id: 0,
                region_size: 0,
                state: 0,
                protect: 0,
                kind: 0,
            };
            // SAFETY: the handle is open with query rights, and `info` is a writable
            // MEMORY_BASIC_INFORMATION whose size is passed.
            let n = unsafe {
                VirtualQueryEx(
                    self.handle.get(),
                    address as *const c_void,
                    &raw mut info,
                    size_of::<MemoryBasicInformation>(),
                )
            };
            if n == 0 || info.region_size == 0 {
                break;
            }
            let base = info.base_address as u64;
            let size = info.region_size as u64;
            if info.state == MEM_COMMIT
                && info.protect & READABLE != 0
                && info.protect & PAGE_GUARD == 0
            {
                out.push(Region {
                    base,
                    size,
                    kind: match info.kind {
                        MEM_IMAGE => RegionKind::Image,
                        MEM_MAPPED => RegionKind::Mapped,
                        _ => RegionKind::Private,
                    },
                    writable: info.protect & WRITABLE != 0,
                });
            }
            address = base.saturating_add(size);
        }
        out
    }

    /// Copy `buf.len()` bytes from `address`, all of them or an error.
    pub fn read(&self, address: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut read = 0usize;
        // SAFETY: the handle is open with PROCESS_VM_READ, `buf` is writable for its length, and
        // the address is only read by the system, in the other process.
        let ok = unsafe {
            ReadProcessMemory(
                self.handle.get(),
                address as *const c_void,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut read,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else if read != buf.len() {
            Err(io::Error::from(io::ErrorKind::UnexpectedEof))
        } else {
            Ok(())
        }
    }
}

/// The user's local application data folder, where the reader keeps its log.
pub fn local_app_data() -> Option<PathBuf> {
    let mut path: *mut u16 = std::ptr::null_mut();
    // SAFETY: the folder id is a valid GUID, a null token means the current user, and `path`
    // receives a string the system allocates.
    let hr =
        unsafe { SHGetKnownFolderPath(&LOCAL_APP_DATA, 0, std::ptr::null_mut(), &raw mut path) };
    let result = if hr == 0 && !path.is_null() {
        let mut len = 0usize;
        // SAFETY: on success the system returns a NUL-terminated wide string; the loop reads up
        // to its terminator.
        while unsafe { path.wrapping_add(len).read() } != 0 {
            len += 1;
        }
        // SAFETY: `len` units before the terminator were just read and are initialized.
        let units = unsafe { std::slice::from_raw_parts(path, len) };
        Some(PathBuf::from(String::from_utf16_lossy(units)))
    } else {
        None
    };
    // SAFETY: the string was allocated by SHGetKnownFolderPath, which requires this call to
    // free it, even on failure; freeing null is allowed.
    unsafe { CoTaskMemFree(path.cast()) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_is_opened_with_query_and_read_rights_only() {
        assert_eq!(READ_ACCESS, 0x1010);
    }

    #[test]
    fn the_process_list_holds_this_process() {
        let me = std::process::id();
        let list = processes().expect("a snapshot");
        let entry = list.iter().find(|p| p.pid == me).expect("this process");
        assert!(entry.image_name.to_ascii_lowercase().ends_with(".exe"));
    }

    #[test]
    fn this_process_is_64_bit_and_readable() {
        let f = facts(std::process::id());
        assert_eq!(f.bitness, Some(Bitness::Bits64));
        assert!(f.created_unix_ms.is_some());
        let p = open(std::process::id()).expect("our own process");
        let regions = p.regions();
        assert!(regions.iter().any(|r| r.kind == RegionKind::Image));
        let value = 0x0123_4567_89ab_cdef_u64;
        let mut buf = [0u8; 8];
        p.read((&raw const value).addr() as u64, &mut buf)
            .expect("readable");
        assert_eq!(u64::from_le_bytes(buf), value);
    }

    #[test]
    fn elevation_can_be_asked() {
        assert!(is_elevated().is_ok());
    }

    #[test]
    fn the_local_application_data_folder_exists() {
        assert!(local_app_data().is_some_and(|p| p.is_dir()));
    }
}

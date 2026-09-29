//! macOS directory listing via `getattrlistbulk(2)`.
//!
//! One syscall returns a batch of entries together with their type, size and
//! timestamps, which is much cheaper than an `lstat` per entry when metadata
//! is needed.

use crate::{DirEntry, NodeFileType, NodeMetadata};
use std::{
    cell::RefCell,
    ffi::{CStr, CString, OsStr},
    io::{Error, ErrorKind, Result},
    num::NonZeroU64,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

const BUFFER_SIZE: usize = 128 * 1024;

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; BUFFER_SIZE]);
}

// Not exported by the libc crate.
const ATTR_CMN_ERROR: libc::attrgroup_t = 0x2000_0000;
// `enum vtype` from <sys/vnode.h>.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const COMMON_ATTRS: libc::attrgroup_t = libc::ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_CRTIME
    | libc::ATTR_CMN_MODTIME;
const FILE_ATTRS: libc::attrgroup_t = libc::ATTR_FILE_DATALENGTH;

/// Lists `path` with `getattrlistbulk`. Returns `Ok(None)` if the file system
/// rejected the bulk call, in which case the caller should fall back to
/// `read_dir`.
pub(crate) fn list_dir(path: &Path) -> Result<Option<Vec<DirEntry>>> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::from(ErrorKind::InvalidInput))?;
    let fd = loop {
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            break fd;
        }
        let error = Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let entries = list_fd(fd);
    unsafe { libc::close(fd) };
    Ok(entries)
}

fn list_fd(fd: libc::c_int) -> Option<Vec<DirEntry>> {
    let mut attrlist: libc::attrlist = unsafe { std::mem::zeroed() };
    attrlist.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrlist.commonattr = COMMON_ATTRS;
    attrlist.fileattr = FILE_ATTRS;

    BUFFER.with_borrow_mut(|buffer| {
        let mut entries = Vec::new();
        loop {
            let count = unsafe {
                libc::getattrlistbulk(
                    fd,
                    (&raw mut attrlist).cast(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    0,
                )
            };
            match count {
                0 => return Some(entries),
                1.. => parse_entries(buffer, count as usize, &mut entries),
                _ => {
                    if Error::last_os_error().kind() != ErrorKind::Interrupted {
                        return None;
                    }
                }
            }
        }
    })
}

fn read<const N: usize>(buffer: &[u8], offset: usize) -> Option<[u8; N]> {
    buffer.get(offset..offset + N)?.try_into().ok()
}

fn read_u32(buffer: &[u8], offset: usize) -> Option<u32> {
    read(buffer, offset).map(u32::from_ne_bytes)
}

fn read_i64(buffer: &[u8], offset: usize) -> Option<i64> {
    read(buffer, offset).map(i64::from_ne_bytes)
}

fn parse_entries(buffer: &[u8], count: usize, entries: &mut Vec<DirEntry>) {
    let mut offset = 0;
    for _ in 0..count {
        let Some(length) = read_u32(buffer, offset).map(|x| x as usize) else {
            return;
        };
        let Some(entry) = buffer.get(offset..offset + length) else {
            return;
        };
        if length == 0 {
            return;
        }
        if let Some(entry) = parse_entry(entry) {
            entries.push(entry);
        }
        offset += length;
    }
}

/// Parses one packed entry. Attributes appear in bit order within each group
/// and only if they were returned; see getattrlist(2).
fn parse_entry(entry: &[u8]) -> Option<DirEntry> {
    let returned_common = read_u32(entry, 4)?;
    let returned_file = read_u32(entry, 4 + 12)?;
    let mut cursor = 4 + size_of::<libc::attribute_set_t>();

    let mut error = 0;
    if returned_common & ATTR_CMN_ERROR != 0 {
        error = read_u32(entry, cursor)?;
        cursor += 4;
    }
    if returned_common & libc::ATTR_CMN_NAME == 0 {
        return None;
    }
    let name_offset = i32::from_ne_bytes(read(entry, cursor)?);
    let name_length = read_u32(entry, cursor + 4)? as usize;
    let name_start = cursor.checked_add_signed(name_offset as isize)?;
    let name = entry.get(name_start..name_start + name_length)?;
    let name = CStr::from_bytes_until_nul(name).ok()?.to_bytes();
    let name = OsStr::from_bytes(name)
        .to_string_lossy()
        .into_owned()
        .into_boxed_str();
    cursor += size_of::<libc::attrreference_t>();

    let r#type = if returned_common & libc::ATTR_CMN_OBJTYPE != 0 {
        let objtype = read_u32(entry, cursor)?;
        cursor += 4;
        Some(match objtype {
            VREG => NodeFileType::File,
            VDIR => NodeFileType::Dir,
            VLNK => NodeFileType::Symlink,
            _ => NodeFileType::Unknown,
        })
    } else {
        None
    };
    let mut timestamp = |flag| {
        if returned_common & flag == 0 {
            return Some(None);
        }
        let secs = read_i64(entry, cursor)?;
        cursor += size_of::<libc::timespec>();
        Some(u64::try_from(secs).ok().and_then(NonZeroU64::new))
    };
    let ctime = timestamp(libc::ATTR_CMN_CRTIME)?;
    let mtime = timestamp(libc::ATTR_CMN_MODTIME)?;
    let size = if returned_file & libc::ATTR_FILE_DATALENGTH != 0 {
        u64::from_ne_bytes(read(entry, cursor)?)
    } else {
        0
    };

    let metadata = match r#type {
        Some(r#type) if error == 0 => Some(NodeMetadata {
            r#type,
            size,
            ctime,
            mtime,
        }),
        _ => None,
    };
    Some(DirEntry {
        name,
        r#type,
        metadata,
    })
}

/// Mount points strictly below `root` that a scan shouldn't descend into:
/// volumes hidden from Finder (`nobrowse`) and non-local ones. That covers
/// the system volumes under `/System/Volumes` (whose contents are already
/// reachable through firmlinks), simulator runtimes, `/dev`, network shares
/// and FSKit file systems such as `~/Library/Developer/CoreDevice/DeviceFS`,
/// which exposes connected devices and takes minutes to traverse.
pub(crate) fn skipped_mount_points(root: &Path) -> Vec<PathBuf> {
    // MNT_NOWAIT: don't block on unresponsive network mounts.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        return Vec::new();
    }
    // Leave headroom in case mounts appear between the two calls.
    let capacity = count as usize + 16;
    let mut mounts = Vec::<libc::statfs>::with_capacity(capacity);
    let count = unsafe {
        libc::getfsstat(
            mounts.as_mut_ptr(),
            (capacity * size_of::<libc::statfs>()) as libc::c_int,
            libc::MNT_NOWAIT,
        )
    };
    if count <= 0 {
        return Vec::new();
    }
    unsafe { mounts.set_len(count as usize) };

    mounts
        .iter()
        .filter(|mount| {
            let flags = mount.f_flags as libc::c_int;
            flags & libc::MNT_DONTBROWSE != 0 || flags & libc::MNT_LOCAL == 0
        })
        .map(|mount| {
            let path = unsafe { CStr::from_ptr(mount.f_mntonname.as_ptr()) };
            PathBuf::from(OsStr::from_bytes(path.to_bytes()))
        })
        .filter(|path| path.starts_with(root) && path != root)
        .collect()
}

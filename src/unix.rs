use std::num::NonZeroUsize;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::io::RawFd;
use std::ptr::null_mut;

use crate::log::*;
use nix::fcntl::OFlag;
use nix::sys::mman::{mmap, munmap, shm_open, shm_unlink, MapFlags, ProtFlags};
use nix::sys::stat::{fstat, Mode};
use nix::unistd::{close, ftruncate};

use crate::ShmemError;

#[derive(Clone, Default)]
pub struct ShmemConfExt;

pub struct MapData {
    //On linux, you must shm_unlink() the object created for the mapping. It wont disappear automatically.
    owner: bool,

    //File descriptor to our open mapping
    map_fd: RawFd,

    //Shared mapping uid
    pub unique_id: String,
    //Total size of the mapping
    pub map_size: usize,
    //Pointer to the first address of our mapping
    pub map_ptr: *mut u8,
    //Whether this mapping uses tmpfs (true) or shm_open (false)
    is_tmpfs: bool,
}

impl MapData {
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.map_ptr
    }
}

/// Shared memory teardown for linux
impl Drop for MapData {
    ///Takes care of properly closing the SharedMem (munmap(), shmem_unlink(), close())
    fn drop(&mut self) {
        //Unmap memory
        if !self.map_ptr.is_null() {
            trace!(
                "munmap(map_ptr:{:p},map_size:{})",
                self.map_ptr,
                self.map_size
            );
            if let Err(_e) = unsafe { munmap(self.map_ptr as *mut _, self.map_size) } {
                debug!("Failed to munmap() shared memory mapping : {}", _e);
            };
        }

        //Unlink shmem
        if self.map_fd != 0 {
            //unlink shmem if we created it
            if self.owner {
                debug!("Deleting persistent mapping");
                if self.is_tmpfs {
                    // tmpfs mode: remove file
                    trace!("remove_file({})", self.unique_id.as_str());
                    if let Err(_e) = std::fs::remove_file(&self.unique_id) {
                        debug!("Failed to remove tmpfs file {} : {}", self.unique_id, _e);
                    };
                } else {
                    // shm_open mode: use shm_unlink
                    trace!("shm_unlink({})", self.unique_id.as_str());
                    if let Err(_e) = shm_unlink(self.unique_id.as_str()) {
                        debug!("Failed to shm_unlink() shared memory : {}", _e);
                    };
                }
            }

            trace!("close({})", self.map_fd);
            if let Err(_e) = close(self.map_fd) {
                debug!(
                    "os_impl::Linux : Failed to close() shared memory file descriptor : {}",
                    _e
                );
            };
        }
    }
}

impl MapData {
    pub fn set_owner(&mut self, is_owner: bool) -> bool {
        let prev_val = self.owner;
        self.owner = is_owner;
        prev_val
    }
}

/// Creates a mapping specified by the uid and size
pub fn create_mapping(
    unique_id: &str,
    map_size: usize,
    mode: Option<Mode>,
) -> Result<MapData, ShmemError> {
    //Create shared memory file descriptor
    debug!("Creating persistent mapping at {}", unique_id);

    let nz_map_size = NonZeroUsize::new(map_size).ok_or(ShmemError::MapSizeZero)?;
    let mode = mode.unwrap_or(Mode::S_IRUSR | Mode::S_IWUSR);
    let shmem_fd = match shm_open(
        unique_id, //Unique name that usualy pops up in /dev/shm/
        OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR, //create exclusively (error if collision) and read/write to allow resize
        mode,                                           // default permission allow user+rw
    ) {
        Ok(v) => {
            trace!(
                "shm_open({}, {:X}, {:X}) == {}",
                unique_id,
                OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
                mode,
                v
            );
            v
        }
        Err(nix::Error::EEXIST) => return Err(ShmemError::MappingIdExists),
        Err(e) => return Err(ShmemError::MapCreateFailed(e as u32)),
    };

    let mut new_map: MapData = MapData {
        owner: true,
        unique_id: String::from(unique_id),
        map_fd: shmem_fd,
        map_size,
        map_ptr: null_mut(),
        is_tmpfs: false,
    };

    //Enlarge the memory descriptor file size to the requested map size
    debug!("Creating memory mapping");
    trace!("ftruncate({}, {})", new_map.map_fd, new_map.map_size);
    match ftruncate(new_map.map_fd, new_map.map_size as _) {
        Ok(_) => {}
        Err(e) => return Err(ShmemError::UnknownOsError(e as u32)),
    };

    //Put the mapping in our address space
    debug!("Loading mapping into address space");
    new_map.map_ptr = match unsafe {
        mmap(
            None,                                         //Desired addr
            nz_map_size,                                  //size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, //Permissions on pages
            MapFlags::MAP_SHARED,                         //What kind of mapping
            new_map.map_fd,                               //fd
            0,                                            //Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                new_map.map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                new_map.map_fd,
                v
            );
            v as *mut _
        }
        Err(e) => return Err(ShmemError::MapCreateFailed(e as u32)),
    };

    Ok(new_map)
}

/// Opens an existing mapping specified by its uid
pub fn open_mapping(
    unique_id: &str,
    _map_size: usize,
    _ext: &ShmemConfExt,
) -> Result<MapData, ShmemError> {
    //Open shared memory
    debug!("Openning persistent mapping at {}", unique_id);
    let shmem_fd = match shm_open(
        unique_id,
        OFlag::O_RDWR, //Open read write
        Mode::S_IRUSR,
    ) {
        Ok(v) => {
            trace!(
                "shm_open({}, {:X}, {:X}) == {}",
                unique_id,
                OFlag::O_RDWR,
                Mode::S_IRUSR,
                v
            );
            v
        }
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    let mut new_map: MapData = MapData {
        owner: false,
        unique_id: String::from(unique_id),
        map_fd: shmem_fd,
        map_size: 0,
        map_ptr: null_mut(),
        is_tmpfs: false,
    };

    //Get mmap size
    new_map.map_size = match fstat(new_map.map_fd) {
        Ok(v) => v.st_size as usize,
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    let nz_map_size = NonZeroUsize::new(new_map.map_size).ok_or(ShmemError::MapSizeZero)?;

    //Map memory into our address space
    debug!("Loading mapping into address space");
    new_map.map_ptr = match unsafe {
        mmap(
            None,                                         //Desired addr
            nz_map_size,                                  //size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, //Permissions on pages
            MapFlags::MAP_SHARED,                         //What kind of mapping
            new_map.map_fd,                               //fd
            0,                                            //Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                new_map.map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                new_map.map_fd,
                v
            );
            v as *mut _
        }
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    Ok(new_map)
}

/// Creates a mapping using tmpfs file
pub fn create_mapping_tmpfs(
    file_path: &str,
    map_size: usize,
    mode: Option<Mode>,
) -> Result<MapData, ShmemError> {
    let nz_map_size = NonZeroUsize::new(map_size).ok_or(ShmemError::MapSizeZero)?;
    let mode_bits = mode.unwrap_or(Mode::S_IRUSR | Mode::S_IWUSR).bits();

    debug!("Creating tmpfs mapping at {}", file_path);

    // Create the file
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(mode_bits.into())
        .open(file_path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => ShmemError::MappingIdExists,
            _ => ShmemError::MapCreateFailed(e.raw_os_error().unwrap_or(0) as u32),
        })?;

    let fd = file.as_raw_fd();

    // Set file size
    trace!("ftruncate({}, {})", fd, map_size);
    match ftruncate(fd, map_size as _) {
        Ok(_) => {}
        Err(e) => return Err(ShmemError::UnknownOsError(e as u32)),
    }

    // Map the file into memory
    debug!("Loading tmpfs mapping into address space");
    let map_ptr = match unsafe {
        mmap(
            None,                                         // Desired addr
            nz_map_size,                                  // Size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, // Permissions on pages
            MapFlags::MAP_SHARED,                         // What kind of mapping
            fd,                                           // File descriptor
            0,                                            // Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                fd,
                v
            );
            v as *mut u8
        }
        Err(e) => return Err(ShmemError::MapCreateFailed(e as u32)),
    };

    // Prevent file from being dropped and closed
    std::mem::forget(file);

    Ok(MapData {
        owner: true,
        unique_id: String::from(file_path),
        map_fd: fd,
        map_size,
        map_ptr,
        is_tmpfs: true,
    })
}

/// Opens an existing tmpfs mapping
pub fn open_mapping_tmpfs(file_path: &str, _expected_size: usize) -> Result<MapData, ShmemError> {
    use std::os::unix::io::AsRawFd;

    debug!("Opening tmpfs mapping at {}", file_path);

    // Open the file
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(file_path)
        .map_err(|e| ShmemError::MapOpenFailed(e.raw_os_error().unwrap_or(0) as u32))?;

    let fd = file.as_raw_fd();

    // Get file size
    let map_size = match fstat(fd) {
        Ok(v) => v.st_size as usize,
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    let nz_map_size = NonZeroUsize::new(map_size).ok_or(ShmemError::MapSizeZero)?;

    // Map the file into memory
    debug!("Loading tmpfs mapping into address space");
    let map_ptr = match unsafe {
        mmap(
            None,                                         // Desired addr
            nz_map_size,                                  // Size of mapping
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE, // Permissions on pages
            MapFlags::MAP_SHARED,                         // What kind of mapping
            fd,                                           // File descriptor
            0,                                            // Offset into fd
        )
    } {
        Ok(v) => {
            trace!(
                "mmap(NULL, {}, {:X}, {:X}, {}, 0) == {:p}",
                map_size,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                fd,
                v
            );
            v as *mut u8
        }
        Err(e) => return Err(ShmemError::MapOpenFailed(e as u32)),
    };

    // Prevent file from being dropped and closed
    std::mem::forget(file);

    Ok(MapData {
        owner: false,
        unique_id: String::from(file_path),
        map_fd: fd,
        map_size,
        map_ptr,
        is_tmpfs: true,
    })
}

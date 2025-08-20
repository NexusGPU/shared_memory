//! A thin wrapper around shared memory system calls
//!
//! For help on how to get started, take a look at the [examples](https://github.com/elast0ny/shared_memory-rs/tree/master/examples) !

use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};

use std::fs::remove_file;
use std::path::{Path, PathBuf};

use cfg_if::cfg_if;

#[cfg(not(target_os = "windows"))]
pub use nix::sys::stat::Mode;

cfg_if! {
    if #[cfg(feature = "logging")] {
        pub use log;
    } else {
        #[allow(unused_macros)]
        mod log {
            macro_rules! trace (($($tt:tt)*) => {{}});
            macro_rules! debug (($($tt:tt)*) => {{}});
            macro_rules! info (($($tt:tt)*) => {{}});
            macro_rules! warn (($($tt:tt)*) => {{}});
            macro_rules! error (($($tt:tt)*) => {{}});
            pub(crate) use {debug, trace};
        }
    }
}

use crate::log::*;

mod error;
pub use error::*;

//Load up the proper OS implementation
cfg_if! {
    if #[cfg(target_os="windows")] {
        mod windows;
        use windows as os_impl;
    } else if #[cfg(any(target_os="freebsd", target_os="linux", target_os="macos"))] {
        mod unix;
        use crate::unix as os_impl;
    } else {
        compile_error!("shared_memory isnt implemented for this platform...");
    }
}

#[derive(Clone, Default)]
/// Struct used to configure different parameters before creating a shared memory mapping
pub struct ShmemConf {
    owner: bool,
    os_id: Option<String>,
    overwrite_flink: bool,
    flink_path: Option<PathBuf>,
    size: usize,
    ext: os_impl::ShmemConfExt,
    #[cfg(not(target_os = "windows"))]
    mode: Option<Mode>,
    use_tmpfs: bool,
    tmpfs_base_dir: Option<PathBuf>,
}

impl Drop for ShmemConf {
    fn drop(&mut self) {
        // Delete the flink if we are the owner of the mapping
        if self.owner {
            if let Some(flink_path) = self.flink_path.as_ref() {
                debug!("Deleting file link {}", flink_path.to_string_lossy());
                let _ = remove_file(flink_path);
            }
        }
    }
}

impl ShmemConf {
    /// Create a new default shmem config
    pub fn new() -> Self {
        ShmemConf::default()
    }
    /// Provide a specific os identifier for the mapping
    ///
    /// When not specified, a randomly generated identifier will be used
    pub fn os_id<S: AsRef<str>>(mut self, os_id: S) -> Self {
        self.os_id = Some(String::from(os_id.as_ref()));
        self
    }

    /// Overwrites file links if it already exist when calling `create()`
    pub fn force_create_flink(mut self) -> Self {
        self.overwrite_flink = true;
        self
    }

    /// Create the shared memory mapping with a file link
    ///
    /// This creates a file on disk that contains the unique os_id for the mapping.
    /// This can be useful when application want to rely on filesystems to share mappings
    pub fn flink<S: AsRef<Path>>(mut self, path: S) -> Self {
        self.flink_path = Some(PathBuf::from(path.as_ref()));
        self
    }

    /// Sets the size of the mapping that will be used in `create()`
    pub fn size(mut self, size: usize) -> Self {
        self.size = size;
        self
    }

    /// Sets the mode of the mapping that will be used in `create()`
    #[cfg(not(target_os = "windows"))]
    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Enable tmpfs mode with a specific base directory
    ///
    /// This will use regular files in the specified directory instead of POSIX shared memory
    #[cfg(not(target_os = "windows"))]
    pub fn use_tmpfs_with_dir<P: AsRef<Path>>(mut self, base_dir: P) -> Self {
        self.use_tmpfs = true;
        self.tmpfs_base_dir = Some(PathBuf::from(base_dir.as_ref()));
        self
    }

    /// Get the tmpfs file path for this configuration
    fn get_tmpfs_file_path(&self) -> Result<PathBuf, ShmemError> {
        if !self.use_tmpfs {
            return Err(ShmemError::NotInTmpfsMode);
        }

        let base_dir = self
            .tmpfs_base_dir
            .as_ref()
            .ok_or(ShmemError::NoTmpfsBaseDir)?;

        if let Some(ref os_id) = self.os_id {
            // Use os_id as filename
            Ok(base_dir.join(format!("shmem_{os_id}")))
        } else {
            // Generate random filename
            Ok(base_dir.join(format!("shmem_{:X}", rand::random::<u64>())))
        }
    }

    /// Create a new mapping using the current configuration
    pub fn create(mut self) -> Result<Shmem, ShmemError> {
        if self.size == 0 {
            return Err(ShmemError::MapSizeZero);
        }

        if let Some(ref flink_path) = self.flink_path {
            if !self.overwrite_flink && flink_path.is_file() {
                return Err(ShmemError::LinkExists);
            }
        }

        // Create the mapping
        let mapping = if cfg!(not(target_os = "windows")) && self.use_tmpfs {
            // tmpfs mode
            if self.os_id.is_some() {
                // Use specified os_id
                let tmpfs_file_path = self.get_tmpfs_file_path()?;
                os_impl::create_mapping_tmpfs(
                    tmpfs_file_path
                        .to_str()
                        .ok_or(ShmemError::UnknownOsError(0))?,
                    self.size,
                    #[cfg(not(target_os = "windows"))]
                    self.mode,
                )?
            } else {
                // Generate random filename until one works
                loop {
                    let random_path = self.get_tmpfs_file_path()?;
                    match os_impl::create_mapping_tmpfs(
                        random_path.to_str().ok_or(ShmemError::UnknownOsError(0))?,
                        self.size,
                        #[cfg(not(target_os = "windows"))]
                        self.mode,
                    ) {
                        Err(ShmemError::MappingIdExists) => continue,
                        Ok(m) => break m,
                        Err(e) => return Err(e),
                    }
                }
            }
        } else {
            // shm_open mode
            match self.os_id {
                None => {
                    // Generate random ID until one works
                    loop {
                        let cur_id = format!("/shmem_{:X}", rand::random::<u64>());
                        match os_impl::create_mapping(
                            &cur_id,
                            self.size,
                            #[cfg(not(target_os = "windows"))]
                            self.mode,
                        ) {
                            Err(ShmemError::MappingIdExists) => continue,
                            Ok(m) => break m,
                            Err(e) => return Err(e),
                        }
                    }
                }
                Some(ref specific_id) => os_impl::create_mapping(
                    specific_id,
                    self.size,
                    #[cfg(not(target_os = "windows"))]
                    self.mode,
                )?,
            }
        };

        debug!("Created shared memory mapping '{}'", mapping.unique_id);

        // Create flink
        if let Some(ref flink_path) = self.flink_path {
            debug!("Creating file link that points to mapping");
            let mut open_options: OpenOptions = OpenOptions::new();
            open_options.write(true);

            if self.overwrite_flink {
                open_options.create(true).truncate(true);
            } else {
                open_options.create_new(true);
            }

            match open_options.open(flink_path) {
                Ok(mut f) => {
                    // write the mapping identifier
                    if let Err(e) = f.write(mapping.unique_id.as_bytes()) {
                        let _ = std::fs::remove_file(flink_path);
                        return Err(ShmemError::LinkWriteFailed(e));
                    }
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    return Err(ShmemError::LinkExists)
                }
                Err(e) => return Err(ShmemError::LinkCreateFailed(e)),
            }

            debug!(
                "Created file link '{}' with id '{}'",
                flink_path.to_string_lossy(),
                mapping.unique_id
            );
        }

        self.owner = true;
        self.size = mapping.map_size;

        Ok(Shmem {
            config: self,
            mapping,
        })
    }

    /// Opens an existing mapping using the current configuration
    pub fn open(mut self) -> Result<Shmem, ShmemError> {
        // Must at least have a flink or an os_id (except in tmpfs mode where we might infer the path)
        if self.flink_path.is_none()
            && self.os_id.is_none()
            && !cfg!(not(target_os = "windows"))
            && self.use_tmpfs
        {
            debug!("Open called with no file link or unique id...");
            return Err(ShmemError::NoLinkOrOsId);
        }

        let mut flink_content = String::new();
        let mut retry = 0;

        loop {
            let target_identifier: Cow<str> = if let Some(ref unique_id) = self.os_id {
                retry = 5;
                if cfg!(not(target_os = "windows")) && self.use_tmpfs {
                    // tmpfs mode: convert os_id to file path
                    let tmpfs_path = self.get_tmpfs_file_path()?;
                    Cow::Owned(tmpfs_path.to_string_lossy().into_owned())
                } else {
                    // shm_open mode: use os_id directly
                    unique_id.as_str().into()
                }
            } else if let Some(ref flink_path) = self.flink_path {
                // Read from flink file
                debug!(
                    "Open shared memory from file link {}",
                    flink_path.to_string_lossy()
                );
                let mut f = File::open(flink_path).map_err(ShmemError::LinkOpenFailed)?;
                flink_content.clear();
                f.read_to_string(&mut flink_content)
                    .map_err(ShmemError::LinkReadFailed)?;
                flink_content.as_str().into()
            } else {
                return Err(ShmemError::NoLinkOrOsId);
            };

            let mapping_result = {
                if cfg!(not(target_os = "windows")) && self.use_tmpfs {
                    // tmpfs mode: target_identifier is a file path
                    os_impl::open_mapping_tmpfs(&target_identifier, self.size)
                } else {
                    // shm_open mode: target_identifier is shm ID
                    os_impl::open_mapping(&target_identifier, self.size, &self.ext)
                }
            };

            match mapping_result {
                Ok(m) => {
                    self.size = m.map_size;
                    self.owner = false;

                    return Ok(Shmem {
                        config: self,
                        mapping: m,
                    });
                }
                // If we got this failing from the flink, try again in case the owner didn't write the full
                // identifier to the file yet
                Err(ShmemError::MapOpenFailed(_)) if self.os_id.is_none() && retry < 5 => {
                    retry += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Structure used to extract information from an existing shared memory mapping
pub struct Shmem {
    config: ShmemConf,
    mapping: os_impl::MapData,
}
#[allow(clippy::len_without_is_empty)]
impl Shmem {
    /// Returns whether we created the mapping or not
    pub fn is_owner(&self) -> bool {
        self.config.owner
    }
    /// Allows for gaining/releasing ownership of the mapping
    ///
    /// Warning : You must ensure at least one process owns the mapping in order to ensure proper cleanup code is ran
    pub fn set_owner(&mut self, is_owner: bool) -> bool {
        self.mapping.set_owner(is_owner);

        let prev_val = self.config.owner;
        self.config.owner = is_owner;
        prev_val
    }
    /// Returns the OS unique identifier for the mapping
    pub fn get_os_id(&self) -> &str {
        self.mapping.unique_id.as_str()
    }

    /// Returns the tmpfs path if present
    #[cfg(not(target_os = "windows"))]
    pub fn get_tmpfs_file_path(&self) -> Option<PathBuf> {
        if self.config.use_tmpfs {
            self.config.get_tmpfs_file_path().ok()
        } else {
            None
        }
    }

    /// Returns the flink path if present
    pub fn get_flink_path(&self) -> Option<&PathBuf> {
        self.config.flink_path.as_ref()
    }
    /// Returns the total size of the mapping
    pub fn len(&self) -> usize {
        self.mapping.map_size
    }
    /// Returns a raw pointer to the mapping
    pub fn as_ptr(&self) -> *mut u8 {
        self.mapping.as_mut_ptr()
    }
    /// Returns mapping as a byte slice
    /// # Safety
    /// This function is unsafe because it is impossible to ensure the range of bytes is immutable
    pub unsafe fn as_slice(&self) -> &[u8] {
        std::slice::from_raw_parts(self.as_ptr(), self.len())
    }
    /// Returns mapping as a mutable byte slice
    /// # Safety
    /// This function is unsafe because it is impossible to ensure the returned mutable refence is unique/exclusive
    pub unsafe fn as_slice_mut(&mut self) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.as_ptr(), self.len())
    }
}

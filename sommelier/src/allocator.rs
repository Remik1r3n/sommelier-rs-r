/*
Copyright 2026 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

//! Host buffer allocation, merged from the two upstream branches.
//!
//! `main` used a sealed `memfd` (no `/dev/dri` dependency, immune to the
//! SIGBUS-on-truncate class of bugs); `virtwl` used a GBM BO (zero-copy when a
//! render node exists). Both are useful, so this module prefers GBM and falls
//! back to sealed memfd. Callers only see the unified [`AllocatedBuffer`],
//! which keeps whichever backing object alive for the buffer's lifetime.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::state::BufferBacking;

/// Converts a Wayland `wl_shm` format enum to bytes-per-pixel.
fn shm_bpp(format: u32) -> io::Result<u32> {
    // WL_SHM_FORMAT_ARGB8888 = 0, WL_SHM_FORMAT_XRGB8888 = 1.
    match format {
        0 | 1 => Ok(4),
        // Fourcc values are passed through by the dmabuf path.
        v if v > 0xff => Ok(4),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Unsupported Wayland SHM format: {}", format),
        )),
    }
}

pub struct AllocatedBuffer {
    /// The fd handed to the host compositor.
    pub fd: OwnedFd,
    pub stride: u32,
    pub size: usize,
    pub modifier: u64,
    /// Keeps the underlying allocation alive (GBM BO or the memfd itself).
    pub backing: BufferBacking,
}

pub struct Allocator {
    gbm: Option<gbm::Device<std::fs::File>>,
}

impl Allocator {
    /// Never fails hard: if there is no usable render node we simply run in
    /// memfd mode. This matters on Crostini guests where `/dev/dri/renderD128`
    /// is often absent while `/dev/wl0` is present.
    pub fn new() -> io::Result<Self> {
        Self::with_device(None)
    }

    /// `forced` implements mainline `--force-drm-device=PATH`: use exactly
    /// that render node instead of enumerating `/dev/dri/renderD128..191`.
    pub fn with_device(forced: Option<&str>) -> io::Result<Self> {
        let gbm = Self::open_gbm(forced);
        if gbm.is_none() {
            log::info!("No GBM render node available; using sealed memfd SHM fallback");
        }
        Ok(Self { gbm })
    }

    fn open_gbm(forced: Option<&str>) -> Option<gbm::Device<std::fs::File>> {
        // Match the mainline sommelier behaviour of enumerating render nodes
        // rather than hard-coding a single one.
        let candidates: Vec<String> = match forced {
            Some(path) => vec![path.to_string()],
            None => (128..=191)
                .map(|index| format!("/dev/dri/renderD{}", index))
                .collect(),
        };
        for path in candidates {
            let file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            match gbm::Device::new(file) {
                Ok(dev) => {
                    log::info!("Using GBM device {}", path);
                    return Some(dev);
                }
                Err(e) => log::debug!("{}: not a usable GBM device: {}", path, e),
            }
        }
        None
    }

    pub fn uses_gbm(&self) -> bool {
        self.gbm.is_some()
    }

    /// Allocate a host-shareable buffer. `format` is either a `wl_shm` format
    /// (0/1) or a DRM fourcc (passed through from the dmabuf path).
    pub fn allocate(&self, width: u32, height: u32, format: u32) -> io::Result<AllocatedBuffer> {
        if let Some(dev) = &self.gbm {
            match self.allocate_gbm(dev, width, height, format) {
                Ok(buf) => return Ok(buf),
                Err(e) => log::warn!(
                    "GBM allocation failed ({}x{}, format={:#x}): {}; falling back to memfd",
                    width,
                    height,
                    format,
                    e
                ),
            }
        }
        self.allocate_memfd(width, height, format)
    }

    fn allocate_gbm(
        &self,
        dev: &gbm::Device<std::fs::File>,
        width: u32,
        height: u32,
        format: u32,
    ) -> io::Result<AllocatedBuffer> {
        use gbm::{BufferObjectFlags, Format as GbmFormat};

        let gbm_format = match format {
            0 => GbmFormat::Argb8888,
            1 => GbmFormat::Xrgb8888,
            v if v > 0xff => GbmFormat::try_from(v)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?,
            v => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unsupported format: {}", v),
                ))
            }
        };

        let bo = dev
            .create_buffer_object(
                width,
                height,
                gbm_format,
                BufferObjectFlags::RENDERING | BufferObjectFlags::LINEAR,
            )
            .map_err(io::Error::other)?;

        let stride = bo.stride().unwrap_or(width * shm_bpp(format)?);
        let size = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "buffer size overflow"))?;
        let modifier = bo.modifier().map(u64::from).unwrap_or(0);

        let fd = bo.fd().map_err(io::Error::other)?;
        let dup = nix::unistd::dup(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd.as_raw_fd()) })
            .map_err(io::Error::from)?;

        Ok(AllocatedBuffer {
            fd: dup,
            stride,
            size,
            modifier,
            backing: BufferBacking::Gbm(bo),
        })
    }

    /// `main` branch allocator: memfd with `F_SEAL_SHRINK|GROW|SEAL` so a
    /// malicious/buggy client cannot truncate the mapping and cause SIGBUS.
    fn allocate_memfd(&self, width: u32, height: u32, format: u32) -> io::Result<AllocatedBuffer> {
        let bpp = shm_bpp(format)?;
        let stride = width
            .checked_mul(bpp)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "width overflow"))?;
        let size = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "size overflow"))?;

        let memfd_name = b"sommelier-rs-r-shm\0";
        let raw_fd = unsafe {
            libc::memfd_create(
                memfd_name.as_ptr() as *const libc::c_char,
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

        nix::unistd::ftruncate(&fd, size as i64)?;

        let seals = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, seals) } < 0 {
            return Err(io::Error::last_os_error());
        }

        // The same fd is both the mapping source and the host-shareable handle.
        let dup = nix::unistd::dup(unsafe { std::os::fd::BorrowedFd::borrow_raw(fd.as_raw_fd()) })
            .map_err(io::Error::from)?;

        Ok(AllocatedBuffer {
            fd: dup,
            stride,
            size,
            modifier: 0,
            backing: BufferBacking::Memfd(fd),
        })
    }
}

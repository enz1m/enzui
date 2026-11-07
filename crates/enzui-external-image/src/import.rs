//! Importing a DMA-BUF as a texture on the renderer's Vulkan device.

use crate::{DmaBuf, DrmDevice, Refusal, Support, fourcc};
use ash::vk;
use iced_wgpu::wgpu;
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use wgpu::hal::api::Vulkan;

/// The Vulkan and wgpu formats a DRM format samples as, and whether its
/// alpha is to be ignored.
fn formats(format: u32) -> Option<(vk::Format, wgpu::TextureFormat, bool)> {
    match format {
        fourcc::ARGB8888 => Some((
            vk::Format::B8G8R8A8_UNORM,
            wgpu::TextureFormat::Bgra8Unorm,
            false,
        )),
        fourcc::XRGB8888 => Some((
            vk::Format::B8G8R8A8_UNORM,
            wgpu::TextureFormat::Bgra8Unorm,
            true,
        )),
        fourcc::ABGR8888 => Some((
            vk::Format::R8G8B8A8_UNORM,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        )),
        fourcc::XBGR8888 => Some((
            vk::Format::R8G8B8A8_UNORM,
            wgpu::TextureFormat::Rgba8Unorm,
            true,
        )),
        _ => None,
    }
}

const FORMATS: [u32; 4] = [
    fourcc::ARGB8888,
    fourcc::XRGB8888,
    fourcc::ABGR8888,
    fourcc::XBGR8888,
];

/// What `device` imports, or `None` when it imports nothing.
pub(crate) fn support(device: &wgpu::Device) -> Option<Support> {
    if !iced_wgpu::imports_dma_buf(device) {
        return None;
    }
    #[allow(unsafe_code, reason = "querying Vulkan format and image capabilities")]
    // SAFETY: only queries: the instance and physical device come from the
    // live hal device, and every output structure is owned here.
    unsafe {
        let hal = device.as_hal::<Vulkan>()?;
        let instance = hal.shared_instance().raw_instance();
        let physical = hal.raw_physical_device();
        let mut max_extent = 0;
        let mut supported = Vec::new();
        for format in FORMATS {
            let (vk_format, _, _) = formats(format)?;
            let mut modifiers = Vec::new();
            for (modifier, planes) in format_modifiers(instance, physical, vk_format) {
                if planes != 1 {
                    continue;
                }
                if let Some(extent) = importable(instance, physical, vk_format, modifier) {
                    max_extent = max_extent.max(extent);
                    modifiers.push(modifier);
                }
            }
            if !modifiers.is_empty() {
                supported.push((format, modifiers));
            }
        }
        (!supported.is_empty()).then(|| Support {
            formats: supported,
            max_extent,
            device: drm_device(instance, physical),
        })
    }
}

/// The DRM device `physical` is, when it supports `VK_EXT_physical_device_drm`.
///
/// # Safety
/// `instance` and `physical` must be live.
#[allow(
    unsafe_code,
    reason = "querying Vulkan device extensions and properties"
)]
unsafe fn drm_device(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<DrmDevice> {
    // SAFETY: queries on a live physical device into owned structures.
    unsafe {
        let extensions = instance
            .enumerate_device_extension_properties(physical)
            .ok()?;
        let supported = extensions.iter().any(|extension| {
            extension.extension_name_as_c_str() == Ok(vk::EXT_PHYSICAL_DEVICE_DRM_NAME)
        });
        if !supported {
            return None;
        }
        let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
        instance.get_physical_device_properties2(physical, &mut properties);
        crate::device::from_vulkan(
            drm.has_primary == vk::TRUE,
            (drm.primary_major, drm.primary_minor),
            drm.has_render == vk::TRUE,
            (drm.render_major, drm.render_minor),
        )
    }
}

/// The modifiers the device samples `format` with, and their plane counts.
///
/// # Safety
/// `instance` and `physical` must be live.
#[allow(unsafe_code, reason = "querying Vulkan format properties")]
unsafe fn format_modifiers(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    format: vk::Format,
) -> Vec<(u64, u32)> {
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
    // SAFETY: the first call fills only the count.
    unsafe {
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        instance.get_physical_device_format_properties2(physical, format, &mut properties);
    }
    let mut entries = vec![
        vk::DrmFormatModifierPropertiesEXT::default();
        list.drm_format_modifier_count as usize
    ];
    let mut list = vk::DrmFormatModifierPropertiesListEXT::default()
        .drm_format_modifier_properties(&mut entries);
    // SAFETY: `entries` has the room the first call reported.
    unsafe {
        let mut properties = vk::FormatProperties2::default().push_next(&mut list);
        instance.get_physical_device_format_properties2(physical, format, &mut properties);
    }
    let count = list.drm_format_modifier_count as usize;
    entries
        .into_iter()
        .take(count)
        .filter(|entry| {
            entry
                .drm_format_modifier_tiling_features
                .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
        })
        .map(|entry| {
            (
                entry.drm_format_modifier,
                entry.drm_format_modifier_plane_count,
            )
        })
        .collect()
}

/// The largest extent an imported DMA-BUF image of `format` and `modifier`
/// can have, or `None` when such an image cannot be imported and sampled.
///
/// # Safety
/// `instance` and `physical` must be live.
#[allow(unsafe_code, reason = "querying Vulkan image format properties")]
unsafe fn importable(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    format: vk::Format,
    modifier: u64,
) -> Option<u32> {
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .push_next(&mut modifier_info)
        .push_next(&mut external_info);
    let mut external = vk::ExternalImageFormatProperties::default();
    let extent = {
        let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external);
        // SAFETY: every structure in both chains lives to the end of the call.
        unsafe {
            instance
                .get_physical_device_image_format_properties2(physical, &info, &mut properties)
                .ok()?;
        }
        properties.image_format_properties.max_extent
    };
    let importable = external
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE);
    importable.then_some(extent.width.min(extent.height))
}

/// A DMA-BUF imported as a texture.
#[derive(Debug)]
pub(crate) struct Imported {
    pub(crate) view: wgpu::TextureView,
    /// Whether the format's alpha is to be ignored.
    pub(crate) opaque: bool,
    /// The size of the imported memory, as the device requires it.
    pub(crate) bytes: u64,
    /// Kept so the texture, and through it the image and its memory, live
    /// as long as the import.
    _texture: wgpu::Texture,
}

impl Drop for Imported {
    /// Counted once the last reference goes: after the GPU work that sampled
    /// it, which the pipeline waits for before letting go.
    fn drop(&mut self) {
        crate::stats::import_released(self.bytes);
    }
}

/// Imports `buffer` on `device` as a sampled texture.
pub(crate) fn import(
    device: &wgpu::Device,
    support: &Support,
    buffer: &DmaBuf,
) -> Result<Imported, Refusal> {
    let (vk_format, wgpu_format, opaque) =
        formats(buffer.format).ok_or(Refusal::Format(buffer.format))?;
    if !support.modifiers(buffer.format).contains(&buffer.modifier) {
        return Err(Refusal::Modifier {
            format: buffer.format,
            modifier: buffer.modifier,
        });
    }
    if buffer.planes.len() != 1 {
        return Err(Refusal::Planes {
            given: buffer.planes.len(),
            expected: 1,
        });
    }
    if buffer.width == 0
        || buffer.height == 0
        || buffer.width > support.max_extent
        || buffer.height > support.max_extent
    {
        return Err(Refusal::Size {
            width: buffer.width,
            height: buffer.height,
        });
    }
    let plane = &buffer.planes[0];
    // Vulkan takes the descriptor it imports: hand it a duplicate.
    let fd = plane
        .fd
        .try_clone()
        .map_err(|error| Refusal::Driver(format!("duplicating the descriptor: {error}")))?;

    #[allow(unsafe_code, reason = "creating and importing a Vulkan image")]
    // SAFETY: the image is created on the live device with the external
    // memory and explicit modifier the buffer declares; the memory is
    // imported from a descriptor this call owns, bound once, and freed with
    // the image by the drop callback wgpu-hal runs when the texture is
    // destroyed. Every failure path destroys what it created.
    let (hal_texture, bytes) = unsafe {
        let hal = device.as_hal::<Vulkan>().ok_or(Refusal::NoImport)?;
        let raw = hal.raw_device().clone();
        let instance = hal.shared_instance().raw_instance().clone();
        let layouts = [vk::SubresourceLayout {
            offset: u64::from(plane.offset),
            size: 0,
            row_pitch: u64::from(plane.stride),
            array_pitch: 0,
            depth_pitch: 0,
        }];
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(buffer.modifier)
            .plane_layouts(&layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: buffer.width,
                height: buffer.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external)
            .push_next(&mut explicit);
        let image = raw
            .create_image(&info, None)
            .map_err(|error| Refusal::Driver(format!("creating the image: {error}")))?;

        let (memory, bytes) =
            match import_memory(&instance, &raw, hal.raw_physical_device(), image, fd) {
                Ok(imported) => imported,
                Err(refusal) => {
                    raw.destroy_image(image, None);
                    return Err(refusal);
                }
            };
        if let Err(error) = raw.bind_image_memory(image, memory, 0) {
            raw.free_memory(memory, None);
            raw.destroy_image(image, None);
            return Err(Refusal::Driver(format!("binding the memory: {error}")));
        }

        let descriptor = wgpu::hal::TextureDescriptor {
            label: Some("enzui external image"),
            size: wgpu::Extent3d {
                width: buffer.width,
                height: buffer.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        let destroy = raw;
        let hal_texture = hal.texture_from_raw(
            image,
            &descriptor,
            // wgpu destroys the texture only once the GPU no longer uses it;
            // the image and memory are this import's alone.
            Some(Box::new(move || {
                destroy.destroy_image(image, None);
                destroy.free_memory(memory, None);
            })),
            wgpu::hal::vulkan::TextureMemory::External,
        );
        (hal_texture, bytes)
    };

    #[allow(unsafe_code, reason = "adopting a wgpu-hal texture")]
    // SAFETY: the hal texture was created on this device for exactly this
    // descriptor's size, format and usage.
    let texture = unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("enzui external image"),
                size: wgpu::Extent3d {
                    width: buffer.width,
                    height: buffer.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    crate::stats::import_made(bytes);
    Ok(Imported {
        view,
        opaque,
        bytes,
        _texture: texture,
    })
}

/// Imports `fd` as dedicated memory for `image`; returns it and its size.
///
/// # Safety
/// `raw` and `image` must be live and `image` unbound.
#[allow(unsafe_code, reason = "importing DMA-BUF memory into Vulkan")]
unsafe fn import_memory(
    instance: &ash::Instance,
    raw: &ash::Device,
    physical: vk::PhysicalDevice,
    image: vk::Image,
    fd: OwnedFd,
) -> Result<(vk::DeviceMemory, u64), Refusal> {
    let external_fd = ash::khr::external_memory_fd::Device::new(instance, raw);
    let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
    // SAFETY: `fd` is a live descriptor this function owns.
    unsafe {
        external_fd
            .get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut fd_properties,
            )
            .map_err(|error| Refusal::Driver(format!("reading the buffer's memory: {error}")))?;
    }
    // SAFETY: `image` is live.
    let requirements = unsafe { raw.get_image_memory_requirements(image) };
    let types = requirements.memory_type_bits & fd_properties.memory_type_bits;
    // SAFETY: a read of the physical device's memory layout.
    let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    let index = (0..memory_properties.memory_type_count)
        .find(|index| types & (1 << index) != 0)
        .ok_or_else(|| Refusal::Driver("no memory type fits the buffer".into()))?;
    let raw_fd = fd.into_raw_fd();
    let mut import = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
        .fd(raw_fd);
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(index)
        .push_next(&mut import)
        .push_next(&mut dedicated);
    // SAFETY: on success Vulkan owns `raw_fd`; on failure it is still ours
    // and is closed here.
    unsafe {
        raw.allocate_memory(&info, None)
            .map(|memory| (memory, requirements.size))
            .map_err(|error| {
                drop(<OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw_fd));
                Refusal::Driver(format!("importing the memory: {error}"))
            })
    }
}

/// Whether a buffer's producer has finished writing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    /// Every plane reported its writes finished.
    Ready,
    /// A plane was still being written when the wait ran out.
    Unfinished,
    /// Polling a plane failed or reported an error, so its writes cannot be
    /// confirmed: the reason.
    Failed(String),
}

/// Waits until every write to `buffer` has landed (its implicit fences), for
/// at most `timeout`. A DMA-BUF polls readable once the producer's GPU writes
/// to it have finished.
pub(crate) fn wait_written(buffer: &DmaBuf, timeout: std::time::Duration) -> Readiness {
    let fds: Vec<std::os::fd::RawFd> = buffer
        .planes
        .iter()
        .map(|plane| {
            let fd: BorrowedFd<'_> = std::os::fd::AsFd::as_fd(&*plane.fd);
            fd.as_raw_fd()
        })
        .collect();
    wait_readable(&fds, timeout, system_poll)
}

/// `poll(2)` over `fds`, its error read from `errno`.
fn system_poll(fds: &mut [libc::pollfd], timeout_ms: i32) -> std::io::Result<i32> {
    let count = libc::nfds_t::try_from(fds.len()).unwrap_or(libc::nfds_t::MAX);
    #[allow(unsafe_code, reason = "polling DMA-BUF descriptors")]
    // SAFETY: `fds` is a live slice of `count` pollfds for the call.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, timeout_ms) };
    if ready < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(ready)
    }
}

/// Waits until every descriptor in `fds` polls readable, through `poll`, for
/// at most `timeout` in all: an interrupted wait resumes with what is left.
/// An error, hang-up or invalid-descriptor flag on any of them fails the
/// wait, readable or not.
pub(crate) fn wait_readable(
    fds: &[std::os::fd::RawFd],
    timeout: std::time::Duration,
    mut poll: impl FnMut(&mut [libc::pollfd], i32) -> std::io::Result<i32>,
) -> Readiness {
    let deadline = std::time::Instant::now() + timeout;
    let mut pending: Vec<libc::pollfd> = fds
        .iter()
        .map(|&fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    loop {
        if pending.is_empty() {
            return Readiness::Ready;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // Rounded up, so a wait with time left never polls without waiting.
        let timeout_ms = i32::try_from(remaining.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
        for fd in &mut pending {
            fd.revents = 0;
        }
        match poll(&mut pending, timeout_ms) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Readiness::Failed(format!("polling the buffer: {error}")),
            Ok(0) => return Readiness::Unfinished,
            Ok(_) => {
                for fd in &pending {
                    let flags = fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL);
                    if flags != 0 {
                        return Readiness::Failed(describe(flags));
                    }
                }
                pending.retain(|fd| fd.revents & libc::POLLIN == 0);
            }
        }
        if !pending.is_empty() && std::time::Instant::now() >= deadline {
            return Readiness::Unfinished;
        }
    }
}

fn describe(flags: libc::c_short) -> String {
    let names: Vec<&str> = [
        (libc::POLLERR, "an error"),
        (libc::POLLHUP, "a hang-up"),
        (libc::POLLNVAL, "an invalid descriptor"),
    ]
    .into_iter()
    .filter(|(flag, _)| flags & flag != 0)
    .map(|(_, name)| name)
    .collect();
    format!("polling the buffer reported {}", names.join(" and "))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test")]

    use super::{Readiness, system_poll, wait_readable};
    use std::io::Write as _;
    use std::os::fd::{AsRawFd as _, OwnedFd};
    use std::time::{Duration, Instant};

    /// A pipe's read and write ends.
    fn pipe() -> (OwnedFd, std::fs::File) {
        let (read, write) = std::io::pipe().unwrap();
        (read.into(), OwnedFd::from(write).into())
    }

    #[test]
    fn a_readable_descriptor_is_ready_and_an_unwritten_one_times_out() {
        let (read, mut write) = pipe();
        let fds = [read.as_raw_fd()];
        let started = Instant::now();
        assert_eq!(
            wait_readable(&fds, Duration::from_millis(30), system_poll),
            Readiness::Unfinished
        );
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(30) && waited < Duration::from_secs(1),
            "{waited:?}"
        );
        write.write_all(b"x").unwrap();
        assert_eq!(
            wait_readable(&fds, Duration::from_millis(30), system_poll),
            Readiness::Ready
        );
    }

    #[test]
    fn every_plane_must_be_ready() {
        let (ready, mut write) = pipe();
        write.write_all(b"x").unwrap();
        let (unwritten, _keep) = pipe();
        assert_eq!(
            wait_readable(
                &[ready.as_raw_fd(), unwritten.as_raw_fd()],
                Duration::from_millis(10),
                system_poll
            ),
            Readiness::Unfinished
        );
    }

    #[test]
    fn a_hang_up_or_an_invalid_descriptor_fails_even_when_readable() {
        let (read, mut write) = pipe();
        write.write_all(b"x").unwrap();
        drop(write);
        // Readable (a byte is waiting) and hung up at once.
        assert!(matches!(
            wait_readable(&[read.as_raw_fd()], Duration::from_millis(10), system_poll),
            Readiness::Failed(reason) if reason.contains("hang-up")
        ));
        let closed = {
            let (read, _write) = pipe();
            read.as_raw_fd()
        };
        assert!(matches!(
            wait_readable(&[closed], Duration::from_millis(10), system_poll),
            Readiness::Failed(reason) if reason.contains("invalid descriptor")
        ));
    }

    #[test]
    fn an_error_flag_fails_the_wait() {
        let poll = |fds: &mut [libc::pollfd], _timeout: i32| {
            fds[0].revents = libc::POLLIN | libc::POLLERR;
            Ok(1)
        };
        assert!(matches!(
            wait_readable(&[0], Duration::from_millis(10), poll),
            Readiness::Failed(reason) if reason.contains("an error")
        ));
        let failing =
            |_: &mut [libc::pollfd], _: i32| Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        assert!(matches!(
            wait_readable(&[0], Duration::from_millis(10), failing),
            Readiness::Failed(_)
        ));
    }

    #[test]
    fn an_interrupted_wait_resumes_within_one_deadline() {
        // Interrupted twice, then readable.
        let mut calls = 0;
        let poll = |fds: &mut [libc::pollfd], _timeout: i32| {
            calls += 1;
            if calls <= 2 {
                return Err(std::io::Error::from_raw_os_error(libc::EINTR));
            }
            fds[0].revents = libc::POLLIN;
            Ok(1)
        };
        assert_eq!(
            wait_readable(&[0], Duration::from_millis(50), poll),
            Readiness::Ready
        );
        // Interrupted for ever: it gives up at the deadline, not later, and
        // never asks for more time than is left.
        let started = Instant::now();
        let deadline = started + Duration::from_millis(30);
        let interrupted = |_: &mut [libc::pollfd], timeout: i32| {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                Duration::from_millis(u64::try_from(timeout).unwrap())
                    <= left + Duration::from_millis(1),
                "{timeout} ms asked with {left:?} left"
            );
            std::thread::sleep(Duration::from_millis(5));
            Err(std::io::Error::from_raw_os_error(libc::EINTR))
        };
        assert_eq!(
            wait_readable(&[0], Duration::from_millis(30), interrupted),
            Readiness::Unfinished
        );
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}

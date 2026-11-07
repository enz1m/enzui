//! Which DRM device the renderer imports on, so a producer can allocate on
//! the same one.

use std::os::fd::{AsRawFd, BorrowedFd};

/// A DRM node's device number: its major and minor.
pub type DrmNode = (u32, u32);

/// A DRM device, named by the device numbers of its nodes. Two handles to
/// the same GPU share a node number however they were opened; device paths,
/// node basenames and PCI ids do not identify a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DrmDevice {
    /// The primary node (`card*`), when the device has one.
    pub primary: Option<DrmNode>,
    /// The render node (`renderD*`), when the device has one.
    pub render: Option<DrmNode>,
}

/// How two devices compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMatch {
    /// They share a node: one device.
    Same,
    /// Both name a node of the same kind, and the numbers differ.
    Different,
    /// Neither side names a node the other can be compared with.
    Unknown,
}

impl DrmDevice {
    /// The device an open DRM node belongs to, from the descriptor alone
    /// (`fstat`; no path is opened). `None` when `node` is not a DRM node.
    /// Whether a character device is a primary or a render node is read
    /// from its minor: render nodes are numbered from 128.
    pub fn of_node(node: BorrowedFd<'_>) -> Option<Self> {
        let number = node_number(node)?;
        let render = number.1 >= 128;
        Some(Self {
            primary: (!render).then_some(number),
            render: render.then_some(number),
        })
    }

    /// Compares with `other`, node kind by node kind.
    pub fn compare(&self, other: &Self) -> DeviceMatch {
        let pairs = [(self.primary, other.primary), (self.render, other.render)];
        let mut compared = false;
        for pair in pairs {
            if let (Some(mine), Some(theirs)) = pair {
                if mine == theirs {
                    return DeviceMatch::Same;
                }
                compared = true;
            }
        }
        if compared {
            DeviceMatch::Different
        } else {
            DeviceMatch::Unknown
        }
    }
}

/// The device number of the DRM character device `node`.
fn node_number(node: BorrowedFd<'_>) -> Option<DrmNode> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    #[allow(unsafe_code, reason = "fstat on a borrowed descriptor")]
    // SAFETY: `node` is open for the borrow and `stat` is a writable stat
    // buffer; it is read only after fstat reports success.
    let stat = unsafe {
        if libc::fstat(node.as_raw_fd(), stat.as_mut_ptr()) != 0 {
            return None;
        }
        stat.assume_init()
    };
    if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
        return None;
    }
    let number = (libc::major(stat.st_rdev), libc::minor(stat.st_rdev));
    // DRM nodes are character devices of major 226 on Linux.
    (number.0 == DRM_MAJOR).then_some(number)
}

const DRM_MAJOR: u32 = 226;

/// The identity `VK_EXT_physical_device_drm` reported, or `None` when the
/// device reported no node.
pub(crate) fn from_vulkan(
    has_primary: bool,
    primary: (i64, i64),
    has_render: bool,
    render: (i64, i64),
) -> Option<DrmDevice> {
    let node = |has: bool, (major, minor): (i64, i64)| {
        has.then(|| Some((u32::try_from(major).ok()?, u32::try_from(minor).ok()?)))
            .flatten()
    };
    let device = DrmDevice {
        primary: node(has_primary, primary),
        render: node(has_render, render),
    };
    (device.primary.is_some() || device.render.is_some()).then_some(device)
}

use std::io::{self, Cursor};

use crate::portal::notification::{MAX_NOTIFICATION_ICON_BYTES, NotificationIcon};

pub(super) fn prepare(icon: &NotificationIcon, version: u32) -> io::Result<ashpd::desktop::Icon> {
    match icon {
        NotificationIcon::Themed(names) => Ok(ashpd::desktop::Icon::with_names(names)),
        NotificationIcon::Bytes(bytes) => {
            validate(bytes)?;
            if version < 2 {
                Ok(ashpd::desktop::Icon::Bytes(bytes.clone()))
            } else {
                sealed_icon(bytes).map(ashpd::desktop::Icon::FileDescriptor)
            }
        }
    }
}

fn invalid_icon() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid notification icon")
}

fn validate(bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_NOTIFICATION_ICON_BYTES {
        return Err(invalid_icon());
    }
    if let Ok(format @ (image::ImageFormat::Png | image::ImageFormat::Jpeg)) =
        image::guess_format(bytes)
    {
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(512);
        limits.max_image_height = Some(512);
        limits.max_alloc = Some(1024 * 1024);
        reader.limits(limits);
        let (width, height) = reader.into_dimensions().map_err(|_| invalid_icon())?;
        if width == 0 || width != height || width > 512 {
            return Err(invalid_icon());
        }
        return Ok(());
    }
    if bytes.len() > 4096 {
        return Err(invalid_icon());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid_icon())?;
    let document = roxmltree::Document::parse(text).map_err(|_| invalid_icon())?;
    // Header preflight must neither open linked files nor decode embedded images.
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_xmltree(&document, &options).map_err(|_| invalid_icon())?;
    if tree.size().width() != tree.size().height() {
        return Err(invalid_icon());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn sealed_icon(bytes: &[u8]) -> io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
    use std::io::{Seek, Write};

    let fd = memfd_create(
        c"enzui-notification-icon",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let mut file = std::fs::File::from(fd);
    file.write_all(bytes)?;
    file.rewind()?;
    fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(file.into())
}

#[cfg(not(target_os = "linux"))]
fn sealed_icon(_bytes: &[u8]) -> io::Result<std::os::fd::OwnedFd> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "sealed notification icons require Linux",
    ))
}

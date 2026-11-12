//! Files a device wrote, and files that install software.

/// Camera-style stems that indicate an unedited capture.
///
/// A prefix list, not the DCF file-name shape: `IMG_`, `DSC`, `PXL_`, `DJI_`
/// and `GOPRO` with a still or video extension a camera writes. The DCF shape
/// (four characters, then four digits) is checked in `sensitive.rs`, where it
/// clears a camera counter that looks like a tax form number; it is a guard
/// there, and it does not decide what counts as a camera file here.
pub fn is_camera(file_name: &str, ext: &str) -> bool {
    let l = file_name.to_ascii_lowercase();
    let camera_stem = l.starts_with("img_")
        || l.starts_with("dsc")
        || l.starts_with("pxl_")
        || l.starts_with("dji_")
        || l.starts_with("gopro");
    camera_stem && matches!(ext, "jpg" | "jpeg" | "heic" | "png" | "raw" | "dng" | "mov")
}

/// Installer images and packages, for macOS and the platforms people download
/// installers for by mistake. `ext` is the scan's extension, already lower
/// case.
pub fn is_installer(ext: &str) -> bool {
    matches!(ext, "dmg" | "pkg" | "msi" | "deb" | "rpm" | "appimage")
}

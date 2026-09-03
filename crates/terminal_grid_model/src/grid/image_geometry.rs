//! R11: 原 warpui::image_cache::{resize_dimensions, FitType} 的纯几何子集,
//! 自 warpui_core/src/image_cache.rs 复制(crate 不引 warpui;逻辑零改动)。
//! FitType 仅保留 ansi_handler 用到的 Contain/Stretch 两型。

/// The algorithm used to map the source rectangle onto the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitType {
    Contain,
    /// Stretches the image to fit the given bounds, ignoring the aspect ratio.
    Stretch,
}

/// Adapted from image::math::utils::resize_dimensions() (via warpui_core).
pub fn resize_dimensions(
    width: u32,
    height: u32,
    nwidth: u32,
    nheight: u32,
    fit_type: FitType,
) -> (u32, u32) {
    use std::cmp::max;

    let wratio = nwidth as f64 / width as f64;
    let hratio = nheight as f64 / height as f64;

    let ratio = match fit_type {
        FitType::Contain => f64::min(wratio, hratio),
        // Stretch doesn't maintain the aspect ratio
        FitType::Stretch => return (nwidth, nheight),
    };

    let nw = max((width as f64 * ratio).round() as u64, 1);
    let nh = max((height as f64 * ratio).round() as u64, 1);

    if nw > u64::from(u32::MAX) {
        let ratio = u32::MAX as f64 / width as f64;
        (u32::MAX, max((height as f64 * ratio).round() as u32, 1))
    } else if nh > u64::from(u32::MAX) {
        let ratio = u32::MAX as f64 / height as f64;
        (max((width as f64 * ratio).round() as u32, 1), u32::MAX)
    } else {
        (nw as u32, nh as u32)
    }
}

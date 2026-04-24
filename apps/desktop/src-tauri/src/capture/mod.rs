use anyhow::Result;
use image::RgbaImage;
use xcap::Monitor;

/// A single captured frame as a raw RGBA image.
pub type Frame = RgbaImage;

/// Capture a single frame from the primary monitor.
pub fn capture_primary() -> Result<Frame> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no monitors found"))?;
    Ok(monitor.capture_image()?)
}

/// Capture a single frame from a monitor identified by index.
pub fn capture_monitor(index: usize) -> Result<Frame> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .into_iter()
        .nth(index)
        .ok_or_else(|| anyhow::anyhow!("monitor index {index} out of range"))?;
    Ok(monitor.capture_image()?)
}

/// List available monitor names.
pub fn list_monitors() -> Result<Vec<String>> {
    let monitors = Monitor::all()?;
    Ok(monitors.iter().map(|m| m.name().to_owned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_monitors_does_not_panic() {
        let _ = list_monitors();
    }
}
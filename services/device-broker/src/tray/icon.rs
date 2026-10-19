//! Shared Windows/Linux/macOS broker LED artwork, independent of platform UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTone {
    Ready,
    Syncing,
    Attention,
    Error,
    Inactive,
}

/// The LED palette shared with the app theme (`--qc-app-status-*`).
pub fn icon_color(tone: IconTone) -> [u8; 4] {
    match tone {
        IconTone::Ready => [64, 196, 118, 255],
        IconTone::Syncing => [91, 164, 231, 255],
        IconTone::Attention => [240, 184, 60, 255],
        IconTone::Error => [226, 73, 73, 255],
        IconTone::Inactive => [140, 145, 151, 255],
    }
}

pub const ICON_SIZE: usize = 32;
pub const SYNC_FRAME_COUNT: u8 = 4;

fn rounded_rect_coverage(x: usize, y: usize, inset: f32, radius: f32) -> f32 {
    let mut covered = 0_u8;
    for sample_y in 0..4 {
        for sample_x in 0..4 {
            let px = x as f32 + (sample_x as f32 + 0.5) / 4.0;
            let py = y as f32 + (sample_y as f32 + 0.5) / 4.0;
            let low = inset + radius;
            let high = ICON_SIZE as f32 - inset - radius;
            let dx = px - px.clamp(low, high);
            let dy = py - py.clamp(low, high);
            if dx * dx + dy * dy <= radius * radius {
                covered += 1;
            }
        }
    }
    covered as f32 / 16.0
}

fn circle_coverage(x: usize, y: usize, radius: f32) -> f32 {
    let mut covered = 0_u8;
    for sample_y in 0..4 {
        for sample_x in 0..4 {
            let dx = x as f32 + (sample_x as f32 + 0.5) / 4.0 - 16.0;
            let dy = y as f32 + (sample_y as f32 + 0.5) / 4.0 - 16.0;
            if dx * dx + dy * dy <= radius * radius {
                covered += 1;
            }
        }
    }
    covered as f32 / 16.0
}

fn blend(pixel: &mut [u8], color: [u8; 4], coverage: f32) {
    let source_alpha = color[3] as f32 / 255.0 * coverage.clamp(0.0, 1.0);
    let destination_alpha = pixel[3] as f32 / 255.0;
    let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
    if output_alpha <= f32::EPSILON {
        return;
    }
    for channel in 0..3 {
        let source = color[channel] as f32 / 255.0;
        let destination = pixel[channel] as f32 / 255.0;
        pixel[channel] = (((source * source_alpha
            + destination * destination_alpha * (1.0 - source_alpha))
            / output_alpha)
            * 255.0)
            .round() as u8;
    }
    pixel[3] = (output_alpha * 255.0).round() as u8;
}

pub fn sync_brightness(frame: u8) -> f32 {
    match frame % SYNC_FRAME_COUNT {
        0 => 0.32,
        1 | 3 => 0.64,
        _ => 1.0,
    }
}

/// The QC housing with its LED lamp, 32x32 straight RGBA. Only the syncing
/// tone animates, and only in brightness.
pub fn icon_rgba(accent: [u8; 4], lit: bool, sync_frame: Option<u8>) -> Vec<u8> {
    let mut rgba = vec![0; ICON_SIZE * ICON_SIZE * 4];
    let brightness = sync_frame.map_or(1.0, sync_brightness);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let pixel = &mut rgba[(y * ICON_SIZE + x) * 4..(y * ICON_SIZE + x + 1) * 4];
            let shell = rounded_rect_coverage(x, y, 0.0, 5.0);
            if shell == 0.0 {
                continue;
            }
            let position = y as f32 / (ICON_SIZE - 1) as f32;
            let shell_color = [
                (146.0 + (76.0 - 146.0) * position).round() as u8,
                (151.0 + (81.0 - 151.0) * position).round() as u8,
                (156.0 + (86.0 - 156.0) * position).round() as u8,
                255,
            ];
            blend(pixel, shell_color, shell);
            let inner_shell = rounded_rect_coverage(x, y, 1.0, 4.0);
            let edge = (shell - inner_shell).max(0.0);
            let edge_color = if y < ICON_SIZE / 2 {
                [255, 255, 255, 72]
            } else {
                [28, 30, 33, 92]
            };
            blend(pixel, edge_color, edge);
            if lit {
                let dx = x as f32 + 0.5 - 16.0;
                let dy = y as f32 + 0.5 - 16.0;
                let distance = (dx * dx + dy * dy).sqrt();
                let glow = ((15.0 - distance) / 10.0).clamp(0.0, 1.0).powi(2) * 0.42 * brightness;
                blend(pixel, accent, glow * shell);
            }
            blend(pixel, [170, 171, 174, 255], circle_coverage(x, y, 10.0));
            blend(pixel, [119, 121, 124, 255], circle_coverage(x, y, 8.9));
            let disc_color = if lit { accent } else { [98, 99, 103, 255] };
            blend(
                pixel,
                disc_color,
                circle_coverage(x, y, 8.4) * if lit { brightness } else { 1.0 },
            );
        }
    }
    rgba
}

pub fn tone_rgba(tone: IconTone, sync_frame: u8) -> Vec<u8> {
    icon_rgba(
        icon_color(tone),
        tone != IconTone::Inactive,
        (tone == IconTone::Syncing).then_some(sync_frame),
    )
}

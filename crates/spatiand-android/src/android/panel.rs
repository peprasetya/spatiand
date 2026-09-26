//! The phone's touch area: the left pad, with the machine's monitors drawn under the thumb.
//!
//! The Deck shows these on its own screen, the sidecar; on the Beam Pro the phone's screen is
//! the controller, and most of it is the touch area. So the graphs live there, dim enough to be
//! a background and not a thing to read while scrolling -- CPU, GPU, memory, network and disk,
//! from the same `crate::system::Monitors` the sidecar draws, in the sidecar's colours. Nothing
//! here takes a touch: the whole area is the pad.

use glam::{Mat4, Vec3};
use smithay::backend::renderer::gles::ffi;
use spatiand_render::{TextImage, TextRenderer};

use crate::gl::QuadPipeline;
use crate::system::{Monitors, Series};

const GROUND: [f32; 4] = [0.035, 0.04, 0.055, 1.0];
/// The sidecar's graph colours, at a fraction of their strength: under a thumb, not in front of
/// the eyes.
const GRAPH_INK: [f32; 4] = [0.42, 0.68, 1.0, 0.32];
const SECOND_INK: [f32; 4] = [1.0, 0.72, 0.38, 0.32];
const LABEL_INK: [f32; 4] = [0.62, 0.67, 0.78, 0.7];

const MARGIN: f32 = 36.0;
const ROW_GAP: f32 = 28.0;
const LABEL_PX: f32 = 34.0;

/// A reading's label, rasterised, and the texture it is in.
struct Label {
    text: String,
    texture: u32,
    size: (u32, u32),
}

#[derive(Default)]
pub struct PhonePanel {
    labels: Vec<Option<Label>>,
    /// What each label should say now; raster and upload only happen when it changes.
    wanted: Vec<String>,
    images: Vec<Option<TextImage>>,
}

fn percent(series: &Series) -> String {
    format!("{} {:.0}%", series_name(series), series.latest() * 100.0)
}

fn series_name(series: &Series) -> &'static str {
    series.label
}

impl PhonePanel {
    /// Work out what the labels say and rasterise any that changed. Outside the GL context,
    /// which the upload in [`draw`](Self::draw) needs.
    pub fn prepare(&mut self, text: &mut TextRenderer, monitors: &Monitors) {
        let net = monitors.network.latest();
        let disk = monitors.disk.latest();
        self.wanted = vec![
            percent(&monitors.cpu),
            percent(&monitors.gpu),
            percent(&monitors.memory),
            format!(
                "Network ↓ {}  ↑ {}",
                crate::system::format_rate(net[0]),
                crate::system::format_rate(net[1])
            ),
            format!(
                "Disk read {}  write {}",
                crate::system::format_rate(disk[0]),
                crate::system::format_rate(disk[1])
            ),
        ];
        self.labels.resize_with(self.wanted.len(), || None);
        self.images.resize_with(self.wanted.len(), || None);
        for (i, wanted) in self.wanted.iter().enumerate() {
            if self.labels[i].as_ref().is_some_and(|l| &l.text == wanted) {
                continue;
            }
            self.images[i] = Some(text.render(wanted, LABEL_PX, 1000, [255, 255, 255, 255]));
        }
    }

    /// Draw into the bound framebuffer, `size` pixels, portrait as the phone is held.
    ///
    /// # Safety
    /// The context must be current, with the target bound.
    pub unsafe fn draw(
        &mut self,
        gl: &ffi::Gles2,
        quads: &QuadPipeline,
        white: u32,
        monitors: &Monitors,
        size: (u32, u32),
    ) {
        for (i, image) in self.images.iter_mut().enumerate() {
            let Some(image) = image.take() else { continue };
            if let Some(old) = self.labels[i].take() {
                gl.DeleteTextures(1, &old.texture);
            }
            self.labels[i] = Some(Label {
                text: self.wanted[i].clone(),
                texture: crate::gl::upload_rgba(gl, &image),
                size: (image.width, image.height),
            });
        }

        let (w, h) = (size.0 as f32, size.1 as f32);
        gl.Viewport(0, 0, size.0 as i32, size.1 as i32);
        gl.ClearColor(GROUND[0], GROUND[1], GROUND[2], 1.0);
        gl.Clear(ffi::COLOR_BUFFER_BIT);
        // Pixels, with the origin at the top-left and y down, as the phone's touches are.
        let projection = Mat4::orthographic_rh_gl(0.0, w, h, 0.0, -1.0, 1.0);
        let rect = |x: f32, y: f32, rw: f32, rh: f32| {
            projection
                * Mat4::from_translation(Vec3::new(x + rw * 0.5, y + rh * 0.5, 0.0))
                * Mat4::from_scale(Vec3::new(rw, -rh, 1.0))
        };

        let rows = 5.0;
        let row_h = (h - MARGIN * 2.0 - ROW_GAP * (rows - 1.0)) / rows;
        let plot_w = w - MARGIN * 2.0;
        let count = crate::system::HISTORY as f32;
        let column = plot_w / count;
        let bar_w = (column - 1.0).max(column * 0.6);
        for row in 0..5 {
            let top = MARGIN + row as f32 * (row_h + ROW_GAP);
            let label_h = self.labels.get(row).and_then(|l| l.as_ref()).map_or(0.0, |l| l.size.1 as f32);
            let plot_top = top + label_h + 6.0;
            let plot_h = (row_h - label_h - 6.0).max(8.0);
            let floor = plot_top + plot_h;
            match row {
                0..=2 => {
                    let series = [&monitors.cpu, &monitors.gpu, &monitors.memory][row];
                    for (i, value) in series.samples().enumerate() {
                        let offset = count - series.len() as f32 + i as f32;
                        let bar = (value.clamp(0.0, 1.0) * plot_h).max(1.5);
                        quads.draw(gl, white, &rect(MARGIN + offset * column, floor - bar, bar_w, bar), GRAPH_INK, (0.0, 1.0));
                    }
                }
                _ => {
                    let rates = if row == 3 { &monitors.network } else { &monitors.disk };
                    let scale = rates.scale();
                    for (i, pair) in rates.samples().enumerate() {
                        let offset = count - rates.len() as f32 + i as f32;
                        let mut bars = [(pair[0], GRAPH_INK), (pair[1], SECOND_INK)];
                        bars.sort_by(|a, b| b.0.total_cmp(&a.0));
                        for (value, colour) in bars {
                            let bar = ((value / scale).clamp(0.0, 1.0) * plot_h).max(1.5);
                            quads.draw(gl, white, &rect(MARGIN + offset * column, floor - bar, bar_w, bar), colour, (0.0, 1.0));
                        }
                    }
                }
            }
            if let Some(Some(label)) = self.labels.get(row) {
                quads.draw(
                    gl,
                    label.texture,
                    &rect(MARGIN, top, label.size.0 as f32, label.size.1 as f32),
                    LABEL_INK,
                    (0.0, 1.0),
                );
            }
        }
    }
}

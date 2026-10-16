//! A tab's picture on the GPU: the engine host's shared buffer, copied where it changed.

use spatiand_app::glow::{self, HasContext};
use spatiand_app::ui::Texture;
use spatiand_app::Gl;
use spatimesh::engine::{Format, Rect};

pub struct PageTexture {
    raw: glow::Texture,
    pub width: u32,
    pub height: u32,
}

impl PageTexture {
    pub fn new(gl: &Gl, width: u32, height: u32, format: Format) -> Result<PageTexture, String> {
        let g = gl.api();
        unsafe {
            let raw = g.create_texture()?;
            g.bind_texture(glow::TEXTURE_2D, Some(raw));
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            // The engines' bytes are blue, green, red, alpha: uploaded as they are, and read
            // with red and blue swapped (GL ES 3's swizzle), rather than swapped on the CPU.
            if format == Format::Bgra {
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_R, glow::BLUE as i32);
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_B, glow::RED as i32);
            }
            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width as i32, height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            Ok(PageTexture { raw, width, height })
        }
    }

    /// Copy what changed from a whole picture of this texture's size.
    pub fn upload(&self, gl: &Gl, picture: &[u8], changed: &[Rect]) {
        if picture.len() < (self.width * self.height * 4) as usize {
            return;
        }
        let g = gl.api();
        unsafe {
            g.bind_texture(glow::TEXTURE_2D, Some(self.raw));
            g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, self.width as i32);
            for rect in changed.iter().filter_map(|r| r.clip(self.width, self.height)) {
                // From the rectangle's first pixel; the row length steps over the rest.
                let start = ((rect.y * self.width + rect.x) * 4) as usize;
                g.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as i32,
                    rect.height as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&picture[start..])),
                );
            }
            g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
    }

    pub fn texture(&self) -> Texture {
        Texture::from_raw(self.raw, self.width, self.height)
    }

    pub fn destroy(self, gl: &Gl) {
        unsafe { gl.api().delete_texture(self.raw) };
    }
}

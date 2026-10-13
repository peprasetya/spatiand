//! Geometry for one frame, with texture coordinates in **image space**: `(0, 0)` is the
//! top left of the picture, as everyone describes side-by-side and over-under frames. The
//! shader flips to OpenGL's bottom-left origin, in one place, on the way to the sampler.
//!
//! Ported from `player/VideoMesh.kt`. Kept as plain vectors rather than GPU buffers so the
//! shapes can be checked without a GPU - the sphere in particular has a handedness that is
//! easy to get wrong and impossible to notice until a panorama turns the wrong way.

/// What `glDrawArrays` should be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    Triangles,
    TriangleStrip,
}

#[derive(Debug, Clone)]
pub struct Mesh {
    /// `x, y, z` per vertex.
    pub positions: Vec<f32>,
    /// `u, v` per vertex, image space with `v` running down.
    pub tex_coords: Vec<f32>,
    pub vertex_count: i32,
    pub draw_mode: DrawMode,
}

impl Mesh {
    /// A rectangle in clip space with half-extents `half_width` and `half_height`.
    ///
    /// Rebuilt rather than rebuilt-from-scratch on every frame in the Android original,
    /// for the same reason it is cheap here: four corners, and the fit changes whenever
    /// the window does.
    pub fn quad(half_width: f32, half_height: f32) -> Self {
        Self {
            positions: vec![
                -half_width, -half_height, 0.0,
                half_width, -half_height, 0.0,
                -half_width, half_height, 0.0,
                half_width, half_height, 0.0,
            ],
            // The bottom of the quad is the bottom of the picture, which in image space is
            // v = 1.
            tex_coords: vec![0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0],
            vertex_count: 4,
            draw_mode: DrawMode::TriangleStrip,
        }
    }

    /// The inside of a sphere, spanning `span_degrees` of longitude centred straight ahead
    /// and the full 180 degrees of latitude.
    ///
    /// 360 gives the whole thing; 180 gives the hemisphere in front, and what is behind the
    /// viewer is simply not built - so turning round shows the clear colour, which is the
    /// honest answer for a file that has no picture there. Spatiand's own sky shader makes
    /// the same choice, and says so: it "says there is no image rather than smearing the
    /// edge pixel round".
    pub fn sphere(span_degrees: f32, rows: usize, columns: usize) -> Self {
        let mut positions = Vec::with_capacity(rows * columns * 6 * 3);
        let mut tex_coords = Vec::with_capacity(rows * columns * 6 * 2);
        let span = f64::from(span_degrees).to_radians();
        let start = -span / 2.0;

        // (u, v) walks the image; (theta, phi) walks the sphere. Equirectangular is exactly
        // that correspondence held linear in both directions.
        let vertex = |u: f64, v: f64, positions: &mut Vec<f32>, tex: &mut Vec<f32>| {
            let theta = start + u * span;
            let phi = std::f64::consts::FRAC_PI_2 - v * std::f64::consts::PI;
            let cos_phi = phi.cos();
            // Looking down -Z, so theta = 0 is dead ahead and increasing u turns right.
            positions.push((cos_phi * theta.sin()) as f32);
            positions.push(phi.sin() as f32);
            positions.push((-cos_phi * theta.cos()) as f32);
            tex.push(u as f32);
            tex.push(v as f32);
        };

        for row in 0..rows {
            let v0 = row as f64 / rows as f64;
            let v1 = (row + 1) as f64 / rows as f64;
            for column in 0..columns {
                let u0 = column as f64 / columns as f64;
                let u1 = (column + 1) as f64 / columns as f64;
                for (u, v) in [
                    (u0, v0),
                    (u0, v1),
                    (u1, v0),
                    (u1, v0),
                    (u0, v1),
                    (u1, v1),
                ] {
                    vertex(u, v, &mut positions, &mut tex_coords);
                }
            }
        }

        Self {
            positions,
            tex_coords,
            vertex_count: (rows * columns * 6) as i32,
            draw_mode: DrawMode::Triangles,
        }
    }

    /// The sphere at the density the player actually draws.
    pub fn sphere_default(span_degrees: f32) -> Self {
        Self::sphere(span_degrees, 36, 72)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(mesh: &Mesh, vertex: usize) -> [f32; 3] {
        [
            mesh.positions[vertex * 3],
            mesh.positions[vertex * 3 + 1],
            mesh.positions[vertex * 3 + 2],
        ]
    }

    #[track_caller]
    fn near(expected: f32, actual: f32) {
        assert!(
            (expected - actual).abs() < 1e-4,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn a_quad_is_four_corners_and_the_picture_is_the_right_way_up() {
        let quad = Mesh::quad(1.0, 0.5);
        assert_eq!(4, quad.vertex_count);
        assert_eq!(DrawMode::TriangleStrip, quad.draw_mode);
        assert_eq!([-1.0, -0.5, 0.0], position(&quad, 0));
        // The first corner is bottom-left in clip space, which is v = 1 in image space.
        assert_eq!(vec![0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0], quad.tex_coords);
    }

    #[test]
    fn a_sphere_is_two_triangles_per_cell() {
        let sphere = Mesh::sphere(360.0, 4, 8);
        assert_eq!(4 * 8 * 6, sphere.vertex_count);
        assert_eq!(sphere.vertex_count as usize * 3, sphere.positions.len());
        assert_eq!(sphere.vertex_count as usize * 2, sphere.tex_coords.len());
        assert_eq!(DrawMode::Triangles, sphere.draw_mode);
    }

    /// The handedness. `u = 0.5` is the middle of the picture and has to be dead ahead,
    /// which is -Z; a sign error here turns a panorama round without looking broken.
    #[test]
    fn the_middle_of_the_image_is_straight_ahead() {
        let sphere = Mesh::sphere(180.0, 2, 2);
        // Row 1, column 1 starts at u = 0.5, v = 0.5 - the centre of the picture.
        let centre = 1 * 2 * 6 + 1 * 6;
        let [x, y, z] = position(&sphere, centre);
        near(0.0, x);
        near(0.0, y);
        near(-1.0, z);
    }

    /// Increasing u turns to the right, and a 180 dome stops at the sides rather than
    /// wrapping - so nothing is ever drawn behind the viewer.
    #[test]
    fn a_dome_spans_the_front_hemisphere_and_no_further() {
        // Two rows, so that v = 0.5 exists: at the poles cos(phi) is zero and every
        // vertex sits on the axis whatever the longitude, which measures nothing.
        let sphere = Mesh::sphere(180.0, 2, 2);
        let mut leftmost = f32::MAX;
        let mut rightmost = f32::MIN;
        let mut behind = false;
        for vertex in 0..sphere.vertex_count as usize {
            let [x, _, z] = position(&sphere, vertex);
            leftmost = leftmost.min(x);
            rightmost = rightmost.max(x);
            if z > 1e-4 {
                behind = true;
            }
        }
        near(-1.0, leftmost);
        near(1.0, rightmost);
        assert!(!behind, "a 180 dome must not put geometry behind the viewer");
    }

    /// A full sphere does close behind, which is what makes 360 different from 180.
    #[test]
    fn a_full_sphere_closes_behind_the_viewer() {
        let sphere = Mesh::sphere(360.0, 4, 8);
        let behind = (0..sphere.vertex_count as usize)
            .map(|v| position(&sphere, v)[2])
            .fold(f32::MIN, f32::max);
        assert!(behind > 0.9, "the sphere should reach +Z, reached {behind}");
    }

    /// Equirectangular means v = 0 is the zenith and v = 1 the nadir, both single points.
    #[test]
    fn the_poles_are_where_equirectangular_puts_them() {
        let sphere = Mesh::sphere(360.0, 4, 8);
        let mut highest = f32::MIN;
        let mut lowest = f32::MAX;
        for vertex in 0..sphere.vertex_count as usize {
            let y = position(&sphere, vertex)[1];
            highest = highest.max(y);
            lowest = lowest.min(y);
        }
        near(1.0, highest);
        near(-1.0, lowest);
    }
}

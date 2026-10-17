//! SpatiMesh built without its own media (`--no-default-features`, where there is no FFmpeg to
//! build against): the engine plays pages' media as it always did, and this is nothing.

use spatiand_app::glow;
use spatiand_app::ui::{Painter, Rect};
use spatiand_app::Gl;
use spatimesh::engine::TabId;

#[derive(Default)]
pub struct Media;

impl Media {
    pub fn said(&mut self, _tab: TabId, _json: &str) {}
    pub fn forget(&mut self, _tab: TabId) {}
    pub fn set_tab_key(&mut self, _tab: TabId, _key: &str) {}
    pub fn tick(&mut self) {}
    pub fn capture(&mut self, _gl: &Gl) {}
    pub fn take_out(&mut self) -> Vec<(TabId, String)> {
        Vec::new()
    }
    pub fn take_fresh(&mut self) -> Vec<TabId> {
        Vec::new()
    }
    pub fn busy(&self) -> bool {
        false
    }
    pub fn playing_in(&self, _tab: TabId) -> bool {
        false
    }
    pub fn films_in(&self, _tab: TabId) -> bool {
        false
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, _gl: &Gl, _p: &mut Painter, _tab: TabId, _page: Rect, _shown: Rect, _uv: [f32; 4], _texture: glow::Texture, _zoom: f32) {}
    pub fn pointer(&mut self, _tab: TabId, _x: f32, _y: f32, _pressed: bool) -> bool {
        false
    }
}

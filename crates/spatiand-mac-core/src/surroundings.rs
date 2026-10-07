//! What surrounds the wearer in the Mac's room: the Deck's environment picker, working as the Deck's does.
//!
//! The choosing, remembering, finding and decoding of environments is the Deck's own code
//! (`spatiand_room::environment`, which the Beam Pro also uses); this only joins it to the menus and holds the
//! picture until the renderer takes it. The state files are the Deck's too, in the same folder under the home
//! directory (`~/.local/share/spatiand`), so a folder of panoramas can be copied between the machines.

use spatiand_render::sky::{Sky, SkyProjection, SkyStereo};
use spatiand_room::environment::{Browser, Environments, SkyLoader};
use spatiand_shell::EnvironmentChoice;

use crate::shell_ui::ShellUi;

pub struct Surroundings {
    environments: Environments,
    browser: Browser,
    loader: SkyLoader,
    /// A picture that has been decoded and not yet handed to the renderer.
    ready: Option<Sky>,
}

/// What the renderer needs to know about a picture besides its pixels, as plain numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct SkyInfo {
    pub width: u32,
    pub height: u32,
    /// 0 for all the way round, 1 for the front half only.
    pub projection: i32,
    /// 0 one picture for both eyes, 1 left eye over right, 2 left eye beside right.
    pub stereo: i32,
    pub yaw_millideg: i32,
}

impl SkyInfo {
    pub fn of(sky: &Sky) -> Self {
        Self {
            width: sky.width,
            height: sky.height,
            projection: match sky.source.projection {
                SkyProjection::Equirect360 => 0,
                SkyProjection::Equirect180 => 1,
            },
            stereo: match sky.source.stereo {
                SkyStereo::Mono => 0,
                SkyStereo::OverUnder => 1,
                SkyStereo::SideBySide => 2,
            },
            yaw_millideg: sky.source.yaw_offset_millideg,
        }
    }
}

impl Surroundings {
    /// The two generated environments and nothing written anywhere: what tests and offscreen runs have.
    pub fn ephemeral() -> Self {
        Self::with(Environments::ephemeral())
    }

    /// The wearer's own: the folder of images, what was chosen last time, and the paths added from the browser.
    pub fn on_disk() -> Self {
        Self::with(Environments::discover())
    }

    fn with(environments: Environments) -> Self {
        let mut loader = SkyLoader::start();
        loader.load(&environments);
        Self { environments, browser: Browser::new(), loader, ready: None }
    }

    /// How many images are on offer, the two generated environments not counted.
    pub fn image_count(&self) -> usize {
        self.environments.entries().len().saturating_sub(2)
    }

    pub fn choice(&self) -> EnvironmentChoice {
        self.environments.choice()
    }

    pub fn describe(&self) -> String {
        self.environments.describe()
    }

    /// The picker is opening: the folder is read again, so an image put there a moment ago is on offer.
    pub fn open_picker(&mut self, ui: &mut ShellUi) {
        self.environments.refresh();
        ui.set_environment_entries(self.environments.entries(), self.environments.choice());
    }

    /// A row was chosen.
    pub fn choose(&mut self, choice: EnvironmentChoice) {
        self.environments.select(choice);
        self.loader.load(&self.environments);
    }

    /// The browser is asked for a folder: the one it is in, or one entered from it.
    pub fn list(&mut self, name: Option<String>, ui: &mut ShellUi) {
        if let Some(name) = name {
            self.browser.enter(&name);
        }
        ui.shell.show_directory(self.browser.label(), self.browser.entries());
        ui.dirty = true;
    }

    /// An image was picked in the browser: it is added to the list, and the room becomes it.
    pub fn add(&mut self, name: &str) {
        let path = self.browser.resolve(name);
        let choice = self.environments.add(&path);
        self.choose(choice);
    }

    /// The newest picture that has finished loading, once.
    pub fn poll(&mut self) -> Option<SkyInfo> {
        if let Some(sky) = self.loader.take() {
            log::info!("environment now {}", self.environments.describe());
            self.ready = Some(sky);
        }
        self.ready.as_ref().map(SkyInfo::of)
    }

    /// The pixels of what [`poll`](Self::poll) reported, handed over: straight RGBA, top row first.
    pub fn take(&mut self) -> Option<Sky> {
        self.ready.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(s: &mut Surroundings) -> SkyInfo {
        for _ in 0..3000 {
            if let Some(info) = s.poll() {
                return info;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("nothing was loaded");
    }

    #[test]
    fn it_starts_on_the_studio_and_hands_over_its_picture_once() {
        let mut s = Surroundings::ephemeral();
        let info = wait(&mut s);
        assert!(info.width >= 1024 && info.height >= 512);
        assert_eq!((info.projection, info.stereo), (0, 0));
        let sky = s.take().expect("the pixels");
        assert_eq!(sky.rgba.len(), (info.width * info.height * 4) as usize);
        assert!(s.take().is_none(), "handed over once");
        assert!(s.poll().is_none(), "and not reported again");
    }

    #[test]
    fn choosing_blank_makes_the_room_black() {
        let mut s = Surroundings::ephemeral();
        wait(&mut s);
        s.take();
        s.choose(EnvironmentChoice::Blank);
        assert_eq!(s.choice(), EnvironmentChoice::Blank);
        wait(&mut s);
        let sky = s.take().unwrap();
        assert!(sky.rgba.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
    }
}

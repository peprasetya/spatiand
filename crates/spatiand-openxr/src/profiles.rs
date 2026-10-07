//! Which interaction profiles and component paths an application may suggest bindings for.
//!
//! The specification lists every profile with the component paths each has, and what makes each
//! available: a version of OpenXR, or extensions being enabled. A runtime accepts a suggestion for
//! one that is available -- *whether or not it has that hardware*, since an application suggests
//! for everything it knows -- and refuses what is not with `XR_ERROR_PATH_UNSUPPORTED`. The table
//! is `profiles_table.rs`, generated from the Khronos conformance suite's own.

use crate::profiles_table::{Availability, Profile, AVAILABILITIES, PROFILES};

/// Whether `feature` -- `XR_VERSION_1_1`, or an extension's name -- is there for an instance made for
/// OpenXR 1.`minor` with these extensions enabled.
fn has(minor: u32, extensions: &[String], feature: &str) -> bool {
    match feature {
        "XR_VERSION_1_0" => true,
        "XR_VERSION_1_1" => minor >= 1,
        name => extensions.iter().any(|e| e == name),
    }
}

fn satisfied(availability: Availability, minor: u32, extensions: &[String]) -> bool {
    availability
        .iter()
        .any(|set| set.iter().all(|feature| has(minor, extensions, feature)))
}

/// The profile at this path, if the specification has one.
pub fn profile(path: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.path == path)
}

/// Whether a profile can be suggested for at all with what the instance has.
pub fn profile_available(profile: &Profile, minor: u32, extensions: &[String]) -> bool {
    satisfied(AVAILABILITIES[profile.available], minor, extensions)
}

/// Whether this is a component path of the profile, and available -- or the path of an input
/// that has components, such as `.../input/select`, which stands for whichever of its components
/// suits the action it is bound to.
pub fn component_available(profile: &Profile, path: &str, minor: u32, extensions: &[String]) -> bool {
    profile.components.iter().any(|c| {
        // An input's identifier, not a user path or the bare word `input`.
        let names_an_input = path.contains("/input/") || path.contains("/output/");
        let named = c.path == path
            || (names_an_input && c.path.strip_prefix(path).is_some_and(|rest| rest.starts_with('/')));
        named && satisfied(AVAILABILITIES[c.available], minor, extensions)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simple_controller_is_there_for_everyone_and_its_select_is_a_component() {
        let simple = profile("/interaction_profiles/khr/simple_controller").unwrap();
        assert!(profile_available(simple, 0, &[]));
        assert!(component_available(simple, "/user/hand/left/input/select/click", 0, &[]));
        assert!(!component_available(simple, "/user/hand/left/input/trigger/value", 0, &[]));
        // The input itself stands for its components, but a half-named one is not a path.
        assert!(component_available(simple, "/user/hand/left/input/select", 0, &[]));
        assert!(!component_available(simple, "/user/hand/left/input/sele", 0, &[]));
        assert!(!component_available(simple, "/user/hand/left/input", 0, &[]));
        assert!(!component_available(simple, "/user/hand", 0, &[]));
    }

    #[test]
    fn a_profile_that_needs_an_extension_is_not_there_without_it() {
        let oppo = profile("/interaction_profiles/oppo/mr_controller_oppo").unwrap();
        assert!(!profile_available(oppo, 1, &[]));
        assert!(profile_available(oppo, 1, &["XR_OPPO_controller_interaction".to_string()]));
    }

    #[test]
    fn a_path_of_a_later_version_waits_for_it() {
        let simple = profile("/interaction_profiles/khr/simple_controller").unwrap();
        let grip_surface = "/user/hand/left/input/grip_surface/pose";
        assert!(!component_available(simple, grip_surface, 0, &[]));
        assert!(component_available(simple, grip_surface, 1, &[]));
    }
}

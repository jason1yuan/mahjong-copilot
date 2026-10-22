//! Game profiles (`profiles/`): which windows a game runs in and the numbers that locate
//! its layout. Only numbers and text, never game art. The built-in profiles are compiled
//! into the app.

use serde::Deserialize;

use crate::capture::WindowInfo;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub windows: WindowMatch,
    pub tile_face: TileFace,
    pub hand: HandGeometry,
}

/// A window belongs to the game when its app is one of `apps` (any app if empty) and its
/// title contains one of `title_contains`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct WindowMatch {
    pub apps: Vec<String>,
    pub title_contains: Vec<String>,
}

/// What a tile face looks like: light (mean of R, G, B above `min_brightness`) and grey
/// (channels within `max_spread` of each other).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct TileFace {
    pub min_brightness: u8,
    pub max_spread: u8,
}

/// Where the hand sits in the game area, which keeps a fixed aspect ratio inside the
/// window: `pitch` and `left` are fractions of the game area's width, `top` of its height.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct HandGeometry {
    pub aspect: f32,
    pub pitch: f32,
    pub left: f32,
    pub top: f32,
}

impl Profile {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn matches(&self, window: &WindowInfo) -> bool {
        let app = self.windows.apps.is_empty() || self.windows.apps.contains(&window.app);
        app && self
            .windows
            .title_contains
            .iter()
            .any(|t| window.title.contains(t.as_str()))
    }
}

/// The profiles compiled into the app.
pub fn builtin() -> Result<Vec<Profile>, serde_json::Error> {
    [include_str!("../../profiles/majsoul-web/profile.json")]
        .into_iter()
        .map(Profile::from_json)
        .collect()
}

/// The first profile whose game runs in this window.
pub fn for_window<'a>(profiles: &'a [Profile], window: &WindowInfo) -> Option<&'a Profile> {
    profiles.iter().find(|p| p.matches(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            id: 1,
            app: app.into(),
            bundle_id: String::new(),
            title: title.into(),
            width: 1280.0,
            height: 720.0,
            on_screen: true,
        }
    }

    #[test]
    fn the_builtin_profiles_parse() {
        let profiles = builtin().unwrap();
        let majsoul = &profiles[0];
        assert_eq!(majsoul.id, "majsoul-web");
        assert!((majsoul.hand.pitch - 0.0493).abs() < 1e-6);
    }

    #[test]
    fn matches_mahjong_soul_in_a_browser_only() {
        let profiles = builtin().unwrap();
        let found = |app, title| for_window(&profiles, &window(app, title)).map(|p| p.id.as_str());
        assert_eq!(found("Google Chrome", "雀魂麻將"), Some("majsoul-web"));
        assert_eq!(
            found("Safari", "雀魂 -じゃんたま-| 麻雀を無料で"),
            Some("majsoul-web")
        );
        assert_eq!(found("Firefox", "Mahjong Soul"), Some("majsoul-web"));
        assert_eq!(found("Google Chrome", "Inbox"), None);
        assert_eq!(
            found("Preview", "雀魂 screenshot.png"),
            None,
            "not a browser"
        );
    }
}

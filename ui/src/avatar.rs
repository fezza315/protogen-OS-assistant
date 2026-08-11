//! avatar.rs
//! ---------
//! A lightweight avatar pane: a stack of state images (idle/listening/
//! thinking/speaking) swapped based on daemon events, in the spirit of
//! Nyarch Assistant's avatar panel but intentionally simple (a Picture
//! widget + state enum) rather than a full animated rig, so it stays cheap
//! to run as a resident background app. Point AVATAR_DIR at a folder of
//! idle.png / listening.png / thinking.png / speaking.png (or .svg) to
//! reskin it -- this is where a Protogen-specific art asset would slot in.
use gtk::prelude::*;
use gtk::{Box as GtkBox, Orientation, Picture};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarState {
    Idle,
    Listening,
    Thinking,
    /// Not yet driven by the client -- reserved for when TTS playback
    /// state is reported back over the socket instead of fire-and-forget.
    #[allow(dead_code)]
    Speaking,
}

#[derive(Clone)]
pub struct AvatarWidget {
    container: GtkBox,
    picture: Picture,
    asset_dir: Rc<PathBuf>,
}

impl AvatarWidget {
    pub fn new() -> Self {
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.set_width_request(280);
        container.add_css_class("avatar-pane");

        let picture = Picture::new();
        picture.set_vexpand(true);
        picture.set_hexpand(true);
        container.append(&picture);

        let asset_dir = Rc::new(
            std::env::var("PROTOGEN_AVATAR_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/usr/share/protogenos/avatar")),
        );

        let widget = Self { container, picture, asset_dir };
        widget.set_state(AvatarState::Idle);
        widget
    }

    pub fn widget(&self) -> &GtkBox {
        &self.container
    }

    pub fn set_state(&self, state: AvatarState) {
        let filename = match state {
            AvatarState::Idle => "idle.png",
            AvatarState::Listening => "listening.png",
            AvatarState::Thinking => "thinking.png",
            AvatarState::Speaking => "speaking.png",
        };
        let path = self.asset_dir.join(filename);
        if path.exists() {
            self.picture.set_filename(Some(&path));
        }
        self.container.set_tooltip_text(Some(match state {
            AvatarState::Idle => "Protogen: idle",
            AvatarState::Listening => "Protogen: listening",
            AvatarState::Thinking => "Protogen: thinking",
            AvatarState::Speaking => "Protogen: speaking",
        }));
    }
}

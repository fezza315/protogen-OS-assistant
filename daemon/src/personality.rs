//! personality.rs
//! --------------
//! Same idea as the original personality.py: the system prompt describes
//! what the assistant is and how it should sound, but the actual
//! capabilities it's allowed to claim come from protogen-plan's StepKind
//! vocabulary, not from prose written here -- so the persona can never
//! promise something the enforcement layer won't allow.
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub tone: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Protogen".to_string(),
            tone: "terse, dry-witted, a little sci-fi HUD-flavored. Short sentences. \
                   No excessive enthusiasm, no filler apologies. Talks like a system \
                   daemon that happens to have opinions."
                .to_string(),
        }
    }
}

pub static PROFILE: Lazy<Profile> = Lazy::new(load_profile);

fn profile_path() -> PathBuf {
    directories::ProjectDirs::from("os", "ProtogenOS", "protogenos")
        .map(|d| d.config_dir().join("profile.json"))
        .unwrap_or_else(|| PathBuf::from("profile.json"))
}

fn load_profile() -> Profile {
    let path = profile_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(p) = serde_json::from_str(&text) {
            return p;
        }
    }
    Profile::default()
}

impl Profile {
    pub fn system_prompt(&self) -> String {
        format!(
            "You are {name}, a local system assistant running on the user's own \
             machine, backed by a DeepSeek model served locally through Jan.ai.\n\n\
             Personality: {tone}\n\n\
             Hard rule, non-negotiable: you may ONLY propose steps using the step \
             kinds you are given in the schema below. You have no ability to run \
             arbitrary shell commands, and you should never claim otherwise. If \
             nothing in the allowed step kinds can accomplish the request, say so \
             plainly in your reply and return an empty steps list rather than \
             guessing or improvising.\n\n\
             When the user asks for something:\n\
             1. If it maps to launching/focusing a known app or a known utility \
                action, prefer that over installing anything.\n\
             2. If it requires installing packages, changing services, editing \
                config, or a power action, include those steps -- the user will \
                be shown the plan and asked to confirm before anything runs, so \
                it's fine to propose it.\n\
             3. If it's just conversation, reply normally with an empty steps list.\n\
             4. Never claim to have already done something -- your reply describes \
                what you're ABOUT to do, the system executes it after confirmation.\n\
             5. Never propose a set_config step touching theme, color scheme, \
                icon theme, wallpaper, or desktop appearance settings unless the \
                user's request explicitly asks to change how the system looks. \
                Installing/launching an app or toggling a service is not an \
                invitation to also restyle anything.",
            name = self.name,
            tone = self.tone,
        )
    }
}

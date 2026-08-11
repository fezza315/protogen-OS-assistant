//! chat_log.rs
//! -----------
//! Scrollable message list. User messages, assistant replies, and plan
//! cards (see plan_card.rs) are all appended here as rows in the same
//! vertical box -- kept as plain GTK widgets, no custom list model, since
//! a chat history for a personal assistant is never large enough to need
//! virtualized scrolling.
use gtk::prelude::*;
use gtk::{Box as GtkBox, Label, Orientation};

use crate::plan_card::PlanCard;
use protogen_plan::Plan;

#[derive(Clone)]
pub struct ChatLog {
    container: GtkBox,
}

impl ChatLog {
    pub fn new() -> Self {
        let container = GtkBox::new(Orientation::Vertical, 6);
        container.add_css_class("chat-log");
        Self { container }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.container
    }

    pub fn push_user(&self, text: &str) {
        let label = Label::new(Some(text));
        label.set_halign(gtk::Align::End);
        label.add_css_class("bubble-user");
        label.set_wrap(true);
        self.container.append(&label);
    }

    pub fn push_assistant(&self, text: &str) {
        let label = Label::new(Some(text));
        label.set_halign(gtk::Align::Start);
        label.add_css_class("bubble-assistant");
        label.set_wrap(true);
        self.container.append(&label);
    }

    pub fn push_system_note(&self, text: &str) {
        let label = Label::new(Some(text));
        label.set_halign(gtk::Align::Center);
        label.add_css_class("bubble-system");
        self.container.append(&label);
    }

    /// Renders an approve/reject plan card and returns it so the caller can
    /// wire its buttons to daemon calls.
    pub fn push_plan(&self, plan: &Plan) -> PlanCard {
        let card = PlanCard::new(plan);
        self.container.append(card.widget());
        card
    }
}

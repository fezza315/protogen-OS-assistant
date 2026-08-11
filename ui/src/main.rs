//! protogen-ui
//! ------------
//! GTK4 chat + avatar window, styled after Nyarch Assistant's layout
//! (https://github.com/NyarchLinux/NyarchAssistant): a fixed avatar pane on
//! one side, scrollable chat log, text entry + mic toggle, and inline
//! "plan cards" when the daemon proposes a system-changing action that
//! needs approval.
//!
//! This process holds NO privileges and executes NOTHING itself -- it only
//! renders state and forwards user text/approvals to protogen-daemon over
//! the Unix socket (see protogen_plan crate for the Plan shape it renders,
//! and ../daemon/src/ipc.rs for the wire protocol this mirrors).
mod avatar;
mod chat_log;
mod client;
mod plan_card;

use gtk::prelude::*;
use gtk::{glib, Application, ApplicationWindow, Box as GtkBox, Button, Entry, Orientation, ScrolledWindow};
use std::cell::RefCell;
use std::rc::Rc;

use client::DaemonClient;

const APP_ID: &str = "os.ProtogenOS.Assistant";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Protogen")
        .default_width(920)
        .default_height(640)
        .build();

    let root = GtkBox::new(Orientation::Horizontal, 0);
    root.add_css_class("protogen-root");

    // Left pane: avatar
    let avatar_widget = avatar::AvatarWidget::new();
    root.append(avatar_widget.widget());

    // Right pane: chat log + input
    let right = GtkBox::new(Orientation::Vertical, 8);
    right.set_hexpand(true);
    right.add_css_class("chat-pane");

    let log = chat_log::ChatLog::new();
    let scroller = ScrolledWindow::builder()
        .child(log.widget())
        .vexpand(true)
        .build();
    right.append(&scroller);

    let input_row = GtkBox::new(Orientation::Horizontal, 6);
    let entry = Entry::builder()
        .placeholder_text("Ask Protogen to do something...")
        .hexpand(true)
        .build();
    let mic_btn = Button::from_icon_name("audio-input-microphone-symbolic");
    mic_btn.set_tooltip_text(Some("Toggle voice listening"));
    let send_btn = Button::with_label("Send");
    input_row.append(&entry);
    input_row.append(&mic_btn);
    input_row.append(&send_btn);
    right.append(&input_row);

    root.append(&right);
    window.set_child(Some(&root));

    let client = Rc::new(RefCell::new(DaemonClient::new()));
    client::spawn_connection(client.clone(), log.clone(), avatar_widget.clone());

    let entry_clone = entry.clone();
    let client_for_send = client.clone();
    let log_for_send = log.clone();
    let do_send = move || {
        let text = entry_clone.text().to_string();
        if text.trim().is_empty() {
            return;
        }
        log_for_send.push_user(&text);
        client_for_send.borrow().send_text(&text);
        entry_clone.set_text("");
    };

    let do_send_click = do_send.clone();
    send_btn.connect_clicked(move |_| do_send_click());
    entry.connect_activate(move |_| do_send());

    let client_for_mic = client.clone();
    let avatar_for_mic = avatar_widget.clone();
    let listening = Rc::new(RefCell::new(false));
    mic_btn.connect_clicked(move |_| {
        let mut is_listening = listening.borrow_mut();
        *is_listening = !*is_listening;
        if *is_listening {
            client_for_mic.borrow().start_listening();
            avatar_for_mic.set_state(avatar::AvatarState::Listening);
        } else {
            client_for_mic.borrow().stop_listening();
            avatar_for_mic.set_state(avatar::AvatarState::Idle);
        }
    });

    load_css();
    window.present();
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

use adw::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use libadwaita as adw;
use std::{ffi::OsStr, time::Duration};

const AUR_PACKAGE_URL: &str = "https://aur.archlinux.org/packages/massiveeq-git";
const BUILD_COMMIT: &str = env!("MASSIVEEQ_BUILD_COMMIT");
const UPDATE_COMMAND: &str = "yay -S massiveeq-git";

pub fn add_update_button(header: &adw::HeaderBar, window: &adw::ApplicationWindow) {
    let button = gtk::Button::new();
    button.add_css_class("update-button");
    button.set_valign(gtk::Align::Center);
    button.set_visible(false);
    button.set_tooltip_text(Some("A newer MassiveEQ revision is available"));
    button.update_property(&[gtk::accessible::Property::Label(
        "MassiveEQ update available",
    )]);

    let contents = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    contents.append(&gtk::Image::from_icon_name(
        "software-update-available-symbolic",
    ));
    contents.append(&gtk::Label::new(Some("UPDATE")));
    button.set_child(Some(&contents));
    header.pack_end(&button);

    button.connect_clicked({
        let window = window.clone();
        move |_| show_update_dialog(&window)
    });

    if valid_build_commit(BUILD_COMMIT) {
        glib::timeout_add_local_once(Duration::from_secs(2), move || {
            check_upstream(&button);
        });
    }
}

fn check_upstream(button: &gtk::Button) {
    let button = button.clone();
    let url = format!(
        "https://api.github.com/repos/massiveadam/massiveeq/compare/{BUILD_COMMIT}...main?per_page=1"
    );
    let arguments: [&OsStr; 11] = [
        OsStr::new("curl"),
        OsStr::new("--fail"),
        OsStr::new("--silent"),
        OsStr::new("--show-error"),
        OsStr::new("--max-time"),
        OsStr::new("8"),
        OsStr::new("--header"),
        OsStr::new("Accept: application/vnd.github+json"),
        OsStr::new("--user-agent"),
        OsStr::new(concat!("MassiveEQ/", env!("CARGO_PKG_VERSION"))),
        OsStr::new(&url),
    ];
    let Ok(process) = gio::Subprocess::newv(
        &arguments,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    ) else {
        return;
    };

    process
        .clone()
        .communicate_utf8_async(None, gio::Cancellable::NONE, move |result| {
            if !process.is_successful() {
                return;
            }
            let Ok((Some(body), _)) = result else {
                return;
            };
            button.set_visible(response_indicates_update(&body));
        });
}

fn show_update_dialog(window: &adw::ApplicationWindow) {
    let dialog = adw::AlertDialog::new(
        Some("MassiveEQ update available"),
        Some(
            "A newer upstream revision is available. Because MassiveEQ is installed through the AUR, update it with your AUR helper. You can copy the command below or open the package page.",
        ),
    );
    dialog.add_responses(&[
        ("later", "Later"),
        ("aur", "Open AUR page"),
        ("copy", "Copy yay command"),
    ]);
    dialog.set_close_response("later");
    dialog.set_default_response(Some("copy"));
    dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    dialog.connect_response(None, move |_, response| match response {
        "copy" => {
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(UPDATE_COMMAND);
            }
        }
        "aur" => {
            let _ =
                gio::AppInfo::launch_default_for_uri(AUR_PACKAGE_URL, gio::AppLaunchContext::NONE);
        }
        _ => {}
    });
    dialog.present(Some(window));
}

fn valid_build_commit(value: &str) -> bool {
    value != "unknown"
        && (7..=64).contains(&value.len())
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn response_indicates_update(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            Some(value.get("status")?.as_str()? == "ahead" && value.get("ahead_by")?.as_u64()? > 0)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_requires_remote_main_to_be_ahead() {
        assert!(response_indicates_update(
            r#"{"status":"ahead","ahead_by":2}"#
        ));
        assert!(!response_indicates_update(
            r#"{"status":"identical","ahead_by":0}"#
        ));
        assert!(!response_indicates_update(
            r#"{"status":"diverged","ahead_by":3}"#
        ));
    }

    #[test]
    fn malformed_update_responses_fail_closed() {
        assert!(!response_indicates_update("not json"));
        assert!(!response_indicates_update(r#"{"status":"ahead"}"#));
    }
}

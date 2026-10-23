use adw::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;
use libadwaita as adw;
use std::{ffi::OsStr, time::Duration};

const AUR_PACKAGE: &str = "massiveeq-git";
const AUR_PACKAGE_URL: &str = "https://aur.archlinux.org/packages/massiveeq-git";
const BUILD_COMMIT: &str = env!("MASSIVEEQ_BUILD_COMMIT");
/// AUR helpers, in the order MassiveEQ prefers them.
const AUR_HELPERS: [&str; 3] = ["paru", "yay", "pikaur"];
/// Terminals and the arguments that come before the command to run.
const TERMINALS: [(&str, &[&str]); 11] = [
    ("xdg-terminal-exec", &[]),
    ("kitty", &[]),
    ("ghostty", &["-e"]),
    ("foot", &[]),
    ("alacritty", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("konsole", &["-e"]),
    ("ptyxis", &["--"]),
    ("gnome-terminal", &["--"]),
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];
/// Runs the upgrade in the terminal and keeps it open to read the result.
const UPDATE_SCRIPT: &str = r#"printf 'Updating MassiveEQ with %s...\n\n' "$1"
if "$1" -S "$2"; then
  printf '\nMassiveEQ is updated. The audio service and tray restart on their own;\n'
  printf 'click RESTART in the editor to reopen it on the new version.\n'
else
  printf '\nThe update did not finish. Run "%s -S %s" to try again.\n' "$1" "$2"
fi
printf '\n'
read -rp 'Press Enter to close. ' _
"#;

/// The header button: UPDATE when GitHub has a newer revision, RESTART once
/// a newer version is installed under the running editor.
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
    let caption = gtk::Label::new(Some("UPDATE"));
    contents.append(&caption);
    button.set_child(Some(&contents));
    header.pack_end(&button);

    let installed = std::rc::Rc::new(std::cell::Cell::new(false));
    button.connect_clicked({
        let window = window.clone();
        let installed = installed.clone();
        move |_| {
            if installed.get() {
                show_restart_dialog(&window);
            } else {
                show_update_dialog(&window);
            }
        }
    });

    if valid_build_commit(BUILD_COMMIT) {
        glib::timeout_add_local_once(Duration::from_secs(2), {
            let button = button.clone();
            move || check_upstream(&button)
        });
        // Long-running editors pick up later revisions too.
        glib::timeout_add_seconds_local(6 * 60 * 60, {
            let button = button.clone();
            let installed = installed.clone();
            move || {
                if !installed.get() {
                    check_upstream(&button);
                }
                glib::ControlFlow::Continue
            }
        });
    }
    // pacman replacing the editor's binary means an update was installed,
    // by this button, cachy-update, or an AUR helper.
    glib::timeout_add_seconds_local(10, move || {
        if !massiveeq_core::upgrade::executable_replaced() {
            return glib::ControlFlow::Continue;
        }
        installed.set(true);
        caption.set_text("RESTART");
        button.set_tooltip_text(Some(
            "MassiveEQ was updated; restart the editor to use the new version",
        ));
        button.update_property(&[gtk::accessible::Property::Label(
            "Restart MassiveEQ on the updated version",
        )]);
        button.set_visible(true);
        glib::ControlFlow::Break
    });
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
            if response_indicates_update(&body) {
                button.set_visible(true);
            }
        });
}

fn installed(program: &str) -> bool {
    glib::find_program_in_path(program).is_some()
}

/// The first installed AUR helper.
fn aur_helper() -> Option<&'static str> {
    AUR_HELPERS.into_iter().find(|helper| installed(helper))
}

/// The user's `$TERMINAL` if set and installed, else the first known one.
fn terminal() -> Option<String> {
    std::env::var("TERMINAL")
        .ok()
        .filter(|terminal| !terminal.is_empty() && installed(terminal))
        .or_else(|| {
            TERMINALS
                .iter()
                .map(|(name, _)| *name)
                .find(|name| installed(name))
                .map(str::to_owned)
        })
}

/// The full command line that runs the update script in `terminal`.
fn terminal_command(terminal: &str, helper: &str) -> Vec<String> {
    let name = std::path::Path::new(terminal)
        .file_name()
        .map_or(terminal.into(), |name| name.to_string_lossy());
    let prefix = TERMINALS
        .iter()
        .find(|(known, _)| *known == name)
        .map_or(&["-e"][..], |(_, prefix)| prefix);
    let mut command = vec![terminal.to_owned()];
    command.extend(prefix.iter().map(|argument| (*argument).to_owned()));
    command.extend(
        [
            "bash",
            "-c",
            UPDATE_SCRIPT,
            "massiveeq-update",
            helper,
            AUR_PACKAGE,
        ]
        .map(str::to_owned),
    );
    command
}

fn run_update(terminal: &str, helper: &str) -> bool {
    let command = terminal_command(terminal, helper);
    let arguments = command.iter().map(OsStr::new).collect::<Vec<_>>();
    gio::Subprocess::newv(&arguments, gio::SubprocessFlags::NONE).is_ok()
}

fn show_update_dialog(window: &adw::ApplicationWindow) {
    let helper = aur_helper();
    let terminal = terminal();
    let command = format!("{} -S {AUR_PACKAGE}", helper.unwrap_or("yay"));
    let body = match (&helper, &terminal) {
        (Some(_), Some(_)) => format!(
            "A newer revision is on the AUR. Update now opens a terminal that runs \
             \u{201c}{command}\u{201d}; enter your password there. The audio service \
             and tray restart on the new version by themselves. Desktop updaters such \
             as cachy-update also list MassiveEQ updates."
        ),
        _ => format!(
            "A newer revision is on the AUR. Update it with your AUR helper: \
             \u{201c}{command}\u{201d}. Desktop updaters such as cachy-update also list \
             MassiveEQ updates."
        ),
    };
    let dialog = adw::AlertDialog::new(Some("MassiveEQ update available"), Some(&body));
    dialog.add_responses(&[("later", "Later"), ("copy", "Copy command")]);
    if helper.is_some() && terminal.is_some() {
        dialog.add_response("update", "Update now");
        dialog.set_default_response(Some("update"));
        dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
    } else {
        dialog.add_response("aur", "Open AUR page");
        dialog.set_default_response(Some("copy"));
        dialog.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
    }
    dialog.set_close_response("later");
    let parent = window.clone();
    dialog.connect_response(None, move |_, response| match response {
        "update" => {
            if let (Some(helper), Some(terminal)) = (helper, terminal.as_deref())
                && !run_update(terminal, helper)
            {
                let failed = adw::AlertDialog::new(
                    Some("Could not open a terminal"),
                    Some(&format!(
                        "Run \u{201c}{command}\u{201d} in a terminal to update."
                    )),
                );
                failed.add_response("close", "Close");
                failed.present(Some(&parent));
            }
        }
        "copy" => {
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(&command);
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

fn show_restart_dialog(window: &adw::ApplicationWindow) {
    let dialog = adw::AlertDialog::new(
        Some("MassiveEQ was updated"),
        Some(
            "This editor is still the previous version. Restart it to use the new one; \
             your edits are saved first.",
        ),
    );
    dialog.add_responses(&[("later", "Later"), ("restart", "Restart")]);
    dialog.set_close_response("later");
    dialog.set_default_response(Some("restart"));
    dialog.set_response_appearance("restart", adw::ResponseAppearance::Suggested);
    let parent = window.clone();
    dialog.connect_response(Some("restart"), move |_, _| {
        // Start the new editor once this one has saved and exited, so it
        // does not just hand its window back to the old instance.
        let pid = std::process::id().to_string();
        let program = std::env::args_os()
            .next()
            .unwrap_or_else(|| "massiveeq".into());
        let relaunch = "while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; exec \"$2\"";
        let arguments = [
            OsStr::new("sh"),
            OsStr::new("-c"),
            OsStr::new(relaunch),
            OsStr::new("massiveeq-restart"),
            OsStr::new(&pid),
            program.as_os_str(),
        ];
        if gio::Subprocess::newv(&arguments, gio::SubprocessFlags::NONE).is_ok() {
            parent.close();
        }
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
    fn update_runs_the_helper_through_each_terminal_convention() {
        let tail = |command: &[String]| command[command.len() - 6..].to_vec();
        let kitty = terminal_command("kitty", "yay");
        assert_eq!(kitty[0], "kitty");
        assert_eq!(kitty[1], "bash");
        assert_eq!(
            tail(&kitty)[4..],
            ["yay".to_owned(), AUR_PACKAGE.to_owned()]
        );
        assert_eq!(
            terminal_command("/usr/bin/wezterm", "paru")[1..3],
            ["start", "--"]
        );
        assert_eq!(terminal_command("gnome-terminal", "yay")[1], "--");
        // Unknown terminals get the common -e convention.
        assert_eq!(terminal_command("myterm", "yay")[1], "-e");
    }

    #[test]
    fn malformed_update_responses_fail_closed() {
        assert!(!response_indicates_update("not json"));
        assert!(!response_indicates_update(r#"{"status":"ahead"}"#));
    }
}

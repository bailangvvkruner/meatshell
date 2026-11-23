//! Clipboard quick connect: strict parsing for SSH targets copied to the
//! system clipboard, plus the runtime-only session they create.
//!
//! A copied `ssh user@host` is only ever a *suggestion*: the prompt below
//! still needs an explicit confirmation, and the resulting session lives in
//! `RuntimeSessions` for the lifetime of the window. It is never written to
//! the persisted `ConfigStore`, never appears in quick connect, and never
//! asks for a password — the connection either succeeds with the local SSH
//! agent / default keys / `none` auth, or it fails.

/// Runtime-only sessions, keyed by session id. Populated by clipboard quick
/// connect and looked up by the normal connect path before the config store.
pub(super) type RuntimeSessions = Rc<RefCell<HashMap<String, Session>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClipboardSshTarget {
    pub(super) user: String,
    pub(super) host: String,
}

/// Parse a complete clipboard value as either `ssh user@host` or `user@host`.
///
/// The parser intentionally does not accept options, shell text, ports, or
/// newlines. A clipboard value is an invitation to ask the user about a
/// connection, so accepting only one unambiguous target keeps ordinary copied
/// prose and commands from opening a prompt.
pub(super) fn parse_clipboard_ssh_target(text: &str) -> Option<ClipboardSshTarget> {
    let input = text.trim();
    if input.is_empty() {
        return None;
    }

    let target = if let Some(rest) = input.strip_prefix("ssh") {
        // `sshx...` is not the SSH command form. Require whitespace after the
        // command and then trim only the separator before the target.
        if !rest.chars().next().is_some_and(char::is_whitespace) {
            return None;
        }
        let rest = rest.trim();
        if rest.split_whitespace().count() != 1 {
            return None;
        }
        rest
    } else {
        input
    };

    if target.is_empty() || target.chars().any(char::is_whitespace) {
        return None;
    }

    let (user, host) = target.split_once('@')?;
    if user.is_empty()
        || host.is_empty()
        || target.matches('@').count() != 1
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        || !valid_hostname(host)
    {
        return None;
    }

    Some(ClipboardSshTarget {
        user: user.to_string(),
        host: host.to_string(),
    })
}

fn valid_hostname(host: &str) -> bool {
    if host.len() > 253 || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// Another modal owns the window: do not stack the clipboard prompt on top of
/// it. The clipboard is re-checked the next time the window regains focus.
fn clipboard_prompt_blocked(win: &AppWindow) -> bool {
    win.get_modal_open()
}

/// Read the clipboard off the UI thread and surface only the parsed target to
/// Slint. The raw value is kept solely for per-value de-duplication and is
/// never logged or persisted.
pub(super) fn queue_clipboard_connect_prompt(
    weak: &slint::Weak<AppWindow>,
    prompted: &Arc<Mutex<Option<String>>>,
) {
    let weak = weak.clone();
    let prompted = prompted.clone();
    std::thread::spawn(move || {
        let Ok(text) = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text())
        else {
            return;
        };
        let raw = text.trim().to_string();
        let target = parse_clipboard_ssh_target(&raw);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = weak.upgrade() else { return };
            let changed = prompted
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .as_deref()
                != Some(raw.as_str());
            if !changed {
                return;
            }
            // A different clipboard value, including a non-SSH value, clears
            // the de-duplication marker so returning to the target later can
            // ask again. Do not retain the new value until it is prompted.
            if let Ok(mut value) = prompted.lock() {
                *value = None;
            }
            let Some(target) = target else { return };
            if clipboard_prompt_blocked(&win) {
                return;
            }
            if let Ok(mut value) = prompted.lock() {
                *value = Some(raw);
            }
            win.set_clipboard_connect_user(target.user.into());
            win.set_clipboard_connect_host(target.host.into());
            win.set_clipboard_connect_open(true);
        });
    });
}

/// Confirmation turns the target into a runtime-only session and then hands it
/// to the normal `connect-session` path, so tabs, SFTP and everything else
/// behave exactly like a saved session.
pub(super) fn wire_clipboard_connect(window: &AppWindow, runtime_sessions: RuntimeSessions) {
    {
        let weak = window.as_weak();
        window.on_clipboard_connect_confirmed(move || {
            let Some(win) = weak.upgrade() else { return };
            let user = win.get_clipboard_connect_user().to_string();
            let host = win.get_clipboard_connect_host().to_string();
            win.set_clipboard_connect_open(false);
            let Some(target) = parse_clipboard_ssh_target(&format!("{user}@{host}")) else {
                return;
            };
            let mut session = Session::new_empty();
            session.id = format!("runtime:clipboard:{}", uuid::Uuid::new_v4());
            session.name = format!("{}@{}", target.user, target.host);
            session.user = target.user;
            session.host = target.host;
            session.auth = AuthMethod::Password;
            session.runtime_passwordless = true;
            let id = session.id.clone();
            runtime_sessions.borrow_mut().insert(id.clone(), session);
            win.invoke_connect_session(id.into());
            win.set_terminal_focus_sequence(win.get_terminal_focus_sequence() + 1);
        });
    }
    {
        let weak = window.as_weak();
        window.on_clipboard_connect_cancelled(move || {
            if let Some(win) = weak.upgrade() {
                win.set_clipboard_connect_open(false);
                win.set_terminal_focus_sequence(win.get_terminal_focus_sequence() + 1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_clipboard_ssh_target, ClipboardSshTarget};

    fn target(user: &str, host: &str) -> ClipboardSshTarget {
        ClipboardSshTarget {
            user: user.into(),
            host: host.into(),
        }
    }

    #[test]
    fn accepts_bare_and_ssh_command_forms() {
        assert_eq!(
            parse_clipboard_ssh_target(
                "ssh cnb-o5g-1jvt0p2ht-001.898117c2-9a4f-45cc-b115-d634102bc01a-e48@cnb.space"
            ),
            Some(target(
                "cnb-o5g-1jvt0p2ht-001.898117c2-9a4f-45cc-b115-d634102bc01a-e48",
                "cnb.space"
            ))
        );
        assert_eq!(
            parse_clipboard_ssh_target("  user@host.example  "),
            Some(target("user", "host.example"))
        );
        assert_eq!(
            parse_clipboard_ssh_target("ssh\tuser@host.example"),
            Some(target("user", "host.example"))
        );
    }

    #[test]
    fn rejects_options_commands_and_ambiguous_values() {
        for value in [
            "",
            "ssh",
            "ssh -p 22 user@host.example",
            "ssh user@host.example && whoami",
            "ssh user@host.example other@host.example",
            "sshx user@host.example",
            "user@host.example extra",
            "user@host.example\nother@host.example",
            "user@@host.example",
            "@host.example",
            "user@",
            "not-an-email",
            "user@host:22",
            "user@host..example",
            "user@-host.example",
            "user@host-.example",
            "user name@host.example",
            "user@host.example/path",
        ] {
            assert_eq!(
                parse_clipboard_ssh_target(value),
                None,
                "accepted {value:?}"
            );
        }
    }
}

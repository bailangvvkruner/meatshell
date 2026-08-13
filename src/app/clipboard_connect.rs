//! Strict parsing for SSH targets copied to the system clipboard.

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

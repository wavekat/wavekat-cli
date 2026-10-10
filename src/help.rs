//! Account-aware top-level help.
//!
//! The platform grants each account a set of modules (`features` on
//! `/api/me`; new accounts get only `voice`), and some commands are
//! root-only. `wk --help` lists only the commands the signed-in account
//! can use, read from the role and modules that `wk login` / `wk me`
//! cache in the auth file — no network call.
//!
//! This is cosmetic only. Hidden commands still parse and run, and the
//! platform still enforces every grant.
//!
//! clap has no first-class way to group subcommands under headings, so
//! the command list is rendered here and swapped in via
//! `help_template`. Keep `SECTIONS` in sync with the `Command` enum in
//! `main.rs`.

use crate::config::AuthConfig;

/// The module every account has, assumed when nothing is cached yet.
const DEFAULT_MODULE: &str = "voice";

#[derive(Clone, Copy)]
enum Gate {
    Always,
    /// Needs this module in the account's `features`.
    Module(&'static str),
    Root,
}

struct Entry {
    name: &'static str,
    about: &'static str,
    gate: Gate,
}

const fn entry(name: &'static str, about: &'static str, gate: Gate) -> Entry {
    Entry { name, about, gate }
}

const SECTIONS: &[(&str, &[Entry])] = &[
    (
        "Account",
        &[
            entry(
                "login",
                "Authenticate against a WaveKat platform instance",
                Gate::Always,
            ),
            entry("logout", "Forget stored credentials", Gate::Always),
            entry(
                "me",
                "Show the currently signed-in user (`GET /api/me`)",
                Gate::Always,
            ),
        ],
    ),
    (
        "Resources",
        &[
            entry("projects", "Manage projects", Gate::Module("projects")),
            entry(
                "annotations",
                "Manage annotations",
                Gate::Module("labeling"),
            ),
            entry(
                "exports",
                "Manage dataset exports",
                Gate::Module("datasets"),
            ),
            entry(
                "models",
                "Manage trained models (push, list, download)",
                Gate::Module("models"),
            ),
            entry(
                "files",
                "Manage project files (list, reserve / unreserve test set)",
                Gate::Module("projects"),
            ),
        ],
    ),
    (
        "Admin",
        &[entry(
            "admin",
            "Root-only platform analytics and fleet tags (users, installs, usage, …) as JSON",
            Gate::Root,
        )],
    ),
    (
        "Raw API",
        &[entry(
            "api",
            "GET any platform endpoint and print its JSON (`wk api /api/me`)",
            Gate::Always,
        )],
    ),
    (
        "CLI",
        &[
            entry(
                "config",
                "Read or change persisted CLI preferences (telemetry, …)",
                Gate::Always,
            ),
            entry(
                "version",
                "Print the local CLI version and probe the platform's `/api/health`",
                Gate::Always,
            ),
            entry(
                "update",
                "Replace this binary with the latest release (or `--check` to peek)",
                Gate::Always,
            ),
            entry(
                "agents",
                "Print the bundled AGENTS.md guide for AI agents using `wk`",
                Gate::Always,
            ),
            entry(
                "help",
                "Print this message or the help of the given subcommand(s)",
                Gate::Always,
            ),
        ],
    ),
];

/// What the cached account is allowed to see.
pub struct Access {
    root: bool,
    /// `None` until `wk login` / `wk me` has cached the modules.
    features: Option<Vec<String>>,
    signed_in: bool,
}

impl Access {
    pub fn from_config(cfg: &AuthConfig) -> Self {
        Self {
            root: cfg.role.as_deref() == Some("root"),
            features: cfg.features.clone(),
            signed_in: cfg.token.is_some() || cfg.session_cookie.is_some(),
        }
    }

    fn allows(&self, gate: Gate) -> bool {
        match gate {
            Gate::Always => true,
            // Root sees every command, granted or not.
            Gate::Module(_) if self.root => true,
            Gate::Module(m) => match &self.features {
                Some(features) => features.iter().any(|f| f == m),
                None => m == DEFAULT_MODULE,
            },
            Gate::Root => self.root,
        }
    }

    /// Subcommands to hide from help (they stay callable).
    pub fn hidden_commands(&self) -> Vec<&'static str> {
        SECTIONS
            .iter()
            .flat_map(|(_, entries)| entries.iter())
            .filter(|e| !self.allows(e.gate))
            .map(|e| e.name)
            .collect()
    }

    pub fn help_template(&self) -> String {
        let mut out = String::from("{about-with-newline}\n{usage-heading} {usage}\n\n");
        for (heading, entries) in SECTIONS {
            let visible: Vec<_> = entries.iter().filter(|e| self.allows(e.gate)).collect();
            if visible.is_empty() {
                continue;
            }
            out.push_str(heading);
            out.push_str(":\n");
            for e in visible {
                out.push_str(&format!("  {:<13}{}\n", e.name, e.about));
            }
            out.push('\n');
        }
        // A login from before modules were cached: point at the refresh
        // rather than silently showing a voice-only list.
        if self.signed_in && self.features.is_none() && !self.root {
            out.push_str("Run `wk me` to list every command your account can use.\n\n");
        }
        out.push_str("Options:\n{options}{after-help}");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access(role: Option<&str>, features: Option<&[&str]>) -> Access {
        Access::from_config(&AuthConfig {
            token: Some("wk_test".into()),
            role: role.map(str::to_string),
            features: features.map(|f| f.iter().map(|s| s.to_string()).collect()),
            ..Default::default()
        })
    }

    #[test]
    fn voice_only_account_sees_no_resources_or_admin() {
        let a = access(Some("user"), Some(&["voice"]));
        let help = a.help_template();
        assert!(!help.contains("Resources:"), "{help}");
        assert!(!help.contains("admin"), "{help}");
        assert!(help.contains("  api "), "{help}");
        assert!(!help.contains("wk me` to list"), "{help}");
        assert_eq!(
            a.hidden_commands(),
            [
                "projects",
                "annotations",
                "exports",
                "models",
                "files",
                "admin"
            ]
        );
    }

    #[test]
    fn granted_modules_show_their_commands() {
        let a = access(Some("user"), Some(&["voice", "projects", "datasets"]));
        assert_eq!(a.hidden_commands(), ["annotations", "models", "admin"]);
        let help = a.help_template();
        assert!(help.contains("Resources:\n  projects "), "{help}");
        assert!(help.contains("  exports "), "{help}");
    }

    #[test]
    fn root_sees_everything() {
        let a = access(Some("root"), Some(&["voice"]));
        assert!(a.hidden_commands().is_empty());
        let help = a.help_template();
        assert!(help.contains("Admin:\n  admin "), "{help}");
    }

    #[test]
    fn uncached_login_defaults_to_voice_and_hints_refresh() {
        let a = access(None, None);
        assert!(a.hidden_commands().contains(&"projects"));
        assert!(a.help_template().contains("Run `wk me`"));
    }

    #[test]
    fn signed_out_gets_no_refresh_hint() {
        let a = Access::from_config(&AuthConfig::default());
        assert!(!a.help_template().contains("Run `wk me`"));
    }
}

//! Codex local session directory discovery.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

pub(crate) fn codex_sessions_dir_candidates(
    home_dir: Option<PathBuf>,
    codex_home: Option<String>,
    custom_dirs: &[String],
    wsl_roots: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    if let Some(sessions_dir) = codex_home.as_deref().and_then(normalize_codex_sessions_dir) {
        push_unique_path(&mut dirs, &mut seen, sessions_dir);
    } else if codex_home
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        && let Some(home) = home_dir
    {
        push_unique_path(&mut dirs, &mut seen, home.join(".codex").join("sessions"));
    }

    for custom_dir in custom_dirs {
        if let Some(sessions_dir) = normalize_codex_sessions_dir(custom_dir) {
            push_unique_path(&mut dirs, &mut seen, sessions_dir);
        }
    }

    for sessions_dir in discover_wsl_codex_sessions_dirs(wsl_roots) {
        push_unique_path(&mut dirs, &mut seen, sessions_dir);
    }

    dirs
}

pub(crate) fn default_wsl_roots() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }

    let preferred = PathBuf::from(r"\\wsl.localhost");
    if fs::read_dir(&preferred).is_ok() {
        return vec![preferred];
    }

    vec![PathBuf::from(r"\\wsl$")]
}

fn normalize_codex_sessions_dir(path: impl AsRef<str>) -> Option<PathBuf> {
    let trimmed = path.as_ref().trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = PathBuf::from(trimmed);
    let file_name = path.file_name().and_then(|name| name.to_str());
    if file_name.is_some_and(|name| name.eq_ignore_ascii_case("sessions")) {
        Some(path)
    } else {
        Some(path.join("sessions"))
    }
}

fn discover_wsl_codex_sessions_dirs(wsl_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();

    for wsl_root in wsl_roots {
        let Ok(distros) = fs::read_dir(wsl_root) else {
            continue;
        };

        for distro in distros.flatten() {
            let distro_path = distro.path();
            let homes_dir = distro_path.join("home");
            if let Ok(users) = fs::read_dir(&homes_dir) {
                for user in users.flatten() {
                    let sessions_dir = user.path().join(".codex").join("sessions");
                    if sessions_dir.exists() {
                        push_unique_path(&mut dirs, &mut seen, sessions_dir);
                    }
                }
            }

            let root_sessions_dir = distro_path.join("root").join(".codex").join("sessions");
            if root_sessions_dir.exists() {
                push_unique_path(&mut dirs, &mut seen, root_sessions_dir);
            }
        }
    }

    dirs
}

fn push_unique_path(dirs: &mut Vec<PathBuf>, seen: &mut HashSet<String>, path: PathBuf) {
    let key = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    if seen.insert(key) {
        dirs.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codex_root_to_sessions_dir() {
        assert_eq!(
            normalize_codex_sessions_dir(r"\\wsl.localhost\archlinux\home\kk\.codex"),
            Some(PathBuf::from(
                r"\\wsl.localhost\archlinux\home\kk\.codex\sessions"
            ))
        );
        assert_eq!(
            normalize_codex_sessions_dir(r"C:\Users\me\.codex\sessions"),
            Some(PathBuf::from(r"C:\Users\me\.codex\sessions"))
        );
        assert_eq!(normalize_codex_sessions_dir("  "), None);
    }

    #[test]
    fn discovers_wsl_codex_sessions_dirs_from_distro_homes() {
        let base = std::env::temp_dir().join(format!("codexbar-wsl-roots-{}", std::process::id()));
        let distro = base.join("Ubuntu");
        let user_sessions = distro
            .join("home")
            .join("alice")
            .join(".codex")
            .join("sessions");
        let root_sessions = distro.join("root").join(".codex").join("sessions");
        fs::create_dir_all(&user_sessions).unwrap();
        fs::create_dir_all(&root_sessions).unwrap();

        let dirs = discover_wsl_codex_sessions_dirs(std::slice::from_ref(&base));

        assert!(dirs.contains(&user_sessions));
        assert!(dirs.contains(&root_sessions));
        // Best-effort teardown of the temp tree; a removal error cannot
        // change the assertions already made above.
        let _removed_temp = fs::remove_dir_all(base);
    }

    #[test]
    fn blank_codex_home_falls_back_to_default_home_sessions_dir() {
        let home = PathBuf::from(r"C:\Users\me");
        let dirs =
            codex_sessions_dir_candidates(Some(home.clone()), Some("  ".to_string()), &[], &[]);

        assert_eq!(dirs, vec![home.join(".codex").join("sessions")]);
    }
}

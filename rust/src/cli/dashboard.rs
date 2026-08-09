//! `codexbar dashboard` — one-shot dashboard-v1 snapshot.
//!
//! Upstream 0.48.0: `CLIDashboardCommand.swift` (#2499 one-shot command,
//! #2716 `--identity redacted|full`, #2719 `--output <path>` atomic write).
//! POSIX `0644` mode bits are skipped per PORTING conventions (Windows).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Args;

use super::serve::dashboard::snapshot::DashboardIdentity;
use super::serve::dashboard::source::SnapshotProducer;

#[derive(Args, Debug, Clone)]
pub struct DashboardArgs {
    /// Pretty-print JSON output
    #[arg(long, default_value_t = false)]
    pub pretty: bool,

    /// Overall fetch timeout in seconds, 0...86400 (30; 0 disables)
    #[arg(long, default_value = "30")]
    pub timeout: f64,

    /// Account identity detail: redacted (default) or full. `full` exposes
    /// real account emails — for one-shot snapshots on trusted surfaces only.
    #[arg(long, value_parser = ["redacted", "full"], default_value = "redacted")]
    pub identity: String,

    /// Atomically write the snapshot to this file (temp sibling + fsync +
    /// rename) instead of stdout. The parent directory must already exist.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

pub async fn run(args: DashboardArgs) -> anyhow::Result<()> {
    let Some(identity) = DashboardIdentity::parse(&args.identity) else {
        anyhow::bail!("--identity must be redacted or full.");
    };
    let fetch_timeout = parse_timeout(args.timeout)?;

    let producer = SnapshotProducer::new(60, identity).with_fetch_timeout(fetch_timeout);
    let payload = producer.collect().await.map_err(anyhow::Error::msg)?;

    let body = if args.pretty {
        serde_json::to_string_pretty(&payload)?
    } else {
        serde_json::to_string(&payload)?
    };

    match &args.output {
        Some(path) => {
            write_atomic(path, body.as_bytes())?;
            eprintln!("Dashboard snapshot written to {}", path.display());
        }
        None => println!("{body}"),
    }
    Ok(())
}

/// `--timeout <seconds>`: 0 disables the (outer) fetch envelope; otherwise
/// clamp to 0...86400 like upstream's Commander validation.
fn parse_timeout(seconds: f64) -> anyhow::Result<Option<std::time::Duration>> {
    if seconds.is_nan() || !(0.0..=86_400.0).contains(&seconds) {
        anyhow::bail!("--timeout must be within 0...86400 seconds.");
    }
    if seconds == 0.0 {
        Ok(None)
    } else {
        Ok(Some(std::time::Duration::from_secs_f64(seconds)))
    }
}

/// Atomic snapshot write: temp sibling file + fsync + rename. The rename
/// replaces an existing target on Windows (`std::fs::rename` uses
/// `MOVEFILE_REPLACE_EXISTING`). Upstream's `0644` POSIX mode does not apply.
/// On failure the temp sibling is truncated to zero bytes rather than deleted
/// (harness policy: no delete APIs); zero-length `.tmp-*` siblings are
/// harmless and overwritten by the next run.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        anyhow::bail!(
            "output directory does not exist: {} (it is not created)",
            parent.display()
        );
    }
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(format!(".tmp-{}", std::process::id()));
    let temp = PathBuf::from(temp_name);

    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err()
        && let Ok(file) = std::fs::OpenOptions::new().write(true).open(&temp)
    {
        let _ = file.set_len(0);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch path per test case; leftovers in %TEMP% are harmless
    /// (harness policy: no delete APIs in tests either).
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("codexbar-dashboard-{name}-{}", std::process::id()))
    }

    #[test]
    fn timeout_validation() {
        assert!(parse_timeout(0.0).unwrap().is_none());
        assert_eq!(
            parse_timeout(30.0).unwrap().unwrap(),
            std::time::Duration::from_secs(30)
        );
        assert!(parse_timeout(-1.0).is_err());
        assert!(parse_timeout(86_401.0).is_err());
        assert!(parse_timeout(f64::NAN).is_err());
    }

    #[test]
    fn atomic_write_creates_and_replaces_without_temp_leftover() {
        let dir = scratch("replace");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("snapshot.json");

        write_atomic(&target, b"{\"v\":1}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"v\":1}");

        write_atomic(&target, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"v\":2}");

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp siblings must not survive: {leftovers:?}"
        );
    }

    #[test]
    fn atomic_write_refuses_missing_parent() {
        let target = scratch("missing-parent")
            .join("subdir")
            .join("snapshot.json");
        let err = write_atomic(&target, b"{}").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn atomic_write_bare_relative_name_uses_cwd_as_parent() {
        // `--output bare-name.json` yields an empty `Path::parent()`; the fix
        // treats that as `.` so the snapshot lands in the working directory.
        // This is the only test that relies on the process CWD, and it restores
        // it before returning; every other test uses absolute temp paths, so a
        // transient chdir cannot corrupt their IO.
        let original = std::env::current_dir().unwrap();
        let dir = scratch("bare-relative");
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_current_dir(&dir).unwrap();
        // A defer-style guard restores the CWD even if an assertion panics,
        // so a failed test cannot strand the suite in the temp directory.
        struct CwdGuard {
            original: PathBuf,
            restore: bool,
        }
        impl Drop for CwdGuard {
            fn drop(&mut self) {
                if self.restore {
                    let _ = std::env::set_current_dir(&self.original);
                }
            }
        }
        let mut cwd_guard = CwdGuard {
            original: original.clone(),
            restore: true,
        };
        let bare = PathBuf::from("bare-name.json");
        write_atomic(&bare, b"{\"v\":1}").unwrap();
        assert_eq!(std::fs::read(&bare).unwrap(), b"{\"v\":1}");

        // Replacement must reuse the same bare path.
        write_atomic(&bare, b"{\"v\":2}").unwrap();
        assert_eq!(std::fs::read(&bare).unwrap(), b"{\"v\":2}");

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp siblings must not survive: {leftovers:?}"
        );
        // Success: suppress the manual restore (the guard handles it) and
        // confirm the CWD is restored for the rest of the suite.
        cwd_guard.restore = false;
        std::env::set_current_dir(&original).unwrap();
    }
}

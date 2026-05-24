//! Discovery of the global and local config directories. Owned here so
//! `format.rs` and `keys.rs` don't drift on path logic.

use std::path::PathBuf;

/// Source layer a piece of config came from. Embedded in compiled
/// `LogFormat` and `Group` entries so `--list-formats` can annotate them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigSource {
    #[default]
    Builtin,
    Global,
    Local,
}

/// Resolve the global config directory.
///
/// Priority:
/// 1. `$TESS_GLOBAL_CONFIG_DIR` if set — returned as-is, **not**
///    existence-checked (so tests and CI can point at a fresh tempdir
///    before populating it).
/// 2. `/etc/tess` if it exists as a directory. The existence probe is
///    deliberate so a default-empty install doesn't claim a global
///    layer that isn't there.
/// 3. Otherwise `None`.
pub fn global_config_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("TESS_GLOBAL_CONFIG_DIR") {
        return Some(PathBuf::from(v));
    }
    let etc = PathBuf::from("/etc/tess");
    if etc.is_dir() {
        return Some(etc);
    }
    None
}

/// Resolve the per-user config directory (`~/.config/tess`). Returns
/// `None` if `$HOME` is not set.
pub fn user_config_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        let mut p = PathBuf::from(h);
        p.push(".config");
        p.push("tess");
        p
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate env vars. Same pattern as the
    /// HOME_LOCK in `format.rs`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn global_config_dir_honors_env_var() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", "/tmp/tess-test-global");
        assert_eq!(
            global_config_dir(),
            Some(PathBuf::from("/tmp/tess-test-global"))
        );
        match prev {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn global_config_dir_with_no_env_var_returns_etc_or_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");
        std::env::remove_var("TESS_GLOBAL_CONFIG_DIR");
        let result = global_config_dir();
        // `/etc/tess` likely doesn't exist on the dev machine, but if it
        // does the result is the etc path — accept either as long as it's
        // not garbage.
        if let Some(p) = &result {
            assert_eq!(p, &PathBuf::from("/etc/tess"));
        }
        match prev {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn user_config_dir_appends_config_tess() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", "/tmp/fakehome");
        assert_eq!(
            user_config_dir(),
            Some(PathBuf::from("/tmp/fakehome/.config/tess"))
        );
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

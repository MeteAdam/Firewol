//! Context-menu actions that start other programs: custom actions and "Run as administrator".
//! Every target is re-verified by identity first, and elevation always goes through the policy.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use windows::Win32::Foundation::HWND;

use super::elevation::{self, ElevationPolicy, executable_id, validate_executable};
use super::{ItemId, attributes, display_path};

/// Starts `executable` with the target's path as its only argument. A folder target is also the
/// working directory; a file target runs in its parent folder. Administrator actions ask Windows
/// for consent through the elevation policy and block until the UAC prompt is answered.
pub fn run_custom(
    executable: &Path,
    administrator: bool,
    target: &ItemId,
    policy: &ElevationPolicy,
    owner: HWND,
) -> Result<()> {
    let (directory, ..) = attributes(&target.verify()?)?;
    validate_executable(executable)?;
    if administrator {
        return elevation::launch(policy, &executable_id(executable)?, Some(target), owner);
    }
    let folder = if directory {
        target.path()
    } else {
        target.path().parent().context("The target has no folder")?
    };
    Command::new(executable)
        .arg(display_path(target.path()))
        .current_dir(folder)
        .spawn()
        .with_context(|| format!("Launch {}", executable.display()))?;
    Ok(())
}

/// Starts an allowlisted executable elevated. Blocks until the UAC prompt is answered.
pub fn run_as_administrator(program: &ItemId, policy: &ElevationPolicy, owner: HWND) -> Result<()> {
    let (directory, ..) = attributes(&program.verify()?)?;
    anyhow::ensure!(!directory, "Select an executable file");
    elevation::launch(policy, program, None, owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::elevation::ElevationMode;

    #[test]
    fn standard_policy_refuses_elevation_and_bad_executables() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let file = fixture.path().join("note.txt");
        std::fs::write(&file, "x")?;
        let target = ItemId::at(file)?;
        let policy = ElevationPolicy::new(ElevationMode::Standard, &[])?;
        let none = HWND::default();
        assert!(run_custom(Path::new("relative.exe"), false, &target, &policy, none).is_err());
        assert!(
            run_custom(
                &fixture.path().join("tool.bat"),
                false,
                &target,
                &policy,
                none
            )
            .is_err()
        );
        let system = std::env::var_os("SystemRoot")
            .map(|root| Path::new(&root).join("System32\\whoami.exe"));
        if let Some(program) = system.filter(|path| path.exists()) {
            let error = run_custom(&program, true, &target, &policy, none).unwrap_err();
            assert!(format!("{error:#}").contains("disabled in Admin settings"));
            let error = run_as_administrator(&executable_id(&program)?, &policy, none).unwrap_err();
            assert!(format!("{error:#}").contains("disabled in Admin settings"));
        }
        Ok(())
    }
}

#![cfg(windows)]

use std::fs::{self, File};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use firewol_core::explorer::elevation::{ElevationMode, ElevationPolicy, executable_id};
use firewol_core::explorer::{
    ItemId, MenuAction, Operations, Settings, TransferControl, TransferRequest,
};

fn request(items: &[&Path], into: &Path, move_items: bool) -> Result<TransferRequest> {
    Ok(TransferRequest {
        items: items
            .iter()
            .map(|path| ItemId::at(path.to_path_buf()))
            .collect::<Result<_>>()?,
        into: ItemId::at(into.to_owned())?,
        move_items,
    })
}

#[test]
fn batch_keeps_conflicts_and_undoes_only_completed_files() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let into = root.join("destination");
    fs::create_dir(&into)?;
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    let three = root.join("three.txt");
    fs::write(&one, b"one")?;
    fs::write(root.join("one.txt:verified"), b"named stream")?;
    fs::write(&two, b"two")?;
    fs::write(&three, b"three")?;
    fs::write(into.join("two.txt"), b"keep this")?;

    let mut operations = Operations::open()?;
    let control = TransferControl::default();
    let result = operations.transfer(request(&[&one, &two, &three], &into, false)?, &control)?;
    assert_eq!(result.transferred.len(), 2);
    assert!(result.message.contains("2/3"));
    assert_eq!(control.progress().finished, 3);
    assert_eq!(fs::read(into.join("one.txt"))?, b"one");
    assert_eq!(fs::read(into.join("one.txt:verified"))?, b"named stream");
    assert_eq!(fs::read(into.join("two.txt"))?, b"keep this");
    assert_eq!(fs::read(into.join("three.txt"))?, b"three");
    assert_eq!(fs::read_dir(&into)?.count(), 3);
    operations.undo()?;
    operations.undo()?;
    assert_eq!(fs::read_dir(into)?.count(), 1);
    assert_eq!(fs::read(one)?, b"one");
    Ok(())
}

#[test]
fn elevation_policy_defaults_to_standard_and_rejects_a_replacement() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let tool = fixture.path().join("action.exe");
    fs::write(&tool, b"fixture used to test identity, never launched")?;
    let id = executable_id(&tool)?;
    assert!(!ElevationPolicy::default().allows(&id));
    fs::write(
        fixture.path().join("explorer.json"),
        serde_json::to_vec(&serde_json::json!({
            "menu": [{"action": {"custom": {"executable": tool}}, "label": "Existing action"}]
        }))?,
    )?;
    let legacy = Settings::load(fixture.path())?;
    assert_eq!(legacy.elevation.mode, ElevationMode::Standard);
    assert!(
        legacy
            .menu
            .iter()
            .any(|entry| entry.action == MenuAction::RunAsAdministrator)
    );
    assert!(legacy.menu.iter().any(|entry| matches!(
        entry.action,
        MenuAction::Custom {
            administrator: false,
            ..
        }
    )));

    let policy = ElevationPolicy::new(ElevationMode::Allowlist, std::slice::from_ref(&tool))?;
    Settings::update(fixture.path(), |settings| settings.elevation = policy)?;
    let loaded = Settings::load(fixture.path())?;
    assert!(loaded.elevation.allows(&id));
    fs::rename(&tool, fixture.path().join("previous.exe"))?;
    fs::write(&tool, b"replacement")?;
    let replaced = executable_id(&tool)?;
    assert!(!loaded.elevation.allows(&replaced));
    assert!(ElevationPolicy::new(ElevationMode::AllExecutables, &[])?.allows(&replaced));
    Ok(())
}

#[test]
#[ignore = "Copies two 192 MiB fixtures and exercises CopyFile2 restart headers"]
fn pauses_resumes_and_cancels_an_in_flight_copy() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let into = root.join("destination");
    fs::create_dir(&into)?;
    let source = root.join("pause-火.bin");
    let length = 192 * 1024 * 1024;
    File::create(&source)?.set_len(length)?;

    for cancel in [false, true] {
        let transfer = request(&[&source], &into, false)?;
        let control = TransferControl::default();
        let target = into.join("pause-火.bin");
        let result = thread::scope(|scope| -> Result<()> {
            let task = scope.spawn(|| Operations::open()?.transfer(transfer, &control));
            let check = (|| {
                wait_for(|| control.progress().bytes > 0 || task.is_finished())?;
                let planned = control.progress();
                ensure!(
                    planned.total_bytes == length && planned.total_items == 1,
                    "The transfer did not plan its size"
                );
                ensure!(
                    !task.is_finished(),
                    "Copy completed before requesting a pause"
                );
                control.toggle_pause();
                wait_for(|| control.progress().paused_copies > 0 || task.is_finished())?;
                ensure!(
                    control.progress().paused_copies > 0,
                    "CopyFile2 did not return ERROR_REQUEST_PAUSED"
                );
                ensure!(
                    !target.exists(),
                    "A paused copy became visible at its final name"
                );
                Ok(())
            })();
            if cancel || check.is_err() {
                control.cancel();
            } else {
                control.toggle_pause();
            }
            let result = task.join().expect("Transfer worker panicked");
            check?;
            if cancel {
                assert!(
                    format!("{:#}", result.err().context("Cancelled copy completed")?)
                        .contains("Transfer cancelled")
                );
            } else {
                result?;
            }
            Ok(())
        });
        result?;
        if cancel {
            assert!(!target.exists());
            assert_eq!(fs::read_dir(&into)?.count(), 0);
        } else {
            assert_eq!(fs::metadata(&target)?.file_size(), length);
            fs::remove_file(target)?;
        }
        assert!(source.exists());
    }
    Ok(())
}

fn wait_for(mut ready: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        ensure!(
            Instant::now() < deadline,
            "Timed out waiting for the transfer"
        );
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
#[ignore = "Set FIREWOL_M3_OTHER_VOLUME to a writable directory on a second fixed local volume"]
fn cross_volume_move_verifies_streams_and_preserves_undo_conflicts() -> Result<()> {
    let first = tempfile::tempdir()?;
    let second = tempfile::Builder::new().prefix("firewol-m3-").tempdir_in(
        std::env::var_os("FIREWOL_M3_OTHER_VOLUME").context("Set FIREWOL_M3_OTHER_VOLUME")?,
    )?;
    let source_root = first.path().canonicalize()?;
    let target_root = second.path().canonicalize()?;
    let source = source_root.join("İzmir-火.txt");
    let target = target_root.join("İzmir-火.txt");
    let source_stream = source_root.join("İzmir-火.txt:meta");
    let target_stream = target_root.join("İzmir-火.txt:meta");
    fs::write(&source, b"content")?;
    fs::write(&source_stream, b"metadata")?;

    let mut operations = Operations::open()?;
    let result = operations.transfer(
        request(&[&source], &target_root, true)?,
        &TransferControl::default(),
    )?;
    assert_eq!(result.message, "Verified and moved file");
    assert!(!source.exists());
    assert_eq!(fs::read(&target)?, b"content");
    assert_eq!(fs::read(&target_stream)?, b"metadata");

    fs::write(&source, b"keep the conflicting file")?;
    assert!(operations.undo().is_err());
    assert_eq!(fs::read(&source)?, b"keep the conflicting file");
    assert_eq!(fs::read(&target)?, b"content");
    fs::remove_file(&source)?;
    operations.undo()?;
    assert!(!target.exists());
    assert_eq!(fs::read(&source)?, b"content");
    assert_eq!(fs::read(&source_stream)?, b"metadata");

    let original_permissions = fs::metadata(&source)?.permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions.clone())?;
    let result = operations.transfer(
        request(&[&source], &target_root, true)?,
        &TransferControl::default(),
    );
    fs::set_permissions(&source, original_permissions)?;
    let error = result
        .err()
        .context("Read-only move unexpectedly completed")?;
    assert!(format!("{error:#}").contains("The transfer was rolled back"));
    assert_eq!(fs::read(&source)?, b"content");
    assert_eq!(fs::read_dir(target_root)?.count(), 0);
    Ok(())
}

#[test]
fn a_transfer_sweeps_stages_left_by_an_interrupted_one() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let into = root.join("destination");
    fs::create_dir(&into)?;
    let stage = |name: &str, files: &[&str]| -> Result<std::path::PathBuf> {
        let folder = into.join(name);
        fs::create_dir(&folder)?;
        for file in files {
            fs::write(folder.join(file), b"partial copy")?;
        }
        Ok(folder)
    };
    let stale = stage(".firewol-Ab12Cd", &["payload", "firewol-stage"])?;
    let empty = stage(".firewol-000000", &["firewol-stage"])?;
    let running = stage(".firewol-Zz9999", &["payload", "firewol-stage"])?;
    let foreign = stage(
        ".firewol-q1w2e3",
        &["payload", "notes.txt", "firewol-stage"],
    )?;
    let other = stage(".firewol-user", &["payload", "firewol-stage"])?;
    // The same shape as a stage, but Firewol never marked these.
    let lookalike = stage(".firewol-backup", &["payload"])?;
    let empty_lookalike = stage(".firewol-photos", &[])?;
    // A running transfer holds its stage open without sharing deletion.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .custom_flags(0x0200_0000)
        .open(&running)?;
    let source = root.join("source.txt");
    fs::write(&source, b"fresh")?;

    let mut operations = Operations::open()?;
    operations.transfer(
        request(&[&source], &into, false)?,
        &TransferControl::default(),
    )?;
    assert_eq!(fs::read(into.join("source.txt"))?, b"fresh");
    assert!(
        !stale.exists() && !empty.exists(),
        "stale stages are removed"
    );
    assert!(running.join("payload").exists(), "a stage in use stays");
    assert!(foreign.join("notes.txt").exists() && other.join("payload").exists());
    assert!(
        lookalike.join("payload").exists() && empty_lookalike.exists(),
        "a folder Firewol did not mark is never touched"
    );
    drop(held);
    Ok(())
}

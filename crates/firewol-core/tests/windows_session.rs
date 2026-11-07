#![cfg(windows)]

use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, ensure};
use firewol_core::explorer::{
    Completion, FolderKey, FolderStyle, History, ItemId, Operation, Rgb, Session, Settings,
};

#[test]
fn history_steps_back_and_forward_and_drops_forward_entries() {
    let mut history = History::default();
    assert_eq!(history.peek(false), None);
    for name in ["a", "b", "c"] {
        history.visit(PathBuf::from(name));
    }
    history.visit(PathBuf::from("c"));
    let (back, path) = history.peek(false).expect("an earlier folder");
    assert_eq!(path, PathBuf::from("b"));
    history.go(back);
    assert_eq!(history.current(), Some(PathBuf::from("b").as_path()));
    assert_eq!(
        history.peek(true).map(|(_, path)| path.to_owned()),
        Some("c".into())
    );
    history.visit(PathBuf::from("d"));
    assert_eq!(history.peek(true), None);
    assert_eq!(
        history.peek(false).map(|(_, path)| path.to_owned()),
        Some("b".into())
    );
    for index in 0..100 {
        history.visit(PathBuf::from(format!("folder-{index}")));
    }
    let mut steps = 0;
    while let Some((position, _)) = history.peek(false) {
        history.go(position);
        steps += 1;
    }
    assert_eq!(steps, 63, "history keeps 64 folders");
}

fn wait(done: &mpsc::Receiver<Completion>) -> Result<Completion> {
    Ok(done.recv_timeout(Duration::from_secs(30))?)
}

#[test]
fn session_cuts_pastes_renames_and_undoes_by_identity() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let (source, destination) = (root.join("source"), root.join("destination"));
    fs::create_dir(&source)?;
    fs::create_dir(&destination)?;
    let file = source.join("rapor-火.txt");
    fs::write(&file, b"contents")?;
    let (sender, done) = mpsc::channel();
    let session = Session::open(root.join("data"), move |completion| {
        let _ = sender.send(completion);
    })?;

    session.set_clipboard(vec![ItemId::at(file.clone())?], true)?;
    session.paste(ItemId::at(destination.clone())?)?;
    let pasted = wait(&done)?;
    ensure!(pasted.succeeded, "paste failed: {}", pasted.message);
    let moved = destination.join("rapor-火.txt");
    assert!(!file.exists() && fs::read(&moved)? == b"contents");
    assert!(
        session.clipboard().is_none(),
        "a completed move empties the clipboard"
    );
    assert!(!session.busy());

    session.run(Operation::Rename {
        item: ItemId::at(moved.clone())?,
        name: "yeni-ad.txt".into(),
    })?;
    assert!(wait(&done)?.succeeded);
    let renamed = destination.join("yeni-ad.txt");
    assert!(renamed.exists());

    session.undo()?;
    assert!(wait(&done)?.succeeded);
    assert!(moved.exists() && !renamed.exists());
    session.undo()?;
    let undone = wait(&done)?;
    ensure!(undone.succeeded, "undo failed: {}", undone.message);
    assert_eq!(fs::read(&file)?, b"contents");

    session.set_clipboard(vec![ItemId::at(file.clone())?], false)?;
    fs::write(destination.join("rapor-火.txt"), b"conflict")?;
    session.paste(ItemId::at(destination.clone())?)?;
    let conflict = wait(&done)?;
    assert!(!conflict.succeeded && conflict.refresh);
    assert_eq!(fs::read(destination.join("rapor-火.txt"))?, b"conflict");
    assert!(session.clipboard().is_some(), "a copy keeps the clipboard");
    Ok(())
}

#[test]
fn session_pins_persist_and_reject_empty_clipboard_actions() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let data = root.join("data");
    let folder = root.join("İzmir");
    fs::create_dir(&folder)?;
    {
        let session = Session::open(data.clone(), |_| {})?;
        assert!(session.toggle_pin(&folder)?);
        assert!(session.paste(ItemId::at(folder.clone())?).is_err());
        assert!(session.set_clipboard(Vec::new(), false).is_err());
    }
    let session = Session::open(data, |_| {})?;
    assert_eq!(session.settings().pins, vec![folder.clone()]);
    assert!(!session.toggle_pin(&folder)?);
    assert!(session.settings().pins.is_empty());
    Ok(())
}

#[test]
fn folder_styles_follow_identity_normalize_and_clear() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let data = root.join("data");
    let folder = root.join("Projects");
    fs::create_dir(&folder)?;
    let file = root.join("note.txt");
    fs::write(&file, "x")?;
    let session = Session::open(data.clone(), |_| {})?;
    let id = ItemId::at(folder.clone())?;
    let style = FolderStyle {
        alias: "  Work  ".into(),
        color: Some(Rgb(1, 2, 3)),
        icon: " ★ ".into(),
        tags: vec![" alpha".into(), "".into(), "beta".into(), "alpha".into()],
    };
    session.set_folder_style(&id, style)?;
    // Renaming keeps the identity, so the style follows the folder.
    let renamed = root.join("Renamed");
    fs::rename(&folder, &renamed)?;
    let saved = Settings::load(&data)?;
    let stored = &saved.folders[&FolderKey::from(&ItemId::at(renamed.clone())?)];
    assert_eq!(stored.alias, "Work");
    assert_eq!(stored.icon, "★");
    assert_eq!(stored.tags, ["alpha", "beta"]);
    assert!(
        session
            .set_folder_style(&ItemId::at(file)?, FolderStyle::default())
            .is_err()
    );
    let too_many = FolderStyle {
        tags: (0..9).map(|index| format!("t{index}")).collect(),
        ..FolderStyle::default()
    };
    let current = ItemId::at(renamed.clone())?;
    assert!(session.set_folder_style(&current, too_many).is_err());
    assert_eq!(session.settings().folders.len(), 1);
    // The stale path no longer verifies, so a replaced folder is never restyled.
    assert!(
        session
            .set_folder_style(&id, FolderStyle::default())
            .is_err()
    );
    let blank = FolderStyle {
        alias: "   ".into(),
        ..FolderStyle::default()
    };
    session.set_folder_style(&current, blank)?;
    assert!(Settings::load(&data)?.folders.is_empty());
    Ok(())
}

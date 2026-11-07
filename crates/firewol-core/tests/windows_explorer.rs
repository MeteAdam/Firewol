#![cfg(windows)]

#[test]
fn natural_sort_keeps_folders_first_and_pages_stable() -> anyhow::Result<()> {
    use firewol_core::explorer::{Cancellation, Directory, Sort, SortColumn, page};
    let fixture = tempfile::tempdir()?;
    for (name, size) in [("file10.txt", 10), ("file2.txt", 2), ("İzmir火.txt", 1)] {
        std::fs::write(fixture.path().join(name), vec![0u8; size])?;
    }
    std::fs::create_dir(fixture.path().join("z-folder"))?;
    let cancel = Cancellation::default();
    let mut directory = Directory::open(fixture.path())?;
    directory.advance(2, &cancel)?;
    directory.sort(Sort::default(), &cancel)?;
    assert!(
        !directory.is_sorted(),
        "first pages do not wait for full sorting"
    );
    while !directory.is_complete() {
        directory.advance(128, &cancel)?;
    }
    directory.sort(Sort::default(), &cancel)?;
    let items = directory.list_dir(page(0), &cancel)?.items;
    assert!(items[0].directory);
    let names: Vec<_> = items
        .iter()
        .map(|item| item.name().to_string_lossy())
        .collect();
    assert!(
        names.iter().position(|name| name == "file2.txt")
            < names.iter().position(|name| name == "file10.txt")
    );
    directory.sort(
        Sort {
            column: SortColumn::Size,
            descending: true,
        },
        &cancel,
    )?;
    let items = directory.list_dir(page(0), &cancel)?.items;
    assert!(items[0].directory);
    assert_eq!(items[1].name(), "file10.txt");
    assert_eq!(
        directory.position(std::ffi::OsStr::new("file2.txt"))?,
        Some(2)
    );
    Ok(())
}

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Barrier, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Result, ensure};
use firewol_core::explorer::{
    Cancellation, Directory, Filters, FolderKey, FolderStyle, ItemId, Operation, Operations,
    Preset, Rgb, SearchIndex, SearchQuery, Settings, page,
};

#[test]
fn shell_operations_verify_identities_and_preserve_conflicts() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let destination = root.join("destination");
    fs::create_dir(&destination)?;
    let source = root.join("özge-火.txt");
    fs::write(&source, b"original")?;
    let mut operations = Operations::open()?;

    let id = || ItemId::at(source.clone());
    let into = || ItemId::at(destination.clone());
    let copy = destination.join("özge-火.txt");
    fs::write(&copy, b"conflict")?;
    assert!(
        operations
            .submit_operation(Operation::Copy {
                item: id()?,
                into: into()?
            })
            .is_err()
    );
    assert_eq!(fs::read(&copy)?, b"conflict");
    fs::remove_file(&copy)?;
    operations.submit_operation(Operation::Copy {
        item: id()?,
        into: into()?,
    })?;
    assert_eq!(fs::read(&copy)?, b"original");
    operations.undo()?;
    assert!(!copy.exists());

    operations.submit_operation(Operation::Move {
        item: id()?,
        into: into()?,
    })?;
    assert!(!source.exists());
    fs::write(&source, b"replacement")?;
    assert!(operations.undo().is_err());
    assert_eq!(fs::read(&source)?, b"replacement");
    assert_eq!(fs::read(&copy)?, b"original");
    fs::remove_file(&source)?;
    operations.undo()?;
    assert_eq!(fs::read(&source)?, b"original");

    operations.submit_operation(Operation::Rename {
        item: id()?,
        name: "yeniden-火.txt".into(),
    })?;
    assert_eq!(fs::read(root.join("yeniden-火.txt"))?, b"original");
    operations.undo()?;
    operations.submit_operation(Operation::Rename {
        item: id()?,
        name: "Özge-火.txt".into(),
    })?;
    assert!(
        fs::read_dir(&root)?
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name() == "Özge-火.txt")
    );
    operations.undo()?;
    let stale = id()?;
    let held = root.join("original.txt");
    fs::rename(&source, &held)?;
    fs::write(&source, b"different item")?;
    assert!(
        operations
            .submit_operation(Operation::Recycle { item: stale })
            .is_err()
    );
    assert_eq!(fs::read(&source)?, b"different item");

    let original = ItemId::at(held.clone())?;
    operations.submit_operation(Operation::Recycle {
        item: original.clone(),
    })?;
    fs::write(&held, b"new file at old path")?;
    assert!(operations.undo().is_err());
    assert_eq!(fs::read(&held)?, b"new file at old path");
    fs::remove_file(&held)?;
    operations.undo()?;
    original.verify()?;
    assert_eq!(fs::read(held)?, b"original");

    let tree = root.join("interrupted");
    fs::create_dir(&tree)?;
    fs::write(tree.join("a-copyable.txt"), b"first file")?;
    let locked = tree.join("z-locked.txt");
    fs::write(&locked, b"preserve this file")?;
    let guard = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&locked)?;
    assert!(
        operations
            .submit_operation(Operation::Copy {
                item: ItemId::at(tree.clone())?,
                into: into()?
            })
            .is_err()
    );
    assert!(!destination.join("interrupted").exists());
    drop(guard);
    assert_eq!(fs::read(locked)?, b"preserve this file");
    Ok(())
}

#[test]
fn folder_search_pages_filters_and_observes_external_changes() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let folder = fixture.path().join("search");
    let data = folder.join("app-data");
    fs::create_dir_all(folder.join("subfolder"))?;
    let folder = folder.canonicalize()?;
    let small = folder.join("resume.txt");
    let original = folder.join("subfolder").join("résumé-火.txt");
    fs::write(small, b"a")?;
    fs::write(&original, b"valid text")?;
    for i in 0..140 {
        fs::write(folder.join(format!("record-{i:03}.txt")), b"")?;
    }
    let index = SearchIndex::create(&data)?;
    let mut listing = Directory::open(&folder)?;
    let cancel = Cancellation::default();
    listing.advance(128, &cancel)?;
    let first = listing.list_dir(page(0), &cancel)?;
    assert_eq!(first.items.len(), 128);
    assert!(first.has_more);
    listing.advance(128, &cancel)?;
    assert!(listing.is_complete());
    assert!(!listing.list_dir(page(128), &cancel)?.has_more);
    let (updated, ready) = mpsc::channel();
    let task_index = index.clone();
    let task_cancel = cancel.clone();
    let task = thread::spawn(move || {
        task_index.watch_folder(&folder, &data, &task_cancel, |count| {
            let _ = updated.send(count);
        })
    });
    let tested = (|| -> Result<()> {
        ready.recv_timeout(Duration::from_secs(5))?;
        let mut reader = index.reader()?;
        let search = |text: &str, offset| SearchQuery {
            text: text.into(),
            regex: false,
            filters: Filters::default(),
            page: page(offset),
        };
        let matches = reader.search(&search("record", 0), &cancel)?;
        assert_eq!(matches.items.len(), 128);
        assert!(matches.has_more);
        assert_eq!(
            reader.search(&search("record", 128), &cancel)?.items.len(),
            12
        );
        assert_eq!(
            reader.search(&search("RÉ", 0), &cancel)?.items[0].path,
            original
        );
        let mut regex = search(r"^r.*\.txt$", 0);
        regex.regex = true;
        regex.filters = Filters {
            extension: Some(".TXT".into()),
            min_bytes: Some(3),
            max_bytes: Some(10),
            ..Filters::default()
        };
        assert_eq!(reader.search(&regex, &cancel)?.items[0].path, original);

        let changed = original.with_file_name("external-change.txt");
        fs::rename(&original, &changed)?;
        ready.recv_timeout(Duration::from_secs(5))?;
        let results = reader.search(&search("external-change", 0), &cancel)?;
        ensure!(
            results.items.len() == 1 && results.items[0].path == changed,
            "The change was not reindexed"
        );
        assert!(
            reader
                .search(&search("résumé", 0), &cancel)?
                .items
                .is_empty()
        );
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert!(reader.search(&search("record", 0), &cancelled).is_err());
        Ok(())
    })();
    cancel.cancel();
    let watched = task.join().expect("Folder watcher panicked");
    tested?;
    watched
}

#[test]
fn directory_pages_preserve_long_unicode_paths() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let mut directory: PathBuf = fixture.path().canonicalize()?;
    for _ in 0..5 {
        directory.push("uzun-klasör-火-".repeat(4));
    }
    fs::create_dir_all(&directory)?;
    let expected = directory.join("𝌆-İzmir-火.txt");
    fs::write(&expected, b"long path")?;
    let cancel = Cancellation::default();
    let mut view = Directory::open(&directory)?;
    view.advance(128, &cancel)?;
    let entries = view.list_dir(page(0), &cancel)?;
    assert_eq!(entries.items[0].path, expected);
    entries.items[0]
        .id
        .as_ref()
        .expect("a Windows file identity")
        .verify()?;
    let mut operations = Operations::open()?;
    operations.submit_operation(Operation::Rename {
        item: entries.items[0]
            .id
            .clone()
            .expect("a Windows file identity"),
        name: "renamed-火.txt".into(),
    })?;
    operations.undo()?;
    assert_eq!(fs::read(&expected)?, b"long path");
    let destination = fixture.path().canonicalize()?.join("destination");
    fs::create_dir(&destination)?;
    let copy = destination.join(expected.file_name().expect("a file name"));
    operations.submit_operation(Operation::Copy {
        item: ItemId::at(expected)?,
        into: ItemId::at(destination)?,
    })?;
    assert_eq!(fs::read(&copy)?, b"long path");
    operations.undo()?;
    assert!(!copy.exists());
    Ok(())
}

#[test]
fn directory_pages_report_size_time_and_attributes() -> Result<()> {
    const HIDDEN: u32 = 0x2;
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    fs::create_dir(root.join("folder"))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .attributes(HIDDEN)
        .open(root.join("hidden.txt"))?;
    std::io::Write::write_all(&mut file, b"12345")?;
    drop(file);
    let cancel = Cancellation::default();
    let mut view = Directory::open(&root)?;
    view.advance(128, &cancel)?;
    let items = view.list_dir(page(0), &cancel)?.items;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let folder = items
        .iter()
        .find(|item| item.directory)
        .expect("the folder");
    let hidden = items.iter().find(|item| !item.directory).expect("the file");
    assert_eq!(folder.size, None);
    assert_eq!(hidden.size, Some(5));
    assert!(
        hidden
            .attributes
            .is_some_and(|attributes| attributes & HIDDEN != 0)
    );
    for item in [folder, hidden] {
        let modified = item.modified.expect("a modification time");
        assert!((now - modified).abs() < 600, "unexpected time {modified}");
    }
    Ok(())
}

#[test]
fn legacy_settings_preserve_pins_and_store_folder_styles_by_identity() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let profile = fixture.path().join("profile");
    let folder = fixture.path().join("İzmir-火");
    fs::create_dir(&profile)?;
    fs::create_dir(&folder)?;
    let units: Vec<_> = folder.as_os_str().encode_wide().collect();
    fs::write(
        profile.join("explorer.json"),
        serde_json::to_vec(&serde_json::json!({"last_folder": units, "pins": [units]}))?,
    )?;

    let settings = Settings::load(&profile)?;
    assert_eq!(settings.last_folder.as_deref(), Some(folder.as_path()));
    assert_eq!(settings.pins.as_slice(), std::slice::from_ref(&folder));
    assert_eq!(settings.appearance.preset, Preset::LightMorphism);
    assert!(!settings.menu.is_empty());

    let key = FolderKey::from(&ItemId::at(folder.clone())?);
    let style = FolderStyle {
        alias: "Archive".into(),
        color: Some(Rgb(61, 120, 204)),
        icon: "A".into(),
        tags: vec!["work".into()],
    };
    Settings::update(&profile, |settings| {
        settings.folders.insert(key, style.clone());
    })?;
    let renamed = fixture.path().join("Renamed");
    fs::rename(&folder, &renamed)?;
    let renamed_key = FolderKey::from(&ItemId::at(renamed.clone())?);
    let settings = Settings::load(&profile)?;
    assert_eq!(settings.folders.get(&renamed_key), Some(&style));
    assert!(fs::read_dir(renamed)?.next().is_none());
    Ok(())
}

#[test]
fn concurrent_settings_writes_merge_independent_edits() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let profile = fixture.path();
    let start = Barrier::new(2);
    thread::scope(|threads| -> Result<()> {
        let sidebar = threads.spawn(|| {
            start.wait();
            Settings::update(profile, |settings| settings.sidebar = Some(false))
        });
        let appearance = threads.spawn(|| {
            start.wait();
            Settings::update(profile, |settings| {
                settings.appearance.preset = Preset::Claymorphism
            })
        });
        sidebar.join().expect("sidebar writer")?;
        appearance.join().expect("appearance writer")?;
        Ok(())
    })?;
    let settings = Settings::load(profile)?;
    assert_eq!(settings.sidebar, Some(false));
    assert_eq!(settings.appearance.preset, Preset::Claymorphism);
    Ok(())
}

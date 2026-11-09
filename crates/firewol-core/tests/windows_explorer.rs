#![cfg(windows)]

#[test]
fn sorting_by_date_mixes_folders_with_files() -> anyhow::Result<()> {
    use firewol_core::explorer::{Cancellation, Directory, Sort, SortColumn, page};
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime};
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let fixture = tempfile::tempdir()?;
    let stamp = |name: &str, age_days: u64| -> anyhow::Result<()> {
        // A handle to a folder needs the backup flag; files accept it too.
        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(fixture.path().join(name))?
            .set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400))?;
        Ok(())
    };
    std::fs::write(fixture.path().join("old.txt"), b"old")?;
    std::fs::write(fixture.path().join("new.txt"), b"new")?;
    std::fs::create_dir(fixture.path().join("middle"))?;
    stamp("old.txt", 30)?;
    stamp("middle", 10)?;
    stamp("new.txt", 1)?;
    let cancel = Cancellation::default();
    let mut directory = Directory::open(fixture.path())?;
    directory.advance(128, &cancel)?;
    let mut names = |descending| -> anyhow::Result<Vec<String>> {
        directory.sort(
            Sort {
                column: SortColumn::Modified,
                descending,
                group: false,
            },
            &cancel,
        )?;
        Ok(directory
            .list_dir(page(0), &cancel)?
            .items
            .iter()
            .map(|item| item.name().to_string_lossy().into_owned())
            .collect())
    };
    assert_eq!(names(false)?, ["old.txt", "middle", "new.txt"]);
    assert_eq!(names(true)?, ["new.txt", "middle", "old.txt"]);
    Ok(())
}

#[test]
fn a_type_filter_keeps_folders_and_matching_files() -> anyhow::Result<()> {
    use firewol_core::explorer::{Cancellation, Directory, ListFilter, Sort, page};
    let fixture = tempfile::tempdir()?;
    std::fs::write(fixture.path().join("Photo.PNG"), b"x")?;
    std::fs::write(fixture.path().join("note.txt"), b"x")?;
    std::fs::create_dir(fixture.path().join("folder"))?;
    let listed = |filter: ListFilter| -> anyhow::Result<Vec<String>> {
        let cancel = Cancellation::default();
        let mut directory = Directory::open(fixture.path())?;
        directory.only(filter);
        directory.advance(128, &cancel)?;
        directory.sort(Sort::default(), &cancel)?;
        Ok(directory
            .list_dir(page(0), &cancel)?
            .items
            .iter()
            .map(|item| item.name().to_string_lossy().into_owned())
            .collect())
    };
    assert_eq!(
        listed(ListFilter::from_spec("*.png;*.jpg"))?,
        ["folder", "Photo.PNG"]
    );
    assert_eq!(listed(ListFilter::FoldersOnly)?, ["folder"]);
    assert_eq!(
        listed(ListFilter::from_spec("*.*"))?.len(),
        3,
        "*.* hides nothing"
    );
    assert_eq!(
        listed(ListFilter::from_spec("report*"))?.len(),
        3,
        "an odd pattern hides nothing"
    );
    Ok(())
}

#[test]
fn grouping_puts_new_items_first_and_counts_each_section() -> anyhow::Result<()> {
    use firewol_core::explorer::{Cancellation, Directory, Sort, SortColumn, page};
    use std::time::{Duration, SystemTime};
    let fixture = tempfile::tempdir()?;
    for (name, age_days) in [
        ("a-ancient.txt", 800),
        ("b-today.txt", 0),
        ("c-ancient.txt", 900),
    ] {
        let path = fixture.path().join(name);
        std::fs::write(&path, b"x")?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400))?;
    }
    let cancel = Cancellation::default();
    let mut directory = Directory::open(fixture.path())?;
    directory.advance(128, &cancel)?;
    directory.sort(
        Sort {
            column: SortColumn::Name,
            descending: false,
            group: true,
        },
        &cancel,
    )?;
    let names: Vec<_> = directory
        .list_dir(page(0), &cancel)?
        .items
        .iter()
        .map(|item| item.name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["b-today.txt", "a-ancient.txt", "c-ancient.txt"]);
    let groups = directory.groups()?;
    assert_eq!(groups.len(), 2);
    assert_eq!(
        (groups[0].start, groups[0].count, groups[0].bucket),
        (0, 1, 0)
    );
    assert_eq!(
        (groups[1].start, groups[1].count, groups[1].bucket),
        (1, 2, 7)
    );
    directory.sort(Sort::default(), &cancel)?;
    assert!(directory.groups()?.is_empty());
    Ok(())
}

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
            group: false,
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
            .submit_operation(Operation::Recycle { items: vec![stale] })
            .is_err()
    );
    assert_eq!(fs::read(&source)?, b"different item");

    let original = ItemId::at(held.clone())?;
    operations.submit_operation(Operation::Recycle {
        items: vec![original.clone()],
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
        assert_eq!(reader.count(&regex, 100, &cancel)?, 1);
        assert_eq!(reader.count(&search("record", 0), 1000, &cancel)?, 140);
        assert_eq!(reader.count(&search("record", 0), 100, &cancel)?, 101);
        let mut reversed = search("record", 0);
        reversed.filters.min_bytes = Some(5);
        reversed.filters.max_bytes = Some(1);
        assert!(reader.search(&reversed, &cancel).is_err());
        let mut wildcard = search("record", 0);
        wildcard.filters.extension = Some("*.txt".into());
        assert!(reader.count(&wildcard, 10, &cancel).is_err());
        let mut invalid = search("(", 0);
        invalid.regex = true;
        assert!(reader.search(&invalid, &cancel).is_err());

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

#[test]
fn stale_search_indexes_are_removed_but_live_ones_stay() -> anyhow::Result<()> {
    use firewol_core::explorer::{SearchIndex, remove_stale_indexes};
    let data = tempfile::tempdir()?;
    let live = SearchIndex::create(data.path())?;
    let _reader = live.reader()?;
    for name in ["search-4294967280-abc", "search-legacy", "unrelated"] {
        std::fs::create_dir(data.path().join(name))?;
    }
    remove_stale_indexes(data.path());
    let mut left: Vec<_> = std::fs::read_dir(data.path())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    left.sort();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left[0].starts_with(&format!("search-{}-", std::process::id())));
    assert_eq!(left[1], "unrelated");
    Ok(())
}

#[test]
fn multi_item_recycle_undoes_as_one_batch() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path();
    let names = ["a-火.txt", "b.txt", "c"];
    fs::write(root.join(names[0]), b"first")?;
    fs::write(root.join(names[1]), b"second")?;
    fs::create_dir(root.join(names[2]))?;
    let items = names
        .iter()
        .map(|name| ItemId::at(root.join(name)))
        .collect::<Result<Vec<_>>>()?;
    let mut operations = Operations::open()?;
    assert!(
        operations
            .submit_operation(Operation::Recycle {
                items: vec![items[0].clone(), items[0].clone()],
            })
            .is_err(),
        "the same path twice is refused"
    );
    let done = operations.submit_operation(Operation::Recycle {
        items: items.clone(),
    })?;
    assert_eq!(done.message, "Moved 3 items to Recycle Bin");
    assert!(names.iter().all(|name| !root.join(name).exists()));

    // A conflict at the first restored (newest) item keeps the whole batch for a retry.
    fs::create_dir(root.join(names[2]))?;
    assert!(operations.undo().is_err());
    fs::remove_dir(root.join(names[2]))?;
    // A conflict later restores the newer items and keeps the rest.
    fs::write(root.join(names[0]), b"conflict")?;
    let partial = operations.undo()?;
    assert!(
        partial.message.starts_with("Restored 2/3 items"),
        "{}",
        partial.message
    );
    assert!(root.join(names[2]).is_dir());
    assert_eq!(fs::read(root.join(names[1]))?, b"second");
    assert_eq!(fs::read(root.join(names[0]))?, b"conflict");
    fs::remove_file(root.join(names[0]))?;
    let rest = operations.undo()?;
    assert_eq!(rest.message, "Restored the exact item from Recycle Bin");
    assert_eq!(fs::read(root.join(names[0]))?, b"first");
    items.iter().try_for_each(|item| item.verify().map(drop))?;
    assert!(operations.undo().is_err(), "the batch was one undo record");
    Ok(())
}

#[test]
fn recycle_and_restore_beyond_max_path_with_unicode_names() -> Result<()> {
    let fixture = tempfile::tempdir()?;
    let mut deep = fixture.path().canonicalize()?.join("Ünïcode 火 😀");
    for n in 1..=8 {
        deep = deep.join(format!("uzun-klasör-adı-{n:02}-{}", "x".repeat(20)));
    }
    fs::create_dir_all(&deep)?;
    let file = deep.join("ünïcöde-火.txt");
    fs::write(&file, b"long path")?;
    assert!(file.as_os_str().len() > 260);
    let item = ItemId::at(file.clone())?;
    let mut operations = Operations::open()?;
    operations.submit_operation(Operation::Recycle {
        items: vec![item.clone()],
    })?;
    assert!(!file.exists());
    operations.undo()?;
    item.verify()?;
    assert_eq!(fs::read(&file)?, b"long path");
    Ok(())
}

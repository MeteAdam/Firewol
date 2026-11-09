//! Windows 11 commands that exist only as packaged `IExplorerCommand` handlers, such as VS Code's
//! "Open with Code". `IContextMenu` never lists them, so they are read from the packages' own
//! manifests. Everything here is best effort: a handler that fails simply adds no row.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
use windows::Win32::Storage::Packaging::Appx::GetPackagePathByFullName;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ECF_HASSUBCOMMANDS, ECF_ISSEPARATOR, IExplorerCommand, IShellItemArray,
    SHCreateShellItemArrayFromIDLists,
};
use windows_core::{GUID, HSTRING, PWSTR, w};

use super::icons;
use super::menu_icons::{self, ResourceIcon};
use super::shell_menu::{ShellItem, Target, clean_label, parse};

/// `EXPCMDSTATE` values; 0 is enabled.
const ECS_DISABLED: u32 = 1;
const ECS_HIDDEN: u32 = 2;
/// Ids above the range `IContextMenu` commands use.
const FIRST_ID: u32 = 0x8000;
const MAX_DEPTH: u32 = 2;

#[derive(Debug, PartialEq, Eq)]
struct Verb {
    clsid: GUID,
    /// `*`, `.ext`, `directory`, `drive` or `directory\background`, in lower case.
    types: Vec<String>,
}

fn parse_manifest(text: &str) -> Vec<Verb> {
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static VERB: OnceLock<Regex> = OnceLock::new();
    let item = ITEM.get_or_init(|| {
        Regex::new(r#"(?s)<\w+:ItemType\b[^>]*\bType="([^"]+)"[^>]*>(.*?)</\w+:ItemType>"#)
            .expect("item type pattern")
    });
    let verb = VERB.get_or_init(|| {
        Regex::new(r#"<\w+:Verb\b[^>]*\bClsid="\{?([0-9A-Fa-f-]{36})\}?""#).expect("verb pattern")
    });
    let mut verbs: Vec<Verb> = Vec::new();
    for block in item.captures_iter(text) {
        let kind = block[1].to_lowercase();
        for found in verb.captures_iter(&block[2]) {
            let Ok(clsid) = GUID::try_from(&found[1]) else {
                continue;
            };
            match verbs.iter_mut().find(|known| known.clsid == clsid) {
                Some(known) => known.types.push(kind.clone()),
                None => verbs.push(Verb {
                    clsid,
                    types: vec![kind.clone()],
                }),
            }
        }
    }
    verbs
}

fn package_path(name: &HSTRING) -> Option<PathBuf> {
    let mut length = 0;
    if unsafe { GetPackagePathByFullName(name, &mut length, None) } != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    let result =
        unsafe { GetPackagePathByFullName(name, &mut length, Some(PWSTR(buffer.as_mut_ptr()))) };
    (result == ERROR_SUCCESS).then(|| {
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        PathBuf::from(String::from_utf16_lossy(&buffer[..end]))
    })
}

fn packages() -> Vec<HSTRING> {
    let mut key = HKEY::default();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Classes\PackagedCom\Package"),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if opened != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        let found = unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if found != ERROR_SUCCESS {
            break;
        }
        names.push(HSTRING::from_wide(&name[..length as usize]));
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    names
}

/// Reading manifests is the slow part, and packages rarely change while Firewol runs.
fn verbs() -> &'static [Verb] {
    static VERBS: OnceLock<Vec<Verb>> = OnceLock::new();
    VERBS.get_or_init(|| {
        packages()
            .iter()
            .filter_map(package_path)
            .filter_map(|path| std::fs::read_to_string(path.join("AppxManifest.xml")).ok())
            .flat_map(|manifest| parse_manifest(&manifest))
            .collect()
    })
}

fn applies(types: &[String], target: &Target) -> bool {
    let matches_path = |path: &Path| {
        if path.is_dir() {
            let root = path.parent().is_none();
            return types
                .iter()
                .any(|kind| kind == "directory" || (root && kind == "drive"));
        }
        let extension = path
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()));
        types
            .iter()
            .any(|kind| kind == "*" || Some(kind) == extension.as_ref())
    };
    match target {
        Target::Background(_) => types.iter().any(|kind| kind == r"directory\background"),
        Target::Items(paths) => paths.iter().all(|path| matches_path(path)),
    }
}

fn take_text(text: PWSTR) -> Option<String> {
    // A handler can report success and still return no string.
    if text.is_null() {
        return None;
    }
    let value = unsafe { text.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(text.0 as _)) };
    value
}

/// `path,index` from a handler; only programs and icon files can be served by key.
fn icon_key(location: &str) -> Option<String> {
    let path = match location.rsplit_once(',') {
        Some((path, index)) if index.trim() == "0" => path,
        Some(_) => return None,
        None => location,
    };
    let key = icons::key(Path::new(&path.trim_matches('"').replace('/', "\\")));
    key.starts_with("path:").then_some(key)
}

#[derive(Default)]
pub(super) struct Modern {
    commands: HashMap<u32, IExplorerCommand>,
    items: Option<IShellItemArray>,
}

impl Modern {
    /// Reads the commands that apply to `target`, recording their ids in `allowed`.
    pub(super) fn load(
        target: &Target,
        allowed: &mut HashSet<u32>,
        mut store_icon: impl FnMut(ResourceIcon) -> String,
    ) -> (Self, Vec<ShellItem>) {
        let mut modern = Self::default();
        let verbs: Vec<&Verb> = verbs()
            .iter()
            .filter(|verb| applies(&verb.types, target))
            .collect();
        if verbs.is_empty() {
            return (modern, Vec::new());
        }
        let paths: Vec<&Path> = match target {
            Target::Items(paths) => paths.iter().map(PathBuf::as_path).collect(),
            Target::Background(folder) => vec![folder.as_path()],
        };
        let Ok(items) = paths
            .iter()
            .map(|path| parse(path))
            .collect::<Result<Vec<_>>>()
        else {
            return (modern, Vec::new());
        };
        let pointers: Vec<*const ITEMIDLIST> =
            items.iter().map(|item| item.0.cast_const()).collect();
        let Ok(array) = (unsafe { SHCreateShellItemArrayFromIDLists(&pointers) }) else {
            return (modern, Vec::new());
        };
        let mut next = FIRST_ID;
        let mut rows = Vec::new();
        for verb in verbs {
            let Ok(command) = (unsafe {
                CoCreateInstance::<_, IExplorerCommand>(
                    &verb.clsid,
                    None,
                    CLSCTX_LOCAL_SERVER | CLSCTX_INPROC_SERVER,
                )
            }) else {
                continue;
            };
            if let Some(mut row) =
                modern.row(command, &array, allowed, &mut next, 0, &mut store_icon)
            {
                // Packaged commands have no verb; the class id names them in any language.
                row.verb = format!("{:?}", verb.clsid).to_lowercase();
                rows.push(row);
            }
        }
        modern.items = Some(array);
        (modern, rows)
    }

    fn row(
        &mut self,
        command: IExplorerCommand,
        array: &IShellItemArray,
        allowed: &mut HashSet<u32>,
        next: &mut u32,
        depth: u32,
        store_icon: &mut impl FnMut(ResourceIcon) -> String,
    ) -> Option<ShellItem> {
        let flags = unsafe { command.GetFlags() }.unwrap_or(0);
        if flags & ECF_ISSEPARATOR.0 as u32 != 0 {
            return Some(ShellItem {
                separator: true,
                ..Default::default()
            });
        }
        let state = unsafe { command.GetState(array, true) }.unwrap_or(0);
        if state & ECS_HIDDEN != 0 {
            return None;
        }
        let label = clean_label(&take_text(unsafe { command.GetTitle(array) }.ok()?)?);
        if label.is_empty() {
            return None;
        }
        let location = unsafe { command.GetIcon(array) }.ok().and_then(take_text);
        let mut children = Vec::new();
        if flags & ECF_HASSUBCOMMANDS.0 as u32 != 0 && depth < MAX_DEPTH {
            let list = unsafe { command.EnumSubCommands() }.ok()?;
            loop {
                let mut slot = [None];
                let mut fetched = 0;
                let result = unsafe { list.Next(&mut slot, Some(&mut fetched)) };
                if result.is_err() || fetched == 0 {
                    break;
                }
                if let Some(child) = slot[0].take()
                    && let Some(row) = self.row(child, array, allowed, next, depth + 1, store_icon)
                {
                    children.push(row);
                }
            }
            if children.is_empty() {
                return None;
            }
        }
        let id = *next;
        *next += 1;
        if children.is_empty() {
            allowed.insert(id);
            self.commands.insert(id, command);
        }
        let icon = location.as_deref().and_then(|location| {
            menu_icons::modern(location)
                .map(store_icon)
                .or_else(|| icon_key(location))
        });
        Some(ShellItem {
            id,
            label,
            icon,
            children,
            disabled: state & ECS_DISABLED != 0,
            verb: String::new(),
            ..Default::default()
        })
    }

    pub(super) fn invoke(&self, id: u32) -> Option<Result<()>> {
        let command = self.commands.get(&id)?;
        let items = self.items.as_ref()?;
        Some(unsafe { command.Invoke(items, None) }.map_err(anyhow::Error::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_string_from_a_handler_is_no_text() {
        assert_eq!(take_text(PWSTR::null()), None);
    }

    const MANIFEST: &str = r#"<Extensions>
        <desktop4:FileExplorerContextMenus>
          <desktop5:ItemType Type="Directory">
            <desktop5:Verb Id="OpenWithCode" Clsid="1C6DF0C0-192A-4451-BE36-6A59A86A692E" />
          </desktop5:ItemType>
          <desktop5:ItemType Type="*">
            <desktop5:Verb Id="OpenWithCode" Clsid="1C6DF0C0-192A-4451-BE36-6A59A86A692E" />
          </desktop5:ItemType>
        </desktop4:FileExplorerContextMenus>
      </Extensions>"#;

    #[test]
    fn a_manifest_gives_one_verb_with_all_its_item_types() {
        let verbs = parse_manifest(MANIFEST);
        assert_eq!(verbs.len(), 1);
        assert_eq!(verbs[0].types, ["directory", "*"]);
    }

    #[test]
    fn item_types_follow_what_was_clicked() {
        let fixture = tempfile::tempdir().expect("fixture");
        let file = fixture.path().join("note.txt");
        std::fs::write(&file, "x").expect("file");
        let types = |kinds: &[&str]| {
            kinds
                .iter()
                .map(|kind| (*kind).to_owned())
                .collect::<Vec<_>>()
        };
        let items = |path: &Path| Target::Items(vec![path.to_owned()]);
        assert!(applies(&types(&["*"]), &items(&file)));
        assert!(applies(&types(&[".txt"]), &items(&file)));
        assert!(!applies(&types(&[".log"]), &items(&file)));
        assert!(!applies(&types(&["*"]), &items(fixture.path())));
        assert!(applies(&types(&["directory"]), &items(fixture.path())));
        let background = Target::Background(fixture.path().to_owned());
        assert!(applies(&types(&[r"directory\background"]), &background));
        assert!(!applies(&types(&["directory"]), &background));
    }

    #[test]
    fn only_programs_and_icon_files_become_icon_keys() {
        assert_eq!(
            icon_key(r"C:\Tools\code.exe,0").as_deref(),
            Some(r"path:C:\Tools\code.exe")
        );
        assert_eq!(icon_key(r"C:\Windows\System32\shell32.dll,-16"), None);
    }
}

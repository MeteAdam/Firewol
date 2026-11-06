#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chrome;
mod explorer;
mod m0;

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, bail};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
use windows::core::{PCWSTR, w};

fn run(args: &[OsString]) -> Result<()> {
    if let [command, drive, serial, pipe, parent] = args
        && command == "--ntfs-helper"
    {
        return firewol_core::explorer::volume::run_helper(
            drive.to_string_lossy().parse()?,
            serial.to_string_lossy().parse()?,
            &pipe.to_string_lossy(),
            parent.to_string_lossy().parse()?,
        );
    }
    if std::env::var_os("FIREWOL_M0_REPORT").is_some() {
        return m0::run();
    }
    match args {
        [] => explorer::run(None),
        [command, path] if command == "--open" => explorer::run(Some(PathBuf::from(path))),
        _ => bail!("Usage: firewol-win32.exe [--open PATH]"),
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Err(error) = run(&args) {
        eprintln!("{error:#}");
        if !args.first().is_some_and(|arg| arg == "--ntfs-helper")
            && std::env::var_os("FIREWOL_M0_REPORT").is_none()
        {
            let message: Vec<_> = format!("{error:#}").encode_utf16().chain([0]).collect();
            unsafe { MessageBoxW(None, PCWSTR(message.as_ptr()), w!("Firewol"), MB_ICONERROR) };
        }
        std::process::exit(1);
    }
}

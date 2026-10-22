//! `pirosman-ofxhost scan <binary.ofx>` prints a JSON scan report.
//! `pirosman-ofxhost render <spec.json>` streams frames from stdin to stdout.
//! `pirosman-ofxhost version` prints the protocol version.
//!
//! Plugin chatter on stdout would corrupt the frame stream, so the process's
//! standard output handle is pointed at stderr before any plugin loads; data
//! goes out through the original handle only.

use std::io::{BufReader, BufWriter, Write};
use std::process::ExitCode;

use pirosman_ofxhost::protocol::{RenderSpec, ScanReport, PROTOCOL};
use pirosman_ofxhost::{host, stream};

#[cfg(windows)]
fn data_out() -> std::fs::File {
    use std::ffi::c_void;
    use std::os::windows::io::FromRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut c_void;
        fn SetStdHandle(n: u32, h: *mut c_void) -> i32;
        fn SetErrorMode(mode: u32) -> u32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    unsafe {
        // No crash or missing-DLL dialogs: a failing plugin must just end the process.
        SetErrorMode(0x0001 | 0x0002 | 0x8000);
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
        std::fs::File::from_raw_handle(out)
    }
}

#[cfg(not(windows))]
fn data_out() -> std::fs::File {
    use std::os::fd::FromRawFd;
    unsafe {
        let out = libc_dup(1);
        libc_dup2(2, 1);
        std::fs::File::from_raw_fd(out)
    }
}

#[cfg(not(windows))]
extern "C" {
    #[link_name = "dup"]
    fn libc_dup(fd: i32) -> i32;
    #[link_name = "dup2"]
    fn libc_dup2(a: i32, b: i32) -> i32;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut out = BufWriter::new(data_out());
    match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("version"), _) => {
            let _ = writeln!(out, "{PROTOCOL}");
            ExitCode::SUCCESS
        }
        (Some("scan"), Some(binary)) => {
            let report = match host::scan_binary(binary) {
                Ok(plugins) => ScanReport { protocol: PROTOCOL, binary: binary.clone(), plugins, error: None },
                Err(e) => ScanReport { protocol: PROTOCOL, binary: binary.clone(), plugins: vec![], error: Some(e) },
            };
            let _ = serde_json::to_writer(&mut out, &report);
            let _ = out.flush();
            ExitCode::SUCCESS
        }
        (Some("render"), Some(spec)) => {
            let spec: RenderSpec = match std::fs::read_to_string(spec).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("pirosman-ofxhost: bad render spec: {e}");
                    return ExitCode::from(2);
                }
            };
            let mut input = BufReader::with_capacity(1 << 22, std::io::stdin().lock());
            match stream::render_stream(&spec, &mut input, &mut out, |_| {}) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("pirosman-ofxhost: {e}");
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            eprintln!("usage: pirosman-ofxhost scan <binary.ofx> | render <spec.json> | version");
            ExitCode::from(2)
        }
    }
}

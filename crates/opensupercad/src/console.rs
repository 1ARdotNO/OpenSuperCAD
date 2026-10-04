//! Windows: the app is built as a GUI program (no console window when it's
//! started from the Start menu), so command-line runs attach to the console
//! of the terminal they were started from to print their output.
//!
//! The alternative, a separate GUI launcher exe next to a console exe, is
//! recorded in #44.

/// Attach to the parent process's console, if there is one. A no-op when
/// there is none (Explorer, shortcuts) and on other platforms.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn attach_parent() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: AttachConsole takes a plain integer and touches no memory we
    // own. Failure (no parent console, or one already attached) is harmless
    // and deliberately ignored.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
pub fn attach_parent() {}

use eframe::egui;
use std::fmt::Write;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::{Command, Stdio};
use std::sync::{Once, OnceLock};

pub(super) fn show(ui: &mut egui::Ui) {
    static START: Once = Once::new();
    static REPORT: OnceLock<String> = OnceLock::new();

    // OS queries can take time. Keep them off the GUI thread.
    START.call_once(|| {
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let _ = REPORT.set(collect());
            ctx.request_repaint();
        });
    });

    ui.label(concat!("FolderHistory ", env!("CARGO_PKG_VERSION")));
    if let Some(report) = REPORT.get() {
        if ui.button("Copy diagnostic info").clicked() {
            ui.ctx().copy_text(report.clone());
        }
    } else {
        ui.label("Collecting diagnostic info…");
    }
}

fn collect() -> String {
    let mut report = format!(
        "FolderHistory: {}\nOperating system: {}\nApp architecture: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    platform_details(&mut report);
    report
}

fn field(report: &mut String, label: &str, value: &str) {
    let value: String = value.trim().chars().filter(|c| !c.is_control()).take(160).collect();
    if !value.is_empty() {
        let _ = write!(report, "\n{label}: {value}");
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.args(args).stdin(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(target_os = "linux")]
fn platform_details(report: &mut String) {
    let os_release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"));
    if let Ok(os_release) = os_release {
        if let Some(name) = os_release.lines().find_map(|line| line.strip_prefix("PRETTY_NAME=")) {
            field(report, "Distribution", name.trim_matches(['"', '\'']));
        }
    }
    if let Ok(kernel) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        field(report, "Kernel", &kernel);
    }
    if let Ok(session) = std::env::var("XDG_SESSION_TYPE") {
        field(report, "Session type", &session);
    }
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        field(report, "Desktop", &desktop);
    }
}

#[cfg(target_os = "macos")]
fn platform_details(report: &mut String) {
    for (label, arg) in [("macOS version", "-productVersion"), ("Build", "-buildVersion")] {
        if let Some(value) = command_output("/usr/bin/sw_vers", &[arg]) {
            field(report, label, &value);
        }
    }
    let translated = command_output("/usr/sbin/sysctl", &["-in", "sysctl.proc_translated"]);
    let arm = command_output("/usr/sbin/sysctl", &["-in", "hw.optional.arm64"]);
    if translated.as_deref() == Some("1") || arm.as_deref() == Some("1") {
        field(report, "Hardware", "Apple Silicon");
        field(report, "Rosetta", if translated.as_deref() == Some("1") { "Yes" } else { "No" });
    } else if std::env::consts::ARCH == "x86_64" && arm.as_deref() == Some("0") {
        field(report, "Hardware", "Intel");
    }
}

#[cfg(target_os = "windows")]
fn platform_details(report: &mut String) {
    // cmd's built-in ver reports the Windows version and build together.
    let cmd = std::env::var_os("SystemRoot").map(|root| std::path::PathBuf::from(root).join("System32/cmd.exe"));
    if let Some(version) = cmd.as_deref().and_then(std::path::Path::to_str).and_then(|cmd| command_output(cmd, &["/D", "/C", "ver"])) {
        field(report, "Windows version and build", &version);
    }
    // WOW64 supplies the native architecture when it differs from the process.
    if let Ok(native) = std::env::var("PROCESSOR_ARCHITEW6432") {
        field(report, "Native architecture", &native);
        field(report, "Running under emulation", "Yes");
    } else if let Ok(architecture) = std::env::var("PROCESSOR_ARCHITECTURE") {
        field(report, "Reported architecture", &architecture);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_details(_report: &mut String) {}

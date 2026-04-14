fn main() {
    // On Windows dev builds: copy QEMU DLLs from binaries/lib/ to {target_dir}/lib/
    // so the DLL PATH injection in qemu.rs can find them at runtime.
    #[cfg(target_os = "windows")]
    copy_qemu_dlls_to_target();

    // Copy pre-built nilbox-mcp-bridge to binaries/ for Tauri externalBin bundling
    ensure_mcp_bridge_binary();

    // Copy pre-built nilbox-blocklist-build to binaries/ for Tauri externalBin bundling
    ensure_blocklist_build_binary();

    // Copy pre-built nilbox-vmm (Swift) to binaries/ for Tauri externalBin bundling (macOS only)
    // On non-macOS platforms, create a stub placeholder so Tauri's externalBin check passes.
    #[cfg(target_os = "macos")]
    ensure_vmm_binary();
    #[cfg(not(target_os = "macos"))]
    ensure_vmm_binary_stub();

    tauri_build::build()
}

/// Copy `nilbox-mcp-bridge` from the workspace target directory to `binaries/`
/// if the platform-specific binary is missing.
///
/// Tauri requires `binaries/nilbox-mcp-bridge-{target_triple}` to exist at build time.
///
/// NOTE: Cannot invoke `cargo build` here (same workspace = cargo lock deadlock).
/// The binary must already exist in `target/{profile}/`. Build it first with:
///   cargo build -p nilbox-mcp-bridge
fn ensure_mcp_bridge_binary() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_triple = std::env::var("TARGET").unwrap();
    let profile = std::env::var("PROFILE").unwrap();

    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let binary_name = format!("nilbox-mcp-bridge-{target_triple}{ext}");
    let dest = Path::new(&manifest_dir).join("binaries").join(&binary_name);

    let workspace_root = Path::new(&manifest_dir)
        .ancestors()
        .find(|p| p.join("Cargo.lock").exists())
        .expect("cannot find workspace root");

    let src = workspace_root
        .join("target")
        .join(&profile)
        .join(format!("nilbox-mcp-bridge{ext}"));

    // Re-run when the source binary changes (rebuilt by cargo)
    println!("cargo:rerun-if-changed={}", src.display());

    // Already copied and up-to-date?
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        // Check if source is newer than dest
        let src_modified = src.metadata().and_then(|m| m.modified()).ok();
        let dest_modified = dest.metadata().and_then(|m| m.modified()).ok();
        if let (Some(s), Some(d)) = (src_modified, dest_modified) {
            if s <= d {
                return;
            }
        } else {
            return;
        }
    }

    // Remove zero-byte placeholder or stale binary
    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }

    if !src.exists() {
        panic!(
            "nilbox-mcp-bridge binary not found at {}.\n\
             Build it first: cargo build -p nilbox-mcp-bridge",
            src.display()
        );
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    std::fs::copy(&src, &dest).unwrap_or_else(|e| {
        panic!("Failed to copy {} → {}: {e}", src.display(), dest.display())
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
    }

    eprintln!("Copied nilbox-mcp-bridge → {}", dest.display());
}

/// Copy `nilbox-blocklist-build` from the workspace target directory to `binaries/`
/// if the platform-specific binary is missing.
///
/// Tauri requires `binaries/nilbox-blocklist-build-{target_triple}` to exist at build time.
///
/// NOTE: Cannot invoke `cargo build` here (same workspace = cargo lock deadlock).
/// The binary must already exist in `target/{profile}/`. Build it first with:
///   cargo build -p nilbox-blocklist --features cli
fn ensure_blocklist_build_binary() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_triple = std::env::var("TARGET").unwrap();
    let profile = std::env::var("PROFILE").unwrap();

    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let binary_name = format!("nilbox-blocklist-build-{target_triple}{ext}");
    let dest = Path::new(&manifest_dir).join("binaries").join(&binary_name);

    let workspace_root = Path::new(&manifest_dir)
        .ancestors()
        .find(|p| p.join("Cargo.lock").exists())
        .expect("cannot find workspace root");

    let src = workspace_root
        .join("target")
        .join(&profile)
        .join(format!("nilbox-blocklist-build{ext}"));

    println!("cargo:rerun-if-changed={}", src.display());

    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        let src_modified = src.metadata().and_then(|m| m.modified()).ok();
        let dest_modified = dest.metadata().and_then(|m| m.modified()).ok();
        if let (Some(s), Some(d)) = (src_modified, dest_modified) {
            if s <= d {
                return;
            }
        } else {
            return;
        }
    }

    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }

    if !src.exists() {
        panic!(
            "nilbox-blocklist-build binary not found at {}.\n\
             Build it first: cargo build -p nilbox-blocklist --features cli",
            src.display()
        );
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    std::fs::copy(&src, &dest).unwrap_or_else(|e| {
        panic!("Failed to copy {} → {}: {e}", src.display(), dest.display())
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
    }

    eprintln!("Copied nilbox-blocklist-build → {}", dest.display());
}

/// Copy `nilbox-vmm` Swift binary to `binaries/` for Tauri externalBin bundling (macOS only).
///
/// Searches for the pre-built binary in the nilbox-vmm Swift package build directory:
///   1. Architecture-specific: nilbox-vmm/.build/{arch}-apple-macosx/release/nilbox-vmm
///   2. Generic release: nilbox-vmm/.build/release/nilbox-vmm
///
/// The binary must already be built with: cd nilbox-vmm && make release
#[cfg(target_os = "macos")]
fn ensure_vmm_binary() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_triple = std::env::var("TARGET").unwrap();

    let binary_name = format!("nilbox-vmm-{target_triple}");
    let dest = Path::new(&manifest_dir).join("binaries").join(&binary_name);

    // nilbox-vmm Swift package is at ../../nilbox-vmm relative to src-tauri
    let vmm_build_dir = Path::new(&manifest_dir).join("../../nilbox-vmm/.build");

    // Map Rust arch to Swift arch
    let swift_arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };

    // Try arch-specific first, then generic release
    let candidates = [
        vmm_build_dir.join(format!("{swift_arch}-apple-macosx/release/nilbox-vmm")),
        vmm_build_dir.join("release/nilbox-vmm"),
    ];

    // Re-run when any candidate changes
    for c in &candidates {
        println!("cargo:rerun-if-changed={}", c.display());
    }

    // Already copied and up-to-date?
    if dest.exists() && dest.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        // Check if any source is newer than dest
        let dest_modified = dest.metadata().and_then(|m| m.modified()).ok();
        let any_newer = candidates.iter().any(|c| {
            if let (Some(s), Some(d)) = (
                c.metadata().and_then(|m| m.modified()).ok(),
                dest_modified,
            ) {
                s > d
            } else {
                false
            }
        });
        if !any_newer {
            return;
        }
    }

    // Remove stale binary
    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }

    let src = candidates.iter().find(|c| c.exists());
    let src = match src {
        Some(p) => p,
        None => {
            panic!(
                "nilbox-vmm binary not found. Build it first:\n  cd nilbox-vmm && make release\n\
                 Searched: {:?}",
                candidates
            );
        }
    };

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    std::fs::copy(src, &dest).unwrap_or_else(|e| {
        panic!("Failed to copy {} → {}: {e}", src.display(), dest.display())
    });

    // Ensure executable permission
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
    }

    // Ad-hoc codesign with virtualization entitlement (required for Virtualization.framework)
    let vmm_entitlements = Path::new(&manifest_dir).join("../../nilbox-vmm/nilbox-vmm.entitlements");
    if vmm_entitlements.exists() {
        let status = std::process::Command::new("codesign")
            .args([
                "--sign", "-",
                "--entitlements", &vmm_entitlements.to_string_lossy(),
                "--force",
                &dest.to_string_lossy(),
            ])
            .status();
        match status {
            Ok(s) if s.success() => eprintln!("Signed nilbox-vmm with virtualization entitlement"),
            Ok(s) => eprintln!("Warning: codesign exited with {s}"),
            Err(e) => eprintln!("Warning: codesign failed: {e}"),
        }
    }

    eprintln!("Copied nilbox-vmm → {}", dest.display());
}

/// Create a stub placeholder binary for `nilbox-vmm` on non-macOS platforms.
///
/// `nilbox-vmm` uses Apple Virtualization.framework and only exists on macOS.
/// Tauri's `externalBin` check requires the file to exist at build time on all platforms,
/// so we create an empty (zero-byte) placeholder to satisfy the check.
/// The stub is never executed on Linux/Windows.
#[cfg(not(target_os = "macos"))]
fn ensure_vmm_binary_stub() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_triple = std::env::var("TARGET").unwrap();

    let binary_name = format!("nilbox-vmm-{target_triple}");
    let dest = Path::new(&manifest_dir).join("binaries").join(&binary_name);

    if dest.exists() {
        return;
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    std::fs::write(&dest, b"").unwrap_or_else(|e| {
        panic!("Failed to create nilbox-vmm stub at {}: {e}", dest.display())
    });

    eprintln!("Created nilbox-vmm stub (macOS-only binary) → {}", dest.display());
}

#[cfg(target_os = "windows")]
fn copy_qemu_dlls_to_target() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let dll_src = Path::new(&manifest_dir).join("binaries").join("windows").join("lib");
    if !dll_src.exists() {
        return;
    }

    // OUT_DIR: target/{profile}/build/nilbox-{hash}/out  →  go up 3 levels → target/{profile}
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let target_dir = Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("unexpected OUT_DIR depth");
    let dll_dest = target_dir.join("lib");

    std::fs::create_dir_all(&dll_dest).unwrap();

    for entry in std::fs::read_dir(&dll_src).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("dll") {
            let dest = dll_dest.join(entry.file_name());
            std::fs::copy(&path, &dest).unwrap();
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    // Copy QEMU BIOS/ROM files to target dir (alongside the exe)
    let bios_src = Path::new(&manifest_dir).join("binaries").join("windows");
    if let Ok(entries) = std::fs::read_dir(&bios_src) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "bin" || ext == "rom" {
                let dest = target_dir.join(entry.file_name());
                let _ = std::fs::copy(&path, &dest);
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}


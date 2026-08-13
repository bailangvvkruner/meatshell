fn main() {
    copy_angle_runtime();

    // Bundle the gettext `.po` translations under `lang/` so the UI's `@tr(...)`
    // strings can switch language at runtime via slint::select_bundled_translation.
    // Source language is Chinese (the msgids); `lang/<lc>/LC_MESSAGES/meatshell.po`
    // provides other locales.  No per-component context, so msgids are the raw
    // Chinese strings.
    println!("cargo:rerun-if-changed=lang");
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent".into())
            .with_bundled_translations("lang")
            .with_default_translation_context(slint_build::DefaultTranslationContext::None),
    )
    .expect("Slint build failed");

    // Build scripts are compiled for the host, so cfg!(windows) is false when
    // cargo-xwin targets Windows from Linux/macOS. Check Cargo's target metadata
    // at runtime instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/meatshell.ico");
        println!("cargo:rerun-if-changed=assets/meatshell.exe.manifest");
        println!("cargo:rerun-if-env-changed=RC_PATH");
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
        );
        let icon = manifest_dir.join("assets/meatshell.ico");
        let manifest = manifest_dir.join("assets/meatshell.exe.manifest");

        // cargo-xwin supplies the MSVC libraries but winresource still needs a
        // resource compiler. LLVM's compatible tool is a reliable fallback.
        if std::env::var_os("RC_PATH").is_none() {
            let llvm_rc_name = if cfg!(windows) {
                "llvm-rc.exe"
            } else {
                "llvm-rc"
            };
            if let Some(llvm_rc) = executable_on_path(llvm_rc_name) {
                std::env::set_var("RC_PATH", llvm_rc);
            }
        }
        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon.to_string_lossy().as_ref());
        // Embed an application manifest declaring Per-Monitor DPI Awareness V2.
        // Without it the DPI-awareness level depends on winit's runtime
        // SetProcessDpiAwarenessContext call, which races: if anything touches a
        // DPI API first the call silently fails and the window jumps in size /
        // cursor offset when dragged across monitors with different scaling (#194).
        // The manifest is authoritative and applied before any code runs.
        res.set_manifest_file(manifest.to_string_lossy().as_ref());
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows resources: {e}");
        }
    }
}

fn copy_angle_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64")
    {
        return;
    }

    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    let runtime_dir = manifest_dir.join("vendor/angle/windows-x86_64");
    let out_dir =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let binary_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR uses Cargo's target/<profile>/build/<package>/out layout");

    for name in ["libEGL.dll", "libGLESv2.dll"] {
        let source = runtime_dir.join(name);
        let destination = binary_dir.join(name);
        println!("cargo:rerun-if-changed={}", source.display());

        let source_bytes = std::fs::read(&source)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", source.display()));
        if std::fs::read(&destination).ok().as_deref() == Some(source_bytes.as_slice()) {
            continue;
        }
        std::fs::write(&destination, source_bytes)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", destination.display()));
    }
}

fn executable_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

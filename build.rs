fn main() {
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

    // Embed the application icon into the Windows executable so it shows up in
    // Explorer, the taskbar and shortcuts. No-op on non-Windows targets.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/meatshell.ico");
        println!("cargo:rerun-if-changed=assets/meatshell.exe.manifest");
        println!("cargo:rerun-if-env-changed=RC_PATH");
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
        );
        let icon = manifest_dir.join("assets/meatshell.ico");
        let manifest = manifest_dir.join("assets/meatshell.exe.manifest");

        // winresource looks for a Windows SDK rc.exe on Windows hosts even when
        // cargo-xwin is providing the MSVC libraries. Fall back to LLVM's
        // compatible resource compiler when it is already available on PATH.
        if std::env::var_os("RC_PATH").is_none() {
            if let Some(llvm_rc) = executable_on_path("llvm-rc.exe") {
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

#[cfg(windows)]
fn executable_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

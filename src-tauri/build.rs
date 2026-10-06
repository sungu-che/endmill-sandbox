fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let _ = tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("tauri.conf.json")),
    ));
}
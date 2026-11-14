fn main() {
    openplugin_core::build::embed_files("presets", "preset", "presets.rs").expect("embed presets");
}

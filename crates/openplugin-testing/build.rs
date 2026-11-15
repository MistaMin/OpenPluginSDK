// The examples test links two example plug-ins into one binary, and each
// exports `clap_entry`. Apple's linker keeps the first definition silently;
// GNU and LLVM linkers on Linux refuse the duplicate. Neither export is
// called here (the tests use the Rust types directly), so Linux does the same.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-tests=-Wl,--allow-multiple-definition");
        println!("cargo:rustc-link-arg-examples=-Wl,--allow-multiple-definition");
    }
    // Windows: the MSVC linker refuses it too (LNK1169) unless forced, and
    // the MinGW linker takes the GNU flag.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let flag = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") { "/FORCE:MULTIPLE" } else { "-Wl,--allow-multiple-definition" };
        println!("cargo:rustc-link-arg-tests={flag}");
        println!("cargo:rustc-link-arg-examples={flag}");
    }
}

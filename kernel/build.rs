fn main() {
    let linker_script = std::path::Path::new("src/arch/aarch64/linker.ld");
    let full_path = std::fs::canonicalize(linker_script)
        .expect("linker script not found at src/arch/aarch64/linker.ld");

    println!("cargo:rustc-link-arg=-T{}", full_path.display());
    println!("cargo:rerun-if-changed={}", linker_script.display());
}

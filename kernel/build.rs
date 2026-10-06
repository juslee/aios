fn main() {
    let linker_script = std::path::Path::new("src/arch/aarch64/linker.ld");
    let full_path = std::fs::canonicalize(linker_script)
        .expect("linker script not found at src/arch/aarch64/linker.ld");

    println!("cargo:rustc-link-arg=-T{}", full_path.display());
    // lld can place a section linker.ld does not name (an orphan) after
    // __kernel_end, where the script's 8 MiB boot-map ASSERT cannot see it.
    // Fail the link instead, so linker.ld places every section.
    println!("cargo:rustc-link-arg=--orphan-handling=error");
    println!("cargo:rerun-if-changed={}", linker_script.display());
}

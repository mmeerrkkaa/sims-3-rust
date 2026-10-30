use std::path::Path;

fn main() {
    // Optional developer scenarios are local files, not required game sources.
    println!("cargo::rustc-check-cfg=cfg(local_validation)");
    let files = ["src/open_lots_test.rs", "src/npc_ownership_test.rs"];
    for file in files { println!("cargo::rerun-if-changed={file}"); }
    if files.iter().all(|file| Path::new(file).is_file()) {
        println!("cargo::rustc-cfg=local_validation");
    }
}

use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let obj = out.join("fast_num.o");
    let lib = out.join("libfast_num.a");
    let status = Command::new("cc")
        .args(["-O3", "-c", "src/fast_num.c", "-o"])
        .arg(&obj)
        .status()
        .expect("C compiler is required for the hybrid Track B build");
    assert!(status.success(), "C helper compilation failed");
    let status = Command::new("ar")
        .args(["crus"])
        .arg(&lib)
        .arg(&obj)
        .status()
        .expect("ar is required for the hybrid Track B build");
    assert!(status.success(), "C helper archive failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=fast_num");
    println!("cargo:rerun-if-changed=src/fast_num.c");
}

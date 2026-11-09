fn main() {
    println!("cargo:rerun-if-changed=assets/fasttimer.ico");
    println!("cargo:rerun-if-changed=assets/fasttimer.rc");
    embed_resource::compile_for("assets/fasttimer.rc", ["fasttimer"], embed_resource::NONE)
        .manifest_optional()
        .expect("compile FastTimer's Windows icon");
}

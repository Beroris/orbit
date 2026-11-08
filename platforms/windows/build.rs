fn main() {
    println!("cargo:rerun-if-changed=res/orbit.rc");
    println!("cargo:rerun-if-changed=res/orbit.manifest");
    println!("cargo:rerun-if-changed=res/orbit.ico");
    embed_resource::compile("res/orbit.rc", embed_resource::NONE).manifest_required().unwrap();
}

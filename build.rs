fn main() {
    embed_resource::compile("res/real.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to compile res/real.rc");

    println!("cargo:rerun-if-changed=res/real.rc");
    println!("cargo:rerun-if-changed=res/app.ico");
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    verify_manifest_version();
    verify_resource_version();
}

fn verify_manifest_version() {
    let manifest =
        std::fs::read_to_string("res/app.manifest").expect("failed to read res/app.manifest");
    let expected = format!("version=\"{}.0\"", env!("CARGO_PKG_VERSION"));

    if !manifest.contains(&expected) {
        panic!("res/app.manifest must declare {expected}");
    }
}

fn verify_resource_version() {
    let resource = std::fs::read_to_string("res/real.rc").expect("failed to read res/real.rc");
    let version = env!("CARGO_PKG_VERSION");
    let numeric = format!("{},0", version.replace('.', ","));
    let display = format!("{version}.0");

    for expected in [
        format!("FILEVERSION {numeric}"),
        format!("PRODUCTVERSION {numeric}"),
        format!("VALUE \"FileVersion\", \"{display}\""),
        format!("VALUE \"ProductVersion\", \"{display}\""),
    ] {
        if !resource.contains(&expected) {
            panic!("res/real.rc must declare {expected}");
        }
    }
}

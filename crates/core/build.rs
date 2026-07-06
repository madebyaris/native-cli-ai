fn main() {
    capnpc::CompilerCommand::new()
        .file("schema/plugin.capnp")
        .run()
        .expect("capnp schema compilation failed");
    println!("cargo:rerun-if-changed=schema/plugin.capnp");
}

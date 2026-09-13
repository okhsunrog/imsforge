fn main() {
    println!("cargo:rerun-if-changed=../proto");
    protobuf_codegen::Codegen::new()
        .pure()
        .includes(["../proto"])
        .input("../proto/carrier_settings.proto")
        .input("../proto/carrier_list.proto")
        .cargo_out_dir("protos")
        .run_from_script();
}

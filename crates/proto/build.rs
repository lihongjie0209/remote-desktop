fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use vendored protoc binary so this works everywhere including cross/Docker builds.
    std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path()?);
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &["proto/remote_desktop.proto"],
            &["proto"],
        )?;
    Ok(())
}

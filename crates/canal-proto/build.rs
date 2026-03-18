fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = "../../proto";
    let protos = [
        "agent_service.proto",
        "common.proto",
        "llm_service.proto",
        "memory_service.proto",
        "tool_service.proto",
        "task_worker.proto",
    ];

    // Tell Cargo to rerun if any proto file changes
    for proto in &protos {
        println!("cargo:rerun-if-changed={}/{}", proto_dir, proto);
    }
    println!("cargo:rerun-if-changed={}", proto_dir);

    // Check if protoc is available
    let protoc_cmd = std::env::var("PROTOC").unwrap_or_else(|_| "protoc".to_string());
    if std::process::Command::new(&protoc_cmd)
        .arg("--version")
        .output()
        .is_err()
    {
        println!("cargo:warning=protoc not found, skipping proto compilation");
        // Generate empty stubs so tonic::include_proto! doesn't fail
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let out_path = std::path::Path::new(&out_dir);
        for pkg in &[
            "canal.agent",
            "canal.common",
            "canal.llm",
            "canal.memory",
            "canal.tools",
            "canal.worker",
        ] {
            let file_name = format!("{}.rs", pkg);
            std::fs::write(
                out_path.join(&file_name),
                format!("// Proto stub -- protoc not available at build time\n"),
            )?;
        }
        return Ok(());
    }

    // Check that all proto files exist
    for proto in &protos {
        let path = format!("{}/{}", proto_dir, proto);
        if !std::path::Path::new(&path).exists() {
            println!("cargo:warning=Proto file not found: {}", path);
            return Ok(());
        }
    }

    // Compile ALL proto files with both server and client enabled.
    // Consuming crates pick whichever side they need.
    let proto_paths: Vec<String> = protos
        .iter()
        .map(|p| format!("{}/{}", proto_dir, p))
        .collect();

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &proto_paths.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            &[proto_dir],
        )?;

    Ok(())
}

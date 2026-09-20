#![allow(clippy::uninlined_format_args)]

use cmake::Config;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const BOOTSTRAP_COMMAND: &str = "npm run bootstrap:deps";
const MARKER_FILE: &str = ".portus-dependency.json";

fn main() {
    reject_unsupported_native_features();

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("whisper-rs-sys must remain under <repo>/vendor/whisper-rs/sys")
        .to_path_buf();
    let dependency_manifest_path = repo_root.join("dependencies/whisper.json");
    let dependency_manifest_sha256 = sha256_file(&dependency_manifest_path, "dependency manifest");
    let dependency_manifest = read_json(&dependency_manifest_path, "dependency manifest");
    let prepared_relative = dependency_manifest
        .get("preparedDir")
        .and_then(Value::as_str)
        .expect("dependencies/whisper.json must define preparedDir");
    if prepared_relative != ".deps/whisper.cpp" {
        panic!("unexpected prepared whisper.cpp path; expected .deps/whisper.cpp");
    }

    let whisper_root = repo_root.join(prepared_relative);
    verify_prepared_dependency(
        &dependency_manifest,
        &dependency_manifest_sha256,
        &whisper_root,
    );

    println!(
        "cargo:rerun-if-changed={}",
        dependency_manifest_path.display()
    );
    println!("cargo:rerun-if-changed={}", whisper_root.display());
    println!("cargo:rerun-if-changed=wrapper.h");

    let target = env::var("TARGET").expect("TARGET");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    prepare_vulkan_build_environment(&repo_root, &target);

    if let Some(cpp_stdlib) = get_cpp_link_stdlib(&target) {
        println!("cargo:rustc-link-lib=dylib={}", cpp_stdlib);
    }

    #[cfg(feature = "openmp")]
    {
        if target.contains("gnu") {
            println!("cargo:rustc-link-lib=gomp");
        } else if target.contains("apple") {
            println!("cargo:rustc-link-lib=omp");
            println!("cargo:rustc-link-search=/opt/homebrew/opt/libomp/lib");
        }
    }

    generate_bindings(&manifest_dir, &whisper_root, &out);

    if env::var("DOCS_RS").is_ok() {
        return;
    }

    let mut config = Config::new(&whisper_root);
    config
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("WHISPER_ALL_WARNINGS", "OFF")
        .define("WHISPER_ALL_WARNINGS_3RD_PARTY", "OFF")
        .define("WHISPER_BUILD_TESTS", "OFF")
        .define("WHISPER_BUILD_EXAMPLES", "OFF")
        .define("WHISPER_BUILD_SERVER", "OFF")
        .define("WHISPER_CURL", "OFF")
        .define("WHISPER_SDL2", "OFF")
        .define("WHISPER_COREML", "OFF")
        .define("WHISPER_OPENVINO", "OFF")
        .define("GGML_CPU", "ON")
        .define("GGML_BLAS", "OFF")
        .define("GGML_CANN", "OFF")
        .define("GGML_CUDA", "OFF")
        .define("GGML_HIP", "OFF")
        .define("GGML_METAL", "OFF")
        .define("GGML_MUSA", "OFF")
        .define("GGML_RPC", "OFF")
        .define("GGML_SYCL", "OFF")
        .define("GGML_VIRTGPU", "OFF")
        .define("GGML_VULKAN", "ON")
        .define("GGML_WEBGPU", "OFF")
        .define("GGML_ZDNN", "OFF")
        .define("GGML_OPENCL", "OFF")
        .define("GGML_HEXAGON", "OFF")
        .define("GGML_ZENDNN", "OFF")
        .define("GGML_OPENVINO", "OFF")
        .define("GGML_CPU_KLEIDIAI", "OFF")
        .define("GGML_LLAMAFILE", "OFF")
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        .pic(true);

    if cfg!(target_os = "windows") {
        config.cxxflag("/utf-8");
        println!("cargo:rustc-link-lib=advapi32");
    }

    if cfg!(feature = "openmp") {
        config.define("GGML_OPENMP", "ON");
    } else {
        config.define("GGML_OPENMP", "OFF");
    }

    if cfg!(debug_assertions) || cfg!(feature = "force-debug") {
        config.define("CMAKE_BUILD_TYPE", "RelWithDebInfo");
        config.cxxflag("-DWHISPER_DEBUG");
    } else {
        config.define("CMAKE_BUILD_TYPE", "Release");
    }

    let destination = config.build();
    add_link_search_path(&out.join("build")).expect("failed to inspect CMake build output");
    println!("cargo:rustc-link-search=native={}", destination.display());
    println!("cargo:rustc-link-lib=static=whisper");
    println!("cargo:rustc-link-lib=static=ggml");
    println!("cargo:rustc-link-lib=static=ggml-base");
    println!("cargo:rustc-link-lib=static=ggml-cpu");
    println!("cargo:rustc-link-lib=static=ggml-vulkan");
    if target.contains("windows") {
        println!("cargo:rustc-link-lib=dylib=vulkan-1");
    } else if target.contains("linux") {
        println!("cargo:rustc-link-lib=dylib=vulkan");
    }

    println!(
        "cargo:WHISPER_CPP_VERSION={}",
        get_whisper_cpp_version(&whisper_root)
            .expect("failed to read prepared whisper.cpp CMake config")
            .expect("could not find whisper.cpp version declaration"),
    );
}

fn reject_unsupported_native_features() {
    const FEATURES: &[(&str, &str)] = &[
        ("CARGO_FEATURE_COREML", "coreml"),
        ("CARGO_FEATURE_CUDA", "cuda"),
        ("CARGO_FEATURE_HIPBLAS", "hipblas"),
        ("CARGO_FEATURE_INTEL_SYCL", "intel-sycl"),
        ("CARGO_FEATURE_METAL", "metal"),
        ("CARGO_FEATURE_OPENBLAS", "openblas"),
    ];
    for (variable, feature) in FEATURES {
        if env::var_os(variable).is_some() {
            panic!(
                "whisper-rs feature '{}' is not part of the PortusEchoes v1 CPU + Vulkan native dependency",
                feature
            );
        }
    }
}

fn prepare_vulkan_build_environment(repo_root: &Path, target: &str) {
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");

    if target.contains("windows") {
        let sdk = resolve_windows_vulkan_sdk().unwrap_or_else(|| {
            panic!(
                "PortusEchoes CPU + Vulkan build requires the LunarG Vulkan SDK. Install it or set VULKAN_SDK before building."
            )
        });
        validate_windows_vulkan_sdk(&sdk);
        println!(
            "cargo:rustc-link-search=native={}",
            sdk.join("Lib").display()
        );
        env::set_var("VULKAN_SDK", &sdk);
        if let Some(spirv_path) = find_file_recursive(&sdk, "SPIRV-HeadersConfig.cmake") {
            if let Some(spirv_cmake_dir) = spirv_path.parent().and_then(|p| p.parent()) {
                let mut prefix_paths = vec![spirv_cmake_dir.to_path_buf(), sdk.clone()];
                if let Some(existing) = env::var_os("CMAKE_PREFIX_PATH") {
                    prefix_paths.extend(env::split_paths(&existing));
                }
                if let Ok(joined) = env::join_paths(prefix_paths) {
                    env::set_var("CMAKE_PREFIX_PATH", joined);
                }
            }
        }
        prepend_path(&sdk.join("Bin"));
        let sdk = PathBuf::from(sdk);
        if sdk.join("lib").is_dir() {
            println!(
                "cargo:rustc-link-search=native={}",
                sdk.join("lib").display()
            );
        }
        prepend_path(&sdk.join("bin"));
    }

    run_vulkan_cmake_preflight(repo_root);
}

fn resolve_windows_vulkan_sdk() -> Option<PathBuf> {
    if let Some(value) = env::var_os("VULKAN_SDK") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return Some(path);
        }
    }

    let root = Path::new(r"C:\VulkanSDK");
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = parse_numeric_version(&name)?;
            Some((version, path))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, path)| path)
}

fn parse_numeric_version(value: &str) -> Option<Vec<u64>> {
    let parts = value
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

fn find_file_recursive(dir: &Path, target: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_file() {
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(target))
            {
                return Some(path);
            }
        } else if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, target) {
                return Some(found);
            }
        }
    }
    None
}

fn validate_windows_vulkan_sdk(sdk: &Path) {
    for relative in [
        "Include/vulkan/vulkan.h",
        "Lib/vulkan-1.lib",
        "Bin/glslc.exe",
    ] {
        if !sdk.join(relative).is_file() {
            panic!(
                "PortusEchoes Vulkan SDK is incomplete (missing {}). Reinstall the LunarG Vulkan SDK with development and shader-tool components.",
                relative
            );
        }
    }
    let found_spirv = sdk.join("Lib/cmake/SPIRV-Headers/SPIRV-HeadersConfig.cmake").is_file()
        || sdk.join("share/cmake/SPIRV-Headers/SPIRV-HeadersConfig.cmake").is_file()
        || find_file_recursive(sdk, "SPIRV-HeadersConfig.cmake").is_some();
    if !found_spirv {
        panic!(
            "PortusEchoes Vulkan SDK is incomplete (missing SPIRV-HeadersConfig.cmake). Reinstall the LunarG Vulkan SDK with development and shader-tool components."
        );
    }
}

fn prepend_path(directory: &Path) {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    let joined = env::join_paths(paths).expect("failed to construct PATH for Vulkan toolchain");
    env::set_var("PATH", joined);
}

fn run_vulkan_cmake_preflight(repo_root: &Path) {
    let root = repo_root
        .join("target")
        .join(format!(".portus-vulkan-preflight-{}", std::process::id()));
    let source = root.join("src");
    let build = root.join("build");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&source).expect("failed to create Vulkan preflight source directory");
    std::fs::write(
        source.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.24)\nproject(portus_vulkan_preflight LANGUAGES C CXX)\nfind_package(Vulkan COMPONENTS glslc REQUIRED)\nif (DEFINED ENV{VULKAN_SDK})\n    list(APPEND CMAKE_PREFIX_PATH \"$ENV{VULKAN_SDK}\")\nendif()\nif (DEFINED ENV{CMAKE_PREFIX_PATH})\n    list(APPEND CMAKE_PREFIX_PATH \"$ENV{CMAKE_PREFIX_PATH}\")\nendif()\nfind_package(SPIRV-Headers CONFIG REQUIRED)\n",
    )
    .expect("failed to write Vulkan preflight CMake project");
    let output = Command::new("cmake")
        .arg("-S")
        .arg(&source)
        .arg("-B")
        .arg(&build)
        .output()
        .unwrap_or_else(|_| {
            panic!(
                "PortusEchoes CPU + Vulkan build requires CMake 3.24 or newer plus the Vulkan development toolchain."
            )
        });
    let _ = std::fs::remove_dir_all(&root);

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "PortusEchoes Vulkan build prerequisites are incomplete. Required: Vulkan headers/loader, glslc, and a CMake-discoverable SPIRV-Headers package. CMake preflight output:\n{}\n{}",
            stdout.trim(),
            stderr.trim()
        );
    }
}

fn read_json(path: &Path, label: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| {
        panic!(
            "PortusEchoes {} is unavailable at {}. Run `{}` from the repository root.",
            label,
            path.display(),
            BOOTSTRAP_COMMAND
        )
    });
    serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("invalid JSON in {}: {}", label, path.display()))
}

fn sha256_file(path: &Path, label: &str) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|_| {
        panic!(
            "PortusEchoes {} is unavailable at {}. Run `{}` from the repository root.",
            label,
            path.display(),
            BOOTSTRAP_COMMAND
        )
    });
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest_paths(manifest: &Value, key: &str) -> Vec<String> {
    manifest
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("dependencies/whisper.json must define {} as an array", key))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| {
                    panic!("dependencies/whisper.json {} entries must be strings", key)
                })
                .replace('\\', "/")
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_string()
        })
        .collect()
}

fn validate_prepared_allowlist(manifest: &Value, whisper_root: &Path) {
    let allowed_files = manifest_paths(manifest, "files");
    let allowed_trees = manifest_paths(manifest, "trees");

    fn walk(root: &Path, directory: &Path, files: &[String], trees: &[String]) {
        for entry in std::fs::read_dir(directory)
            .unwrap_or_else(|_| panic!("failed to inspect prepared whisper.cpp tree"))
        {
            let entry =
                entry.unwrap_or_else(|_| panic!("failed to inspect prepared whisper.cpp entry"));
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .unwrap_or_else(|_| panic!("failed to inspect prepared whisper.cpp metadata"));
            let relative = path
                .strip_prefix(root)
                .expect("prepared dependency entry must remain under whisper root")
                .to_string_lossy()
                .replace('\\', "/");

            if metadata.file_type().is_symlink() {
                panic!(
                    "PortusEchoes prepared whisper.cpp dependency contains a symbolic link: {}",
                    relative
                );
            }
            if metadata.is_dir() {
                walk(root, &path, files, trees);
                continue;
            }
            if !metadata.is_file() {
                panic!(
                    "PortusEchoes prepared whisper.cpp dependency contains an unsupported filesystem entry: {}",
                    relative
                );
            }
            if relative == MARKER_FILE {
                continue;
            }

            let allowed = files.iter().any(|file| file == &relative)
                || trees
                    .iter()
                    .any(|tree| relative == *tree || relative.starts_with(&format!("{tree}/")));
            if !allowed {
                panic!(
                    "PortusEchoes prepared whisper.cpp dependency contains non-allowlisted source: {}. Run `{}` from the repository root.",
                    relative,
                    BOOTSTRAP_COMMAND
                );
            }
        }
    }

    walk(whisper_root, whisper_root, &allowed_files, &allowed_trees);
}

fn verify_prepared_dependency(manifest: &Value, manifest_sha256: &str, whisper_root: &Path) {
    let root_metadata = std::fs::symlink_metadata(whisper_root).ok();
    if root_metadata
        .as_ref()
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
        .unwrap_or(true)
    {
        panic!(
            "PortusEchoes native whisper.cpp dependency is not prepared. Run `{}` from the repository root.",
            BOOTSTRAP_COMMAND
        );
    }

    let marker_path = whisper_root.join(MARKER_FILE);
    let marker_metadata = std::fs::symlink_metadata(&marker_path).ok();
    if marker_metadata
        .as_ref()
        .map(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        .unwrap_or(true)
    {
        panic!(
            "PortusEchoes native whisper.cpp dependency has no valid authoritative marker. Run `{}` from the repository root.",
            BOOTSTRAP_COMMAND
        );
    }

    let marker = read_json(&marker_path, "prepared dependency marker");
    for key in [
        "name",
        "release",
        "commit",
        "archiveSha256",
        "transformVersion",
    ] {
        if marker.get(key) != manifest.get(key) {
            panic!(
                "PortusEchoes prepared whisper.cpp dependency is stale for manifest field '{}'. Run `{}` from the repository root.",
                key,
                BOOTSTRAP_COMMAND
            );
        }
    }
    if marker.get("manifestSha256").and_then(Value::as_str) != Some(manifest_sha256) {
        panic!(
            "PortusEchoes prepared whisper.cpp dependency does not match the committed dependency manifest. Run `{}` from the repository root.",
            BOOTSTRAP_COMMAND
        );
    }

    for relative in manifest_paths(manifest, "requiredFiles") {
        if !whisper_root.join(&relative).is_file() {
            panic!(
                "PortusEchoes prepared whisper.cpp dependency is incomplete (missing {}). Run `{}` from the repository root.",
                relative,
                BOOTSTRAP_COMMAND
            );
        }
    }

    for prohibited in manifest_paths(manifest, "prohibitedPaths") {
        if whisper_root.join(&prohibited).exists() {
            panic!(
                "PortusEchoes prepared whisper.cpp dependency contains prohibited source: {}",
                prohibited
            );
        }
    }

    validate_prepared_allowlist(manifest, whisper_root);
}

fn generate_bindings(manifest_dir: &Path, whisper_root: &Path, out: &Path) {
    if env::var("WHISPER_DONT_GENERATE_BINDINGS").is_ok() {
        std::fs::copy(
            manifest_dir.join("src/bindings.rs"),
            out.join("bindings.rs"),
        )
        .expect("failed to copy bundled whisper bindings");
        return;
    }

    let package_msrv = match option_env!("CARGO_PKG_RUST_VERSION") {
        Some(v) if !v.is_empty() => {
            let version = semver::Version::parse(v).expect("invalid CARGO_PKG_RUST_VERSION");
            bindgen::RustTarget::stable(version.minor, version.patch)
        }
        _ => bindgen::RustTarget::stable(88, 0),
    }
    .map_err(|v| v.to_string())
    .expect("unsupported Rust target for bindgen");

    let wrapper = manifest_dir.join("wrapper.h");
    let mut builder = bindgen::Builder::default()
        .rust_edition(bindgen::RustEdition::Edition2021)
        .rust_target(package_msrv)
        .header(wrapper.display().to_string())
        .clang_arg(format!("-I{}", whisper_root.display()))
        .clang_arg(format!("-I{}", whisper_root.join("include").display()))
        .clang_arg(format!("-I{}", whisper_root.join("ggml/include").display()))
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    if cfg!(feature = "vulkan") {
        builder = builder.clang_arg("-DGGML_USE_VULKAN");
    }
    let bindings = builder.generate();

    match bindings {
        Ok(bindings) => bindings
            .write_to_file(out.join("bindings.rs"))
            .expect("could not write generated whisper bindings"),
        Err(error) => {
            println!("cargo:warning=Unable to generate bindings: {}", error);
            println!("cargo:warning=Using bundled bindings.rs, which may be out of date");
            std::fs::copy(
                manifest_dir.join("src/bindings.rs"),
                out.join("bindings.rs"),
            )
            .expect("unable to copy bundled whisper bindings");
        }
    }
}

fn get_cpp_link_stdlib(target: &str) -> Option<&'static str> {
    if target.contains("msvc") {
        None
    } else if target.contains("apple") || target.contains("freebsd") || target.contains("openbsd") {
        Some("c++")
    } else if target.contains("android") {
        Some("c++_shared")
    } else {
        Some("stdc++")
    }
}

fn add_link_search_path(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        println!("cargo:rustc-link-search={}", dir.display());
        for entry in std::fs::read_dir(dir)? {
            add_link_search_path(&entry?.path())?;
        }
    }
    Ok(())
}

fn get_whisper_cpp_version(whisper_root: &Path) -> std::io::Result<Option<String>> {
    let cmake_lists = BufReader::new(File::open(whisper_root.join("CMakeLists.txt"))?);
    for line in cmake_lists.lines() {
        let line = line?;
        if let Some(suffix) = line.strip_prefix(r#"project("whisper.cpp" VERSION "#) {
            return Ok(Some(suffix.trim_end_matches(')').into()));
        }
    }
    Ok(None)
}

import assert from "node:assert/strict";
import crypto from "node:crypto";
import fsp from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import * as tar from "tar";

import { bootstrapDependency, validatePreparedTree } from "./bootstrap-whisper.mjs";

const ROOT_CMAKE = `if (CMAKE_SOURCE_DIR STREQUAL CMAKE_CURRENT_SOURCE_DIR)\n    set(WHISPER_STANDALONE ON)\n\n    include(git-vars)\n\n    # configure project version\n    configure_file(\${CMAKE_SOURCE_DIR}/bindings/javascript/package-tmpl.json \${CMAKE_SOURCE_DIR}/bindings/javascript/package.json @ONLY)\nelse()\n    set(WHISPER_STANDALONE OFF)\nendif()\n\nset_target_properties(parakeet PROPERTIES PUBLIC_HEADER \${CMAKE_CURRENT_SOURCE_DIR}/include/parakeet.h)\ninstall(TARGETS parakeet LIBRARY PUBLIC_HEADER)\n\ntarget_compile_definitions(parakeet PRIVATE\n    PARAKEET_VERSION="\${PROJECT_VERSION}"\n)\n\nset(PARAKEET_INCLUDE_INSTALL_DIR \${CMAKE_INSTALL_INCLUDEDIR} CACHE PATH "Location")\nset(PARAKEET_LIB_INSTALL_DIR \${CMAKE_INSTALL_LIBDIR} CACHE PATH "Location")\nset(PARAKEET_BIN_INSTALL_DIR \${CMAKE_INSTALL_BINDIR} CACHE PATH "Location")\nconfigure_file(cmake/parakeet.pc.in "\${CMAKE_CURRENT_BINARY_DIR}/parakeet.pc" @ONLY)\ninstall(FILES "\${CMAKE_CURRENT_BINARY_DIR}/parakeet.pc"\n        DESTINATION \${CMAKE_INSTALL_LIBDIR}/pkgconfig)\n`;

const VULKAN_CMAKE = `    include(ExternalProject)\n\n    if (CMAKE_CROSSCOMPILING)\n        list(APPEND VULKAN_SHADER_GEN_CMAKE_ARGS -DCMAKE_TOOLCHAIN_FILE=\${HOST_CMAKE_TOOLCHAIN_FILE})\n        message(STATUS "vulkan-shaders-gen toolchain file: \${HOST_CMAKE_TOOLCHAIN_FILE}")\n    endif()\n\n    ExternalProject_Add(\n        vulkan-shaders-gen\n        SOURCE_DIR \${CMAKE_CURRENT_SOURCE_DIR}/vulkan-shaders\n        CMAKE_ARGS -DCMAKE_INSTALL_PREFIX=\${CMAKE_BINARY_DIR}/$<CONFIG>\n                   -DCMAKE_INSTALL_BINDIR=.\n                   -DCMAKE_BUILD_TYPE=$<CONFIG>\n                   \${VULKAN_SHADER_GEN_CMAKE_ARGS}\n\n        BUILD_COMMAND \${CMAKE_COMMAND} --build . --config $<CONFIG>\n        BUILD_ALWAYS  TRUE\n\n        # NOTE: When DESTDIR is set using Makefile generators and\n        # "make install" triggers the build step, vulkan-shaders-gen\n        # would be installed into the DESTDIR prefix, so it is unset\n        # to ensure that does not happen.\n\n        INSTALL_COMMAND \${CMAKE_COMMAND} -E env --unset=DESTDIR\n                        \${CMAKE_COMMAND} --install . --config $<CONFIG>\n    )\n\n    set (_ggml_vk_host_suffix $<IF:$<STREQUAL:\${CMAKE_HOST_SYSTEM_NAME},Windows>,.exe,>)\n    set (_ggml_vk_genshaders_dir "\${CMAKE_BINARY_DIR}/$<CONFIG>")\n    set (_ggml_vk_genshaders_cmd "\${_ggml_vk_genshaders_dir}/vulkan-shaders-gen\${_ggml_vk_host_suffix}")\n`;

const SRC_CMAKE = `add_library(parakeet\n            ../include/parakeet.h\n            parakeet-arch.h\n            parakeet.cpp\n            )\n\ntarget_include_directories(parakeet PUBLIC . ../include)\ntarget_compile_features   (parakeet PUBLIC cxx_std_11)\ntarget_link_libraries(parakeet PUBLIC ggml Threads::Threads)\n\nset_target_properties(parakeet PROPERTIES\n    VERSION \${PROJECT_VERSION}\n    SOVERSION \${SOVERSION}\n)\n\nif (CMAKE_CXX_BYTE_ORDER STREQUAL "BIG_ENDIAN")\n    set(WHISPER_EXTRA_FLAGS \${WHISPER_EXTRA_FLAGS} -DWHISPER_BIG_ENDIAN)\n    set(PARAKEET_EXTRA_FLAGS \${PARAKEET_EXTRA_FLAGS} -DPARAKEET_BIG_ENDIAN)\nendif()\n\nif (PARAKEET_EXTRA_FLAGS)\n    target_compile_options(parakeet PRIVATE \${PARAKEET_EXTRA_FLAGS})\nendif()\n\nif (BUILD_SHARED_LIBS)\n    set_target_properties(whisper PROPERTIES POSITION_INDEPENDENT_CODE ON)\n    target_compile_definitions(whisper PRIVATE WHISPER_SHARED WHISPER_BUILD)\n\n    set_target_properties(parakeet PROPERTIES POSITION_INDEPENDENT_CODE ON)\n    target_compile_definitions(parakeet PRIVATE PARAKEET_SHARED PARAKEET_BUILD)\nendif()\n`;

async function createFixture({
  omitRequiredHeader = false,
  omitRequiredVulkan = false,
  unexpectedCmake = false,
} = {}) {
  const root = await fsp.mkdtemp(path.join(os.tmpdir(), "portus-whisper-bootstrap-"));
  await fsp.mkdir(path.join(root, "dependencies"), { recursive: true });
  const archiveRootName = "whisper.cpp-deadbeef";
  const archiveSource = path.join(root, "archive-source", archiveRootName);
  const vulkanRoot = path.join(archiveSource, "ggml", "src", "ggml-vulkan");
  const shaderRoot = path.join(vulkanRoot, "vulkan-shaders");
  await fsp.mkdir(path.join(archiveSource, "include"), { recursive: true });
  await fsp.mkdir(path.join(archiveSource, "src"), { recursive: true });
  await fsp.mkdir(path.join(archiveSource, "extra"), { recursive: true });
  await fsp.mkdir(path.join(shaderRoot, "feature-tests"), { recursive: true });
  await fsp.mkdir(path.join(vulkanRoot, "cmake"), { recursive: true });
  await fsp.writeFile(path.join(archiveSource, "CMakeLists.txt"), unexpectedCmake ? "unexpected\n" : ROOT_CMAKE);
  await fsp.writeFile(path.join(archiveSource, "src", "CMakeLists.txt"), SRC_CMAKE);
  if (!omitRequiredHeader) await fsp.writeFile(path.join(archiveSource, "include", "whisper.h"), "header\n");
  if (!omitRequiredVulkan) await fsp.writeFile(path.join(vulkanRoot, "ggml-vulkan.cpp"), "vulkan source\n");
  await fsp.writeFile(path.join(vulkanRoot, "CMakeLists.txt"), VULKAN_CMAKE);
  await fsp.writeFile(path.join(shaderRoot, "CMakeLists.txt"), "shader cmake\n");
  await fsp.writeFile(path.join(shaderRoot, "vulkan-shaders-gen.cpp"), "generator\n");
  await fsp.writeFile(path.join(shaderRoot, "feature-tests", "probe.comp"), "shader\n");
  await fsp.writeFile(path.join(vulkanRoot, "cmake", "host-toolchain.cmake.in"), "cross only\n");
  await fsp.writeFile(path.join(archiveSource, "extra", "kept.txt"), "kept\n");
  await fsp.writeFile(path.join(archiveSource, "forbidden.txt"), "forbidden\n");

  const archivePath = path.join(root, "fixture.tar.gz");
  await tar.c({ gzip: true, cwd: path.join(root, "archive-source"), file: archivePath }, [archiveRootName]);
  const archive = await fsp.readFile(archivePath);
  const manifest = {
    schemaVersion: 1,
    name: "whisper.cpp",
    release: "fixture",
    commit: "d".repeat(40),
    archiveUrl: "https://invalid.example/fixture.tar.gz",
    archiveSha256: crypto.createHash("sha256").update(archive).digest("hex"),
    archiveBytes: archive.length,
    archiveRoot: archiveRootName,
    preparedDir: ".deps/whisper.cpp",
    transformVersion: 3,
    transform: "whisper-cmake-cpu-vulkan-win-native-v3",
    files: [
      "CMakeLists.txt",
      "src/CMakeLists.txt",
      "include/whisper.h",
      "extra/kept.txt",
      "ggml/src/ggml-vulkan/CMakeLists.txt",
      "ggml/src/ggml-vulkan/ggml-vulkan.cpp",
    ],
    trees: ["include", "ggml/src/ggml-vulkan/vulkan-shaders"],
    requiredFiles: [
      "CMakeLists.txt",
      "src/CMakeLists.txt",
      "include/whisper.h",
      "ggml/src/ggml-vulkan/CMakeLists.txt",
      "ggml/src/ggml-vulkan/ggml-vulkan.cpp",
      "ggml/src/ggml-vulkan/vulkan-shaders/CMakeLists.txt",
      "ggml/src/ggml-vulkan/vulkan-shaders/vulkan-shaders-gen.cpp",
    ],
    prohibitedPaths: [
      "forbidden.txt",
      "ggml/src/ggml-vulkan/cmake",
      "ggml/src/ggml-cuda",
      "ggml/src/ggml-hip",
      "ggml/src/ggml-metal",
      "ggml/src/ggml-sycl",
      "ggml/src/ggml-webgpu",
    ],
  };
  await writeManifest(root, manifest);
  return { root, archivePath, manifest };
}

async function writeManifest(root, manifest) {
  await fsp.writeFile(path.join(root, "dependencies", "whisper.json"), `${JSON.stringify(manifest, null, 2)}\n`);
}

async function readMarker(root) {
  return JSON.parse(await fsp.readFile(path.join(root, ".deps", "whisper.cpp", ".portus-dependency.json"), "utf8"));
}

test("valid verified archive prepares only allowlisted source and second run is a no-op", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));

  const first = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  assert.equal(first.status, "prepared");
  await fsp.stat(path.join(first.preparedRoot, "include", "whisper.h"));
  await fsp.stat(path.join(first.preparedRoot, "extra", "kept.txt"));
  await assert.rejects(fsp.stat(path.join(first.preparedRoot, "forbidden.txt")));
  await fsp.stat(path.join(first.preparedRoot, "ggml", "src", "ggml-vulkan", "ggml-vulkan.cpp"));
  await fsp.stat(path.join(first.preparedRoot, "ggml", "src", "ggml-vulkan", "vulkan-shaders", "vulkan-shaders-gen.cpp"));
  await assert.rejects(fsp.stat(path.join(first.preparedRoot, "ggml", "src", "ggml-vulkan", "cmake", "host-toolchain.cmake.in")));
  const transformedRoot = await fsp.readFile(path.join(first.preparedRoot, "CMakeLists.txt"), "utf8");
  const transformedSrc = await fsp.readFile(path.join(first.preparedRoot, "src", "CMakeLists.txt"), "utf8");
  assert.doesNotMatch(transformedRoot, /parakeet/i);
  assert.doesNotMatch(transformedSrc, /parakeet/i);

  const second = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  assert.equal(second.status, "ready");
});

test("checksum mismatch fails closed without authoritative prepared tree", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  fixture.manifest.archiveSha256 = "0".repeat(64);
  await writeManifest(fixture.root, fixture.manifest);
  await assert.rejects(
    bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true }),
    /SHA-256 mismatch/,
  );
  await assert.rejects(fsp.stat(path.join(fixture.root, ".deps", "whisper.cpp")));
});

test("partial prepared tree without marker is replaced atomically", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  const partial = path.join(fixture.root, ".deps", "whisper.cpp");
  await fsp.mkdir(partial, { recursive: true });
  await fsp.writeFile(path.join(partial, "partial.txt"), "partial\n");

  const result = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  assert.equal(result.status, "prepared");
  await fsp.stat(path.join(result.preparedRoot, "include", "whisper.h"));
  await assert.rejects(fsp.stat(path.join(result.preparedRoot, "partial.txt")));
});

test("stale or wrong release marker forces verified rebuild", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  const first = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  const markerPath = path.join(first.preparedRoot, ".portus-dependency.json");
  const marker = await readMarker(fixture.root);
  marker.release = "wrong-release";
  marker.commit = "e".repeat(40);
  await fsp.writeFile(markerPath, `${JSON.stringify(marker, null, 2)}\n`);

  const rebuilt = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  assert.equal(rebuilt.status, "prepared");
  const repaired = await readMarker(fixture.root);
  assert.equal(repaired.release, fixture.manifest.release);
  assert.equal(repaired.commit, fixture.manifest.commit);
});

test("old CPU-only transform marker is stale and forces verified GPU-target rebuild", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  const first = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  const markerPath = path.join(first.preparedRoot, ".portus-dependency.json");
  const marker = await readMarker(fixture.root);
  marker.transformVersion = 1;
  await fsp.writeFile(markerPath, `${JSON.stringify(marker, null, 2)}\n`);

  const rebuilt = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  assert.equal(rebuilt.status, "prepared");
  const repaired = await readMarker(fixture.root);
  assert.equal(repaired.transformVersion, 3);
});

test("missing required Vulkan source in verified archive fails before replacement", async t => {
  const fixture = await createFixture({ omitRequiredVulkan: true });
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  await assert.rejects(
    bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true }),
    /missing required source: ggml\/src\/ggml-vulkan\/ggml-vulkan\.cpp/,
  );
  await assert.rejects(fsp.stat(path.join(fixture.root, ".deps", "whisper.cpp")));
});

test("missing required source in verified archive fails before replacement", async t => {
  const fixture = await createFixture({ omitRequiredHeader: true });
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  await assert.rejects(
    bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true }),
    /missing required source/,
  );
  await assert.rejects(fsp.stat(path.join(fixture.root, ".deps", "whisper.cpp")));
});

test("unexpected upstream transform source text fails closed", async t => {
  const fixture = await createFixture({ unexpectedCmake: true });
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  await assert.rejects(
    bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true }),
    /upstream source transform mismatch/,
  );
  await assert.rejects(fsp.stat(path.join(fixture.root, ".deps", "whisper.cpp")));
});

test("prepared-tree validation rejects every unapproved GPU backend", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  const result = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });

  for (const backend of ["ggml-cuda", "ggml-hip", "ggml-metal", "ggml-sycl", "ggml-webgpu"]) {
    const backendRoot = path.join(result.preparedRoot, "ggml", "src", backend);
    await fsp.mkdir(backendRoot, { recursive: true });
    await fsp.writeFile(path.join(backendRoot, "probe.txt"), "no\n");
    await assert.rejects(validatePreparedTree(result.preparedRoot, fixture.manifest), /prohibited source/);
    await fsp.rm(backendRoot, { recursive: true, force: true });
  }
});

test("prepared-tree validation rejects prohibited and non-allowlisted files", async t => {
  const fixture = await createFixture();
  t.after(() => fsp.rm(fixture.root, { recursive: true, force: true }));
  const result = await bootstrapDependency({ repoRoot: fixture.root, archivePath: fixture.archivePath, quiet: true });
  await fsp.writeFile(path.join(result.preparedRoot, "forbidden.txt"), "no\n");
  await assert.rejects(validatePreparedTree(result.preparedRoot, fixture.manifest), /prohibited source/);
  await fsp.rm(path.join(result.preparedRoot, "forbidden.txt"));
  await fsp.writeFile(path.join(result.preparedRoot, "surprise.txt"), "no\n");
  await assert.rejects(validatePreparedTree(result.preparedRoot, fixture.manifest), /non-allowlisted source/);
});
